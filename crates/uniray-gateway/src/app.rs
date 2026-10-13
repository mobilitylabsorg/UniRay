//! The gateway's state: servers, subscriptions, the running core.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uniray_core::config::{self, LogSink, Runtime, Tun};
use uniray_core::engine as core_engine;
use uniray_core::hwid::{self, Device};
use uniray_core::model::{new_id, Mode, RoutingPreset, Server, ServerSource, Settings, Subscription};
use uniray_core::{links, subscription};

use crate::engine::{self, Engine};
use crate::net;
use crate::options::Options;

const LOG_LINES: usize = 300;
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_GRACE: Duration = Duration::from_secs(5);
const PING_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_UPDATE_HOURS: u32 = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Default)]
pub struct State {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub selected: Option<String>,
    pub status: Status,
    pub error: Option<String>,
    /// Unix seconds when the core came up.
    pub since: Option<u64>,
    pub updating: bool,
    pub pinging: bool,
}

struct Core {
    child: Child,
    /// For a server Xray cannot run itself.
    engine: Option<Engine>,
    metrics_port: u16,
    proxy_tags: Vec<String>,
}

impl Core {
    /// What ended, once Xray or the engine has.
    fn exited(&mut self) -> Option<String> {
        if let Ok(Some(status)) = self.child.try_wait() {
            return Some(format!("xray stopped ({status})"));
        }
        let status = self.engine.as_mut()?.exited()?;
        Some(format!("the engine stopped ({status})"))
    }
}

/// What survives a restart when `DATA_DIR` is mounted.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    /// Name of the selected server; ids change with every fetch.
    selected_name: Option<String>,
    device_seed: Option<String>,
    servers: Vec<Server>,
    subscriptions: Vec<Subscription>,
}

pub struct App {
    pub options: Options,
    pub device: Device,
    pub state: Mutex<State>,
    core: Mutex<Option<Core>>,
    pub logs: Mutex<VecDeque<String>>,
    /// The physical interface, for probes that must not take the tunnel.
    uplink: Option<String>,
    run_dir: PathBuf,
}

impl App {
    pub fn new(options: Options) -> Self {
        let uplink = net::default_interface();
        let mut saved = load_saved(&options);
        let device = device(&options, uplink.as_deref(), &mut saved);
        let mut state = State {
            servers: saved.servers.clone(),
            subscriptions: saved.subscriptions.clone(),
            ..State::default()
        };
        state.selected = saved
            .selected_name
            .as_ref()
            .and_then(|name| state.servers.iter().find(|server| &server.name == name))
            .map(|server| server.id.clone());
        let app = Self {
            options,
            device,
            state: Mutex::new(state),
            core: Mutex::new(None),
            logs: Mutex::new(VecDeque::new()),
            uplink,
            run_dir: std::env::temp_dir().join("uniray"),
        };
        app.load_links();
        app
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        println!("{line}");
        let mut logs = self.logs.lock().unwrap();
        if logs.len() == LOG_LINES {
            logs.pop_front();
        }
        logs.push_back(line);
    }

    /// The servers this image can run: WireGuard, AmneziaWG and Naive need
    /// the engine, Naive `naive` as well; without them such servers are left
    /// out with a note.
    fn supported(&self, servers: Vec<Server>) -> Vec<Server> {
        let (supported, other): (Vec<Server>, Vec<Server>) =
            servers.into_iter().partition(|server| runnable(&server.source, &self.options.engine, &self.options.naive));
        for server in &other {
            let missing = if self.options.engine.is_file() { &self.options.naive } else { &self.options.engine };
            self.log(format!("uniray: skipped {} ({}): no {}", server.name, server.protocol, missing.display()));
        }
        supported
    }

