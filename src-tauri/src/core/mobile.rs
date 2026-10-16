//! Mobile: the core runs inside the app process, behind the OS VPN service.
//!
//! The platform side (Kotlin `VpnPlugin` + `UniRayVpnService` on Android) owns
//! the TUN interface and libXray; this module hands it a ready config and
//! watches the result. Logs come back through files, traffic counters through
//! the metrics endpoint - the same way as on desktop.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::async_runtime::JoinHandle;
use tauri::plugin::{PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Wry};
use uniray_core::config::{self, LogSink, Runtime, Tun};
use uniray_core::engine::{self, EngineRun, Peers};
use uniray_core::hwid::derive_hwid;
use uniray_core::model::{PerAppMode, Server, Settings};

use super::{prepare_engine, ConnectionInfo, RunEnd, Status, PROBE_TIMEOUT};
use crate::app::{emit_state, App};
use crate::network::NetworkInfo;
use crate::error::{CmdError, CmdResult};
use crate::state::write_private;
use crate::{logs, stats};

/// Plain resolver for the core's own (Go-side) lookups, which do not resolve
/// the proxy server - Xray's built-in DNS does that over DoH - so this only
/// needs to be reachable. A public one, never the operator's.
const FALLBACK_DNS: &str = "8.8.8.8";
/// The loopback port the stealth tproxy inbound listens on; the iptables
/// TPROXY target points here. Matches the port in the generated config.
#[cfg(not(play_store))]
const STEALTH_TPROXY_PORT: u16 = 12345;
/// Checked every 3 s to avoid high IPC Binder wakeups while keeping UI responsive.
const WATCH_INTERVAL: Duration = Duration::from_secs(3);

/// Configures TCP keep-alive on all non-blackhole proxy outbounds so mobile
/// carrier CGNAT and Wi-Fi stateful firewalls do not drop idle TCP connections after 2-3 minutes.
/// The Linux kernel handles 0-byte keep-alive probes without waking user-space processes.
/// Also adjusts TUN MTU to 1360 to prevent packet fragmentation over cellular networks (LTE/5G).
fn apply_mobile_optimizations(config: &mut Value) {
    if let Some(outbounds) = config.get_mut("outbounds").and_then(|o| o.as_array_mut()) {
        for outbound in outbounds {
            if outbound["protocol"].as_str() != Some("blackhole") {
                if !outbound["streamSettings"].is_object() {
                    outbound["streamSettings"] = json!({});
                }
                if !outbound["streamSettings"]["sockopt"].is_object() {
                    outbound["streamSettings"]["sockopt"] = json!({});
                }
                let sockopt = &mut outbound["streamSettings"]["sockopt"];
                if sockopt.get("tcpKeepAliveInterval").is_none() {
                    sockopt["tcpKeepAliveInterval"] = 60.into();
                }
                if sockopt.get("tcpKeepAliveIdle").is_none() {
                    sockopt["tcpKeepAliveIdle"] = 60.into();
                }
            }
        }
    }
    if let Some(inbounds) = config.get_mut("inbounds").and_then(|i| i.as_array_mut()) {
        for inbound in inbounds {
            if inbound["protocol"].as_str() == Some("tun") {
                if !inbound["settings"].is_object() {
                    inbound["settings"] = json!({});
                }
                inbound["settings"]["mtu"] = json!(config::MOBILE_TUN_MTU);
            }
        }
    }
}

struct Vpn(PluginHandle<Wry>);

/// Registers the native half. It is not a separate crate: the Kotlin classes
/// live in the app module (`gen/android/app`), next to the libXray AAR.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new("vpn")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin("dev.uniray.app", "VpnPlugin")?;
                app.manage(Vpn(handle));
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

async fn call<T: serde::de::DeserializeOwned>(app: &AppHandle, command: &str, args: impl Serialize) -> CmdResult<T> {
    let vpn = app
        .try_state::<Vpn>()
        .ok_or_else(|| CmdError::new("unsupported", "VPN is not implemented on this platform yet"))?;
    vpn.0.run_mobile_plugin_async(command, args).await.map_err(|err| {
        use tauri::plugin::mobile::PluginInvokeError::InvokeRejected;
        match err {
            InvokeRejected(rejection) => {
                let message = rejection.message.unwrap_or_else(|| "the VPN service failed".into());
                match rejection.code.as_deref() {
                    Some("vpnDenied") => CmdError::new("vpnDenied", message),
                    Some("busy") => CmdError::new("delayWhileConnected", message),
                    // Lets the caller fall back from stealth to the VPN service.
                    Some("noRoot") => CmdError::new("noRoot", message),
                    _ => CmdError::core(message),
                }
            }
            other => CmdError::core(other.to_string()),
        }
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartArgs {
    /// The Xray config; the service adds the TUN fd and the asset directory.
    config: String,
    per_app_mode: PerAppMode,
    packages: Vec<String>,
    /// `ip:port` of the DNS server for the core's own lookups.
    dns: String,
    mtu: u32,
    /// What the VPN routes into itself (`address/length`, both families):
    /// everything but the engine's server. The engine and its naive are
    /// processes of their own, whose sockets VpnService cannot protect; left
    /// out of the VPN, their packets do not go through Xray a second time.
    routes: Vec<String>,
    /// Label shown for the VPN in Android's system settings.
    session_name: String,
    /// What the service needs to keep its notification current by itself
    /// once the app is gone: the server, and where to read the counters.
    server_name: String,
    /// The server's id: the Quick Settings tile starts the service again from
    /// the saved spec, and tells the app which server that is.
    server_id: String,
    metrics_socket: String,
    proxy_tags: Vec<String>,
    /// Hold a wake lock while connected (Settings::keep_awake).
    keep_awake: bool,
    #[serde(flatten)]
    engine: EngineArgs,
}

/// The engine for a server Xray cannot run (WireGuard, AmneziaWG), started by
/// the service before Xray; empty when there is none.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct EngineArgs {
    engine_mode: String,
    /// The engine's config (secrets included); the service writes it to a
    /// private file the engine removes once read.
    engine_config: String,
    engine_port: u16,
}

impl EngineArgs {
    fn new(run: Option<&EngineRun>, peers: &Peers, detached: bool) -> Self {
        run.map_or_else(Self::default, |run| Self {
            engine_mode: run.mode.to_owned(),
            engine_config: String::from_utf8(run.config_for(peers, detached)).expect("JSON is UTF-8"),
            engine_port: run.runtime.port,
        })
    }
}

#[derive(Deserialize)]
struct ServiceState {
    running: bool,
    /// The service wrote its own stop; otherwise it was killed.
    #[serde(default)]
    reported: bool,
    /// Why it stopped, when it was not asked to.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct StopAnswer {
    /// What the service could not tear down.
    #[serde(default)]
    error: Option<String>,
}

/// The DNS servers of the network beneath the VPN.
pub(super) async fn system_dns(app: &AppHandle) -> Vec<String> {
    network_info(app).await.map(|network| network.dns).unwrap_or_default()
}

pub async fn network_info(app: &AppHandle) -> Option<NetworkInfo> {
    call::<NetworkInfo>(app, "networkInfo", json!({})).await.ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_id: String,
    model: String,
    os_version: String,
    /// Android TV / Google TV.
    #[serde(default)]
    tv: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub package: String,
    pub label: String,
    pub system: bool,
}

#[derive(Deserialize)]
struct AppList {
    apps: Vec<InstalledApp>,
}

/// What is needed to pick a running connection up again after the activity
/// was recreated while the VPN service kept going.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    server_id: String,
    since: i64,
    metrics_socket: PathBuf,
    proxy_tags: Vec<String>,
}

struct RunFiles {
    error_log: PathBuf,
    access_log: PathBuf,
    session: PathBuf,
    /// Counters socket; the directory is private to the app.
    metrics: PathBuf,
}

/// The lock file Xray keeps next to a Unix socket it listens on.
fn socket_lock(socket: &std::path::Path) -> PathBuf {
    let mut lock = socket.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}

fn run_files(app: &AppHandle) -> RunFiles {
    let dir = &app.state::<App>().paths.run_dir;
    RunFiles {
        error_log: dir.join("error.log"),
        access_log: dir.join("access.log"),
        session: dir.join("session.json"),
        metrics: dir.join("m.sock"),
    }
}

async fn service_running(app: &AppHandle) -> bool {
    service_state(app).await.is_some_and(|state| state.running)
}

async fn service_state(app: &AppHandle) -> Option<ServiceState> {
    call::<ServiceState>(app, "state", json!({})).await.ok()
}

pub struct Running {
    /// First log line of this run, see [`logs::mark`].
    pub log_mark: u64,
    stats: JoinHandle<()>,
    watcher: JoinHandle<()>,
    tailer: JoinHandle<()>,
}

impl Running {
    pub fn mixed_port(&self) -> Option<u16> {
        None
    }

    pub async fn start(app: &AppHandle, server: &Server, settings: &Settings, generation: u64) -> CmdResult<Self> {
        let settings = &super::with_provider_dns(app, settings).await;
        let files = run_files(app);
        for log in [&files.error_log, &files.access_log] {
            write_private(log, b"").map_err(CmdError::io)?;
        }
        // A stale socket file would make the core fail to listen.
        let _ = std::fs::remove_file(&files.metrics);
        // Xray guards the socket with "<socket>.lock". One left by an earlier
        // run - the in-app core's, or the root one's - may not be openable by
        // the next core ("permission denied"), so it goes as well.
        let _ = std::fs::remove_file(socket_lock(&files.metrics));
        let name = {
            let name = settings.interface_name.trim();
            if name.is_empty() { config::DEFAULT_INTERFACE_NAME.to_owned() } else { name.to_owned() }
        };
        let metrics = files.metrics.to_string_lossy().into_owned();
        let log = LogSink::Files {
            error: files.error_log.to_string_lossy().into_owned(),
            access: files.access_log.to_string_lossy().into_owned(),
        };
        if let Some(network) = network_info(app).await {
            logs::push(app, &format!("[Info] network: {}", network.kind));
        }

        let log_mark = logs::mark();
        let tailer = logs::tail(app.clone(), vec![files.error_log.clone(), files.access_log.clone()]);

        // Stealth (no VpnService) when the user asked and root is there;
        // otherwise, and when root turns out to be missing, the VPN service.
        // The Google Play edition has only the VPN service (edition.rs).
        #[cfg(play_store)]
        let outcome = start_vpn_service(app, server, settings, &name, &metrics, &log).await;
        #[cfg(not(play_store))]
        let outcome = if settings.stealth {
            match start_stealth(app, server, settings, &name, &metrics, &log).await {
                Err(err) if err.code == "noRoot" => {
                    logs::push(app, "[Info] stealth needs root, unavailable here; using the VPN service");
                    start_vpn_service(app, server, settings, &name, &metrics, &log).await
                }
                other => other,
            }
        } else {
            start_vpn_service(app, server, settings, &name, &metrics, &log).await
        };
        let proxy_tags = match outcome {
            Ok(tags) => tags,
            Err(err) => {
                // Give the tailer a moment to pick up what the core logged.
                tokio::time::sleep(Duration::from_millis(600)).await;
                tailer.abort();
                return Err(err);
            }
        };

        let session = Session {
            server_id: server.id.clone(),
            since: logs::now_secs(),
            metrics_socket: files.metrics.clone(),
            proxy_tags: proxy_tags.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec(&session) {
            let _ = write_private(&files.session, &bytes);
        }

        Ok(Self {
            log_mark,
            stats: stats::spawn(app.clone(), stats::MetricsSource::Socket(files.metrics), proxy_tags),
            watcher: spawn_watcher(app.clone(), generation),
            tailer,
        })
    }

    /// Stops the service. Returns what it could not tear down: a stealth
    /// rule left in place cuts the device off the network.
    pub async fn stop(self, app: &AppHandle) -> Option<CmdError> {
        self.watcher.abort();
        self.stats.abort();
        let teardown = match call::<StopAnswer>(app, "stop", json!({})).await {
            Ok(answer) => answer.error.map(|message| CmdError::new("teardown", message)),
            Err(err) => Some(err),
        };
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.tailer.abort();
        let _ = std::fs::remove_file(run_files(app).session);
        teardown
    }
}

/// Brings the tunnel up through the OS VPN service, the default path.
async fn start_vpn_service(
    app: &AppHandle,
    server: &Server,
    settings: &Settings,
    name: &str,
    metrics: &str,
    log: &LogSink,
) -> CmdResult<Vec<String>> {
    let engine_run = prepare_engine(app, server, settings).await?;
    let runtime = Runtime {
        tun: Some(Tun::External { name: name.to_owned() }),
        mixed_port: None,
        metrics_port: None,
        metrics_socket: Some(metrics.to_owned()),
        log: log.clone(),
        engine: engine_run.as_ref().map(|run| run.runtime.clone()),
    };
    let mut built = config::build(server, settings, &runtime)?;
    apply_mobile_optimizations(&mut built.config);
    // Xray's built-in DNS resolves the servers over the configured DoH, straight
    // out the physical interface, so a resolver on the path cannot substitute
    // them. Go's own resolver still needs a plain IP; a public one when the
    // direct DNS is a DoH URL (the usual case).
    let dns_endpoint = if let Ok(ip) = settings.direct_dns.parse::<IpAddr>() {
        if ip.is_ipv6() { format!("[{ip}]:53") } else { format!("{ip}:53") }
    } else if let Ok(addr) = settings.direct_dns.parse::<std::net::SocketAddr>() {
        addr.to_string()
    } else {
        format!("{FALLBACK_DNS}:53")
    };
    let args = StartArgs {
        config: built.config.to_string(),
        per_app_mode: settings.per_app.mode,
        packages: settings.per_app.packages.clone(),
        dns: dns_endpoint,
        mtu: config::MOBILE_TUN_MTU,
        routes: config::tunnel_routes(runtime.engine.as_ref()),
        session_name: name.to_owned(),
        server_name: server.name.clone(),
        server_id: server.id.clone(),
        metrics_socket: metrics.to_owned(),
        proxy_tags: built.proxy_tags.clone(),
        keep_awake: settings.keep_awake,
        // Xray runs inside the app: the engine accepts the app's own uid,
        // which the service vouches for over a socket only it and the engine
        // know (the name is random, the abstract namespace is shared).
        engine: EngineArgs::new(
            engine_run.as_ref(),
            &Peers::Uid { uid: own_uid(), socket: Some(format!("@uniray-{}", &engine::secret()[..24])) },
            false,
        ),
    };
    call::<Value>(app, "start", args).await?;
    Ok(built.proxy_tags)
}

/// Brings the tunnel up without a VpnService, as a separate root process. The
/// plugin rejects with `noRoot` when the device has no root to run it with.
#[cfg(not(play_store))]
async fn start_stealth(
    app: &AppHandle,
    server: &Server,
    settings: &Settings,
    name: &str,
    metrics: &str,
    log: &LogSink,
) -> CmdResult<Vec<String>> {
    // tproxy is the primary path (no interface); the root TUN is the fallback
    // for kernels without TPROXY, where Xray makes its own interface as root
    // and the service routes into it.
    let tun = if settings.stealth_root_tun {
        Tun::Routed { name: name.to_owned() }
    } else {
        Tun::Transparent { port: STEALTH_TPROXY_PORT }
    };
    let engine_run = prepare_engine(app, server, settings).await?;
    let runtime = Runtime {
        tun: Some(tun),
        mixed_port: None,
        metrics_port: None,
        metrics_socket: Some(metrics.to_owned()),
        log: log.clone(),
        engine: engine_run.as_ref().map(|run| run.runtime.clone()),
    };
    let mut built = config::build(server, settings, &runtime)?;
    apply_mobile_optimizations(&mut built.config);
    // Xray runs as root, so the engine does too, accepting root's connections
    // (from /proc/net, which root reads). The setup script starts it and the
    // teardown kills it, so it must not stop with the script's stdin.
    let engine = EngineArgs::new(engine_run.as_ref(), &Peers::Uid { uid: 0, socket: None }, true);
    let args = json!({
        "config": built.config.to_string(),
        "mode": if settings.stealth_root_tun { "roottun" } else { "tproxy" },
        "port": STEALTH_TPROXY_PORT,
        "tunName": name,
        "ipv6": settings.ipv6,
        "serverName": server.name,
        "serverId": server.id,
        "metricsSocket": metrics,
        "proxyTags": built.proxy_tags,
        "keepAwake": settings.keep_awake,
        "engineMode": engine.engine_mode,
        "engineConfig": engine.engine_config,
        "enginePort": engine.engine_port,
    });
    call::<Value>(app, "startStealth", args).await?;
    Ok(built.proxy_tags)
}

#[derive(Deserialize)]
struct ProbeAnswer {
    delay: i64,
}

/// One engine delay probe, see [`super::probe_engines`]: the service side
/// runs it, since only it knows where the engine was extracted.
pub(super) async fn run_engine_probe(app: &AppHandle, run: &EngineRun, probe_url: &str) -> i64 {
    let args = json!({
        "mode": run.mode,
        "config": String::from_utf8_lossy(&run.config_for(&Peers::Pids(Vec::new()), false)),
        "url": probe_url,
        "timeout": PROBE_TIMEOUT.as_secs(),
    });
    match call::<ProbeAnswer>(app, "probeEngine", args).await {
        Ok(answer) => answer.delay,
        Err(err) => {
            logs::push(app, &format!("[Warning] delay test failed: {}", err.message));
            -1
        }
    }
}

/// The app's uid: Xray's in VPN mode, where it runs inside the app.
fn own_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// What elevated access the device offers, for the settings UI.
pub async fn privilege_state(app: &AppHandle) -> CmdResult<Value> {
    call::<Value>(app, "privilegeState", json!({})).await
}

/// Asks Shizuku for permission (interactive) and returns the new state.
pub async fn request_shizuku(app: &AppHandle) -> CmdResult<Value> {
    call::<Value>(app, "requestShizuku", json!({})).await
}

/// This device's IPv4 address in the local network (Wi-Fi or Ethernet),
/// for the TV's import page. Asked of Android: an outgoing socket's address
/// would be the tunnel's while connected.
pub async fn lan_address(app: &AppHandle) -> CmdResult<IpAddr> {
    let answer = call::<Value>(app, "lanAddress", json!({})).await?;
    answer["address"]
        .as_str()
        .and_then(|address| address.parse().ok())
        .ok_or_else(|| CmdError::new("noLan", "not connected to a Wi-Fi or Ethernet network"))
}

/// Applies Settings::keep_awake to the running tunnel, if any, at once; a
/// tunnel started later reads it from its start arguments. When turning it
/// on, also asks Android to exempt the app from battery optimization (a
/// system dialog), without which Doze still cuts its network off. Returns
/// whether the app is exempt.
pub async fn set_keep_awake(app: &AppHandle, enabled: bool) -> CmdResult<Value> {
    call::<Value>(app, "setKeepAwake", json!({ "enabled": enabled })).await
}

/// Notices when the service stops on its own: the user revoked the VPN,
/// another VPN app took over, or the notification's disconnect was tapped.
///
/// A change of underlying network (Wi-Fi to mobile data and back) needs no
/// reconnect: the config's DNS no longer depends on the network, and Xray
/// re-dials its own protected sockets onto whatever network is current. Not
/// reconnecting also means less conspicuous churn.
fn spawn_watcher(app: AppHandle, generation: u64) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            let visible = app.state::<App>().visible.load(std::sync::atomic::Ordering::Relaxed);
            let interval = if visible { WATCH_INTERVAL } else { Duration::from_secs(15) };
            tokio::time::sleep(interval).await;
            let end = match service_state(&app).await {
                Some(state) if state.running => continue,
                Some(ServiceState { reported: true, error: Some(reason), .. }) => RunEnd::Failed(reason),
                // Stopped on request from outside the app: the notification.
                Some(ServiceState { reported: true, error: None, .. }) => RunEnd::Stopped,
                _ => RunEnd::Crashed,
            };
            // Not awaited: the handler aborts this very task.
            let app = app.clone();
            tauri::async_runtime::spawn(async move { super::on_run_end(&app, generation, end).await });
            return;
        }
    })
}