    /// Share links from `SRV*`, kept apart from the subscriptions.
    fn load_links(&self) {
        let text = self.options.links.join("\n");
        let imported = links::parse_many(&text);
        for error in &imported.errors {
            self.log(format!("uniray: skipped a link: {error}"));
        }
        let servers = self.supported(imported.servers);
        let mut state = self.state.lock().unwrap();
        state.servers.retain(|server| server.subscription_id.is_some());
        let position = 0;
        state.servers.splice(position..position, servers);
    }

    // ---- subscriptions -------------------------------------------------

    /// Fetches every subscription, keeping the old servers of one that fails.
    /// While the core runs, requests go through the tunnel; when that fails
    /// the core is stopped for a moment and the request is made directly, so
    /// a dead server never locks the gateway out of its subscription.
    pub fn update_subscriptions(&'static self) {
        {
            let mut state = self.state.lock().unwrap();
            if state.updating {
                return;
            }
            state.updating = true;
        }
        let urls = self.options.subscriptions.clone();
        let mut failed_while_running = Vec::new();
        for (index, url) in urls.iter().enumerate() {
            if let Err(err) = self.update_one(index, url) {
                let running = self.state.lock().unwrap().status == Status::Running;
                if running && err.network {
                    failed_while_running.push((index, url.clone()));
                } else {
                    self.log(format!("uniray: subscription {}: {}", index + 1, err.message));
                }
            }
        }
        if !failed_while_running.is_empty() {
            self.log("uniray: subscription unreachable through the tunnel, retrying directly");
            self.stop_core();
            for (index, url) in &failed_while_running {
                if let Err(err) = self.update_one(*index, url) {
                    self.log(format!("uniray: subscription {}: {}", index + 1, err.message));
                }
            }
            self.restart_core();
        }
        self.ensure_selection();
        self.state.lock().unwrap().updating = false;
        self.save();
    }

    fn update_one(&self, index: usize, url: &str) -> Result<(), FetchError> {
        let id = format!("sub-{}", index + 1);
        let result = self.fetch(url);
        let mut state = self.state.lock().unwrap();
        if !state.subscriptions.iter().any(|sub| sub.id == id) {
            state.subscriptions.push(Subscription { id: id.clone(), name: host_of(url), url: url.into(), ..Default::default() });
        }
        let parsed = match result {
            Ok(parsed) => parsed,
            Err(err) => {
                if let Some(sub) = state.subscriptions.iter_mut().find(|sub| sub.id == id) {
                    sub.last_error = Some(err.message.clone());
                }
                return Err(err);
            }
        };
        for error in &parsed.errors {
            self.log(format!("uniray: skipped an entry of subscription {}: {error}", index + 1));
        }
        let sub = state.subscriptions.iter_mut().find(|sub| sub.id == id).expect("inserted above");
        parsed.meta.apply(sub, now_secs() as i64);

        let old: Vec<Server> = state.servers.iter().filter(|server| server.subscription_id.as_deref() == Some(&id)).cloned().collect();
        let position = state
            .servers
            .iter()
            .position(|server| server.subscription_id.as_deref() == Some(&id))
            .unwrap_or(state.servers.len());
        state.servers.retain(|server| server.subscription_id.as_deref() != Some(&id));
        let position = position.min(state.servers.len());
        let new = subscription::reconcile(&old, self.supported(parsed.servers), &id);
        let count = new.len();
        state.servers.splice(position..position, new);
        drop(state);
        self.log(format!("uniray: subscription {} updated, {count} servers", index + 1));
        Ok(())
    }