/// Runs once at startup: learns the device identity and, when the VPN service
/// is already up (the activity was recreated under it), adopts that connection.
pub async fn init_device(app: &AppHandle) {
    let state = app.state::<App>();
    if let Ok(info) = call::<DeviceInfo>(app, "deviceInfo", json!({})).await {
        let mut device = state.device.lock().unwrap();
        if !info.device_id.is_empty() {
            device.hwid = derive_hwid(&info.device_id);
        }
        device.model = info.model;
        device.os_version = info.os_version;
        state.tv.store(info.tv, std::sync::atomic::Ordering::Relaxed);
    }

    let files = run_files(app);
    let session = std::fs::read(&files.session)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok());
    if !service_running(app).await {
        let _ = std::fs::remove_file(&files.session);
        emit_state(app);
        return;
    }
    let mut running = state.running.lock().await;
    match session {
        Some(session) if running.is_none() => {
            let generation = state.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            *running = Some(Running {
                log_mark: logs::mark(),
                stats: stats::spawn(app.clone(), stats::MetricsSource::Socket(session.metrics_socket), session.proxy_tags),
                watcher: spawn_watcher(app.clone(), generation),
                tailer: logs::tail(app.clone(), vec![files.error_log, files.access_log]),
            });
            let mode = state.store.lock().unwrap().data.settings.mode;
            *state.connection.lock().unwrap() = ConnectionInfo {
                status: Status::Connected,
                server_id: Some(session.server_id),
                mode,
                since: Some(session.since),
                ..Default::default()
            };
        }
        Some(_) => {}
        // A service we know nothing about cannot be shown or controlled.
        None => drop(call::<Value>(app, "stop", json!({})).await),
    }
    drop(running);
    emit_state(app);
}

/// Tells the service which server carries the traffic and how the last check
/// went; it measures speed itself. Sent only when something changed.
pub async fn update_notification(app: &AppHandle, summary: &crate::monitor::Summary) {
    static LAST: std::sync::Mutex<Option<(String, Option<u64>, Option<bool>)>> = std::sync::Mutex::new(None);
    let current = (summary.server.clone(), summary.latency_ms, summary.ok);
    if LAST.lock().unwrap().as_ref() == Some(&current) {
        return;
    }
    let args = json!({ "server": current.0, "latencyMs": current.1, "ok": current.2 });
    if call::<Value>(app, "updateNotification", args).await.is_ok() {
        *LAST.lock().unwrap() = Some(current);
    }
}

/// The system's Material You palette as UI color tokens, `{ light, dark }`;
/// an empty object where the system has none (before Android 12).
pub async fn system_colors(app: &AppHandle) -> CmdResult<Value> {
    call::<Value>(app, "systemColors", json!({})).await
}

pub async fn list_apps(app: &AppHandle) -> CmdResult<Vec<InstalledApp>> {
    Ok(call::<AppList>(app, "listApps", json!({})).await?.apps)
}

pub async fn measure_delays(
    app: &AppHandle,
    servers: Vec<Server>,
    on_result: impl Fn(&str, i64) + Sync,
) -> CmdResult<()> {
    let settings = app.state::<App>().store.lock().unwrap().data.settings.clone();
    let is_disconnected = app.state::<App>().connection.lock().unwrap().status == Status::Disconnected;

    if settings.delay_test_mode == uniray_core::model::DelayTestMode::Tcp {
        const BATCH_SIZE: usize = 8;
        for batch in servers.chunks(BATCH_SIZE) {
            futures_util::future::join_all(batch.iter().map(|server| async {
                let delay = super::tcp_rtt(&server.address, server.port, super::PROBE_TIMEOUT).await;
                on_result(&server.id, delay);
            }))
            .await;
        }
        return Ok(());
    }

    let (servers, engine_servers): (Vec<Server>, Vec<Server>) =
        servers.into_iter().partition(|server| server.source.is_xray());

    if is_disconnected {
        super::probe_engines(app, &engine_servers, &on_result).await;

        let mut configs = Vec::new();
        let mut valid_servers = Vec::new();
        for server in &servers {
            let runtime = Runtime::default();
            if let Ok(built) = config::build(server, &settings, &runtime) {
                configs.push(built.config.to_string());
                valid_servers.push(server);
            }
        }

        #[derive(Deserialize)]
        struct DelayResponse {
            delays: Vec<i64>,
        }

        let batch_result = call::<DelayResponse>(
            app,
            "measureDelays",
            json!({
                "configs": configs,
                "url": settings.probe_url,
                "timeout": super::PROBE_TIMEOUT.as_secs(),
            }),
        )
        .await;

        match batch_result {
            Ok(resp) => {
                for (server, delay) in valid_servers.iter().zip(resp.delays) {
                    on_result(&server.id, delay);
                }
            }
            Err(_) => {
                for server in &servers {
                    let delay = super::tcp_rtt(&server.address, server.port, super::PROBE_TIMEOUT).await;
                    on_result(&server.id, delay);
                }
            }
        }
    } else {
        for server in &engine_servers {
            on_result(&server.id, -1);
        }
        for server in &servers {
            let delay = super::tcp_rtt(&server.address, server.port, super::PROBE_TIMEOUT).await;
            on_result(&server.id, delay);
        }
    }
    Ok(())
}