    fn fetch(&self, url: &str) -> Result<subscription::Parsed, FetchError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(25)))
            .build()
            .into();
        let headers = hwid::request_headers(&self.device, self.options.send_hwid);
        let hwid_sent = headers.iter().any(|(name, _)| *name == "x-hwid");
        let mut request = agent.get(url);
        for (name, value) in &headers {
            request = request.header(*name, value);
        }
        let mut response = request
            .call()
            .map_err(|err| FetchError { network: true, message: err.to_string() })?;
        let status = response.status().as_u16();
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.as_str().to_ascii_lowercase(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .map_err(|err| FetchError { network: true, message: err.to_string() })?;
        subscription::parse_response(status, &response_headers, &body, hwid_sent)
            .map_err(|err| FetchError { network: false, message: err.to_string() })
    }

    /// Keeps a valid selection: the saved one, else `SERVER`, else the first.
    fn ensure_selection(&self) {
        let mut state = self.state.lock().unwrap();
        let valid = state.selected.as_ref().is_some_and(|id| state.servers.iter().any(|server| &server.id == id));
        if valid {
            return;
        }
        let preferred = self.options.preferred_server.as_ref().and_then(|wanted| {
            let wanted = wanted.to_lowercase();
            state.servers.iter().find(|server| server.name.to_lowercase().contains(&wanted))
        });
        state.selected = preferred.or_else(|| state.servers.first()).map(|server| server.id.clone());
    }

    pub fn select(&'static self, id: &str) -> Result<(), String> {
        {
            let mut state = self.state.lock().unwrap();
            if !state.servers.iter().any(|server| server.id == id) {
                return Err("no such server".into());
            }
            state.selected = Some(id.to_owned());
        }
        self.save();
        self.restart_core();
        Ok(())
    }

    /// Spawns the auto-update loop.
    pub fn spawn_updater(&'static self) {
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(15 * 60));
            let due = {
                let state = self.state.lock().unwrap();
                let now = now_secs() as i64;
                state.subscriptions.iter().any(|sub| {
                    let hours = self.options.update_hours.or(sub.update_interval_hours).unwrap_or(DEFAULT_UPDATE_HOURS);
                    sub.updated_at.is_none_or(|at| now - at >= i64::from(hours) * 3600)
                })
            };
            if due {
                self.update_subscriptions();
            }
        });
    }

    // ---- the core ------------------------------------------------------

    pub fn restart_core(&'static self) {
        self.stop_core();
        if let Err(err) = self.start_core() {
            self.log(format!("uniray: {err}"));
            let mut state = self.state.lock().unwrap();
            state.status = Status::Failed;
            state.error = Some(err);
        }
    }

    fn start_core(&'static self) -> Result<(), String> {
        let server = {
            let mut state = self.state.lock().unwrap();
            let server = state.selected.as_ref().and_then(|id| state.servers.iter().find(|server| &server.id == id)).cloned();
            state.status = Status::Starting;
            state.error = None;
            server
        };
        let Some(mut server) = server else {
            self.state.lock().unwrap().status = Status::Stopped;
            return Err("no servers yet: set SUB or SRV".into());
        };
        // A provider's own config: no split routing here, and no geo data in
        // the image. Done before the build, which then adds its own rules
        // (such as answering DNS that arrives through the tunnel).
        if let ServerSource::Config(provider) = &mut server.source {
            config::proxy_everything(provider, &self.options.remote_dns);
        }

        // Resolves the server's name, which the bypass rule needs as an
        // address. The gateway's tunnel is IPv4 only.
        let engine_run = core_engine::prepare(&server, free_port()?, false, &self.options.naive).map_err(|err| err.to_string())?;
        for warning in engine_run.iter().flat_map(|run| &run.warnings) {
            self.log(warning.clone());
        }

        let metrics_port = free_port()?;
        // The engine's own connections to its server leave by the normal
        // interface, not through Xray: less work for a small router, and no
        // chance of them looping back into the engine.
        let mut excluded = net::system_nameservers();
        excluded.extend(engine_run.iter().flat_map(|run| &run.runtime.bypass).filter_map(|bypass| match bypass.addr.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        }));
        let routes = net::tunnel_routes(&excluded);
        let settings = Settings {
            mode: Mode::Tun,
            mixed_port: self.options.mixed_port,
            allow_lan: true,
            routing_preset: RoutingPreset::Global,
            remote_dns: self.options.remote_dns.clone(),
            direct_dns: self.options.direct_dns.clone(),
            log_level: self.options.log_level.clone(),
            ..Settings::default()
        };
        let runtime = Runtime {
            tun: self.options.tun.then(|| Tun::Gateway { name: self.options.tun_name.clone(), routes }),
            mixed_port: Some(self.options.mixed_port),
            metrics_port: Some(metrics_port), metrics_socket: None,
            log: LogSink::Stdout,
            engine: engine_run.as_ref().map(|run| run.runtime.clone()),
        };
        let built = config::build(&server, &settings, &runtime).map_err(|err| err.to_string())?;

        std::fs::create_dir_all(&self.run_dir).map_err(|err| err.to_string())?;
        let path = self.run_dir.join("config.json");
        std::fs::write(&path, serde_json::to_vec(&built.config).map_err(|err| err.to_string())?)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;

        let mut child = Command::new(&self.options.xray)
            .arg("run")
            .arg("-c")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("cannot start {}: {err}", self.options.xray.display()))?;
        self.forward_output(child.stdout.take());
        self.forward_output(child.stderr.take());

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("xray exited during startup ({status}), see the log"));
            }
            let ready = [metrics_port, self.options.mixed_port]
                .iter()
                .all(|port| TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, *port)), Duration::from_millis(200)).is_ok());
            if ready {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("xray did not become ready in time".into());
            }
            thread::sleep(Duration::from_millis(100));
        }

        // Started after Xray, so that its config names Xray's process. Xray
        // connects to it only for traffic.
        let engine = match &engine_run {
            Some(run) => match Engine::start(self, &self.options.engine, run, child.id(), &self.run_dir) {
                Ok(engine) => Some(engine),
                Err(err) => {
                    terminate(&mut child);
                    return Err(err);
                }
            },
            None => None,
        };

        self.log(format!("uniray: connected to {}", server.name));
        *self.core.lock().unwrap() = Some(Core { child, engine, metrics_port, proxy_tags: built.proxy_tags });
        let mut state = self.state.lock().unwrap();
        state.status = Status::Running;
        state.since = Some(now_secs());
        Ok(())
    }

    pub fn stop_core(&self) {
        let Some(mut core) = self.core.lock().unwrap().take() else { return };
        terminate(&mut core.child);
        if let Some(engine) = core.engine.as_mut() {
            engine.stop();
        }
        let mut state = self.state.lock().unwrap();
        state.status = Status::Stopped;
        state.since = None;
    }

    /// Restarts the core when it dies on its own.
    pub fn spawn_supervisor(&'static self) {
        thread::spawn(move || {
            let mut backoff = Duration::from_secs(2);
            loop {
                thread::sleep(Duration::from_secs(2));
                // The rest of the core is stopped by the restart.
                let exited = self.core.lock().unwrap().as_mut().and_then(Core::exited);
                let failed = self.state.lock().unwrap().status == Status::Failed;
                if let Some(what) = exited {
                    self.log(format!("uniray: {what}, restarting in {}s", backoff.as_secs()));
                } else if !failed {
                    backoff = Duration::from_secs(2);
                    continue;
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
                self.restart_core();
            }
        });
    }

    pub fn forward_output(&'static self, stream: Option<impl Read + Send + 'static>) {
        let Some(stream) = stream else { return };
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                self.log(line);
            }
        });
    }

    /// Bytes sent and received through the proxy since the core started.
    pub fn traffic(&self) -> Option<(u64, u64)> {
        let (port, tags) = {
            let core = self.core.lock().unwrap();
            let core = core.as_ref()?;
            (core.metrics_port, core.proxy_tags.clone())
        };
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(1))).build().into();
        let vars: Value = serde_json::from_str(
            &agent.get(&format!("http://127.0.0.1:{port}/debug/vars")).call().ok()?.body_mut().read_to_string().ok()?,
        )
        .ok()?;
        let outbound = &vars["stats"]["outbound"];
        Some(tags.iter().fold((0, 0), |(up, down), tag| {
            (
                up + outbound[tag.as_str()]["uplink"].as_u64().unwrap_or(0),
                down + outbound[tag.as_str()]["downlink"].as_u64().unwrap_or(0),
            )
        }))
    }

    // ---- delay -----------------------------------------------------------

    /// TCP handshake time to every server, measured past the tunnel. A
    /// WireGuard server has no TCP to time: the engine brings its tunnel up
    /// and times a request through it.
    pub fn ping_all(&'static self) {
        let (targets, tunnels): (Vec<(String, String, u16)>, Vec<Server>) = {
            let mut state = self.state.lock().unwrap();
            if state.pinging {
                return;
            }
            state.pinging = true;
            let (tunnels, others): (Vec<&Server>, Vec<&Server>) =
                state.servers.iter().partition(|server| matches!(server.source, ServerSource::WireGuard(_)));
            (
                others.iter().map(|server| (server.id.clone(), server.address.clone(), server.port)).collect(),
                tunnels.into_iter().cloned().collect(),
            )
        };
        thread::spawn(move || {
            for chunk in targets.chunks(8) {
                let handles: Vec<_> = chunk
                    .iter()
                    .cloned()
                    .map(|(id, address, port)| {
                        let uplink = self.uplink.clone();
                        thread::spawn(move || {
                            let delay = if port == 0 || address.is_empty() {
                                None
                            } else {
                                net::tcp_ping(&address, port, uplink.as_deref(), PING_TIMEOUT)
                            };
                            (id, delay)
                        })
                    })
                    .collect();
                for handle in handles {
                    if let Ok((id, delay)) = handle.join() {
                        self.set_delay(&id, delay);
                    }
                }
            }
            // One at a time: each is a whole tunnel, in a container of 64 MB.
            for server in &tunnels {
                let delay = core_engine::prepare(server, 0, false, &self.options.naive)
                    .ok()
                    .flatten()
                    .and_then(|run| engine::probe(&self.options.engine, &run, &self.run_dir, &server.id));
                self.set_delay(&server.id, delay);
            }
            self.state.lock().unwrap().pinging = false;
        });
    }

    fn set_delay(&self, id: &str, delay: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        if let Some(server) = state.servers.iter_mut().find(|server| server.id == id) {
            server.delay_ms = Some(delay.map_or(-1, |ms| ms as i64));
        }
    }

    // ---- views and persistence ------------------------------------------

    pub fn view(&self) -> Value {
        let state = self.state.lock().unwrap();
        let servers: Vec<Value> = state
            .servers
            .iter()
            .map(|server| {
                json!({
                    "id": server.id,
                    "name": server.name,
                    "protocol": server.protocol,
                    "transport": server.transport,
                    "security": server.security,
                    "group": server.subscription_id,
                    "delay": server.delay_ms,
                })
            })
            .collect();
        let subscriptions: Vec<Value> = state
            .subscriptions
            .iter()
            .map(|sub| {
                json!({
                    "id": sub.id,
                    "name": sub.name,
                    "updatedAt": sub.updated_at,
                    "userInfo": sub.user_info,
                    "lastError": sub.last_error,
                    "supportUrl": sub.support_url,
                })
            })
            .collect();
        json!({
            "status": state.status,
            "error": state.error,
            "since": state.since,
            "selected": state.selected,
            "updating": state.updating,
            "pinging": state.pinging,
            "servers": servers,
            "subscriptions": subscriptions,
            "hwid": self.device.hwid,
            "tun": self.options.tun,
            "mixedPort": self.options.mixed_port,
            "version": env!("CARGO_PKG_VERSION"),
        })
    }

    fn save(&self) {
        if !self.options.data_dir.is_dir() {
            return;
        }
        let saved = {
            let state = self.state.lock().unwrap();
            Saved {
                selected_name: state
                    .selected
                    .as_ref()
                    .and_then(|id| state.servers.iter().find(|server| &server.id == id))
                    .map(|server| server.name.clone()),
                device_seed: load_saved(&self.options).device_seed,
                // Static links come from the environment every start.
                servers: state.servers.iter().filter(|server| server.subscription_id.is_some()).cloned().collect(),
                subscriptions: state.subscriptions.clone(),
            }
        };
        write_saved(&self.options, &saved);
    }
}

#[derive(Debug)]
struct FetchError {
    /// The request did not get an answer (as opposed to a bad answer).
    network: bool,
    message: String,
}

fn saved_path(options: &Options) -> PathBuf {
    options.data_dir.join("uniray.json")
}

fn load_saved(options: &Options) -> Saved {
    std::fs::read(saved_path(options))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_saved(options: &Options, saved: &Saved) {
    let path = saved_path(options);
    let tmp = path.with_extension("tmp");
    if let Ok(bytes) = serde_json::to_vec(saved) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// The HWID must stay the same across restarts or every restart takes another
/// device slot. Sources, best first: `HWID`, the seed kept in `DATA_DIR`, the
/// veth's MAC address (RouterOS keeps it in the container's settings).
fn device(options: &Options, uplink: Option<&str>, saved: &mut Saved) -> Device {
    let identity = if let Some(hwid) = options.hwid.clone() {
        hwid
    } else {
        let seed = saved.device_seed.clone().or_else(|| uplink.and_then(net::mac_address)).unwrap_or_else(|| {
            let seed = new_id();
            saved.device_seed = Some(seed.clone());
            if options.data_dir.is_dir() {
                write_saved(options, saved);
            }
            seed
        });
        hwid::derive_hwid(&seed)
    };
    Device {
        hwid: identity,
        os: "RouterOS".into(),
        os_version: std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_owned(),
        model: options.device_model.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// Whether the image has what `source` runs on.
fn runnable(source: &ServerSource, engine: &Path, naive: &Path) -> bool {
    match source {
        ServerSource::Outbound(_) | ServerSource::Config(_) => true,
        ServerSource::WireGuard(_) => engine.is_file(),
        ServerSource::Naive(_) => engine.is_file() && naive.is_file(),
    }
}

/// SIGTERM, which lets Xray remove its routes and interface, then a kill.
fn terminate(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        // SAFETY: the pid is our child, not yet reaped.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
            thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| err.to_string())
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).to_owned()
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_servers_need_their_binaries() {
        let dir = std::env::temp_dir().join(format!("uniray-gateway-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (engine, naive) = (dir.join("uniray-engine"), dir.join("naive"));
        let server = |link: &str| links::parse_many(link).servers.remove(0).source;
        let vless = server("vless://11111111-2222-3333-4444-555555555555@203.0.113.1:443?security=none#v");
        let wireguard = server(
            "wireguard://YNXtAzepDqRv9H52osJVDQnznT5AM11eCK3ESpwSt04=@203.0.113.1:51820\
             ?publickey=Z1XXLsKYkYxuiYjJIkRvtIKFepCYHTgON%2BGfPFp9rhs%3D&address=10.0.0.2%2F32#w",
        );
        let naive_server = server("naive+https://user:pass@example.com:443#n");

        // Neither binary: Xray's servers only.
        assert!(runnable(&vless, &engine, &naive));
        assert!(!runnable(&wireguard, &engine, &naive));
        assert!(!runnable(&naive_server, &engine, &naive));
        // The engine: WireGuard and AmneziaWG; Naive needs naive as well.
        std::fs::write(&engine, b"").unwrap();
        assert!(runnable(&wireguard, &engine, &naive));
        assert!(!runnable(&naive_server, &engine, &naive));
        std::fs::write(&naive, b"").unwrap();
        assert!(runnable(&naive_server, &engine, &naive));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
