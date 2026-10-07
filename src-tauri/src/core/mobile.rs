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
use uniray_core::hwid::derive_hwid;
use uniray_core::model::{PerAppMode, Server, Settings};

use super::{ConnectionInfo, RunEnd, Status, PROBE_TIMEOUT, PROBE_URL};
use crate::app::{emit_state, App};
use crate::network::NetworkInfo;
use crate::error::{CmdError, CmdResult};
use crate::state::write_private;
use crate::{logs, stats};

/// libXray tests at most this many configs per call.
const PROBE_BATCH: usize = 5;
/// Plain resolver for the core's own (Go-side) lookups, which do not resolve
/// the proxy server - Xray's built-in DNS does that over DoH - so this only
/// needs to be reachable. A public one, never the operator's.
const FALLBACK_DNS: &str = "8.8.8.8";
/// The loopback port the stealth tproxy inbound listens on; the iptables
/// TPROXY target points here. Matches the port in the generated config.
const STEALTH_TPROXY_PORT: u16 = 12345;
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Label shown for the VPN in Android's system settings.
    session_name: String,
    /// What the service needs to keep its notification current by itself
    /// once the app is gone: the server, and where to read the counters.
    server_name: String,
    metrics_socket: String,
    proxy_tags: Vec<String>,
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

async fn network_info(app: &AppHandle) -> Option<NetworkInfo> {
    call::<NetworkInfo>(app, "networkInfo", json!({})).await.ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_id: String,
    model: String,
    os_version: String,
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

#[derive(Deserialize)]
struct Delays {
    delays: Vec<i64>,
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
        let files = run_files(app);
        for log in [&files.error_log, &files.access_log] {
            write_private(log, b"").map_err(CmdError::io)?;
        }
        // A stale socket file would make the core fail to listen.
        let _ = std::fs::remove_file(&files.metrics);
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
    let runtime = Runtime {
        tun: Some(Tun::External { name: name.to_owned() }),
        mixed_port: None,
        metrics_port: None,
        metrics_socket: Some(metrics.to_owned()),
        log: log.clone(),
    };
    let built = config::build(server, settings, &runtime)?;
    // Xray's built-in DNS resolves the servers over the configured DoH, straight
    // out the physical interface, so a resolver on the path cannot substitute
    // them. Go's own resolver still needs a plain IP; a public one when the
    // direct DNS is a DoH URL (the usual case).
    let dns = if settings.direct_dns.parse::<IpAddr>().is_ok() { settings.direct_dns.as_str() } else { FALLBACK_DNS };
    let args = StartArgs {
        config: built.config.to_string(),
        per_app_mode: settings.per_app.mode,
        packages: settings.per_app.packages.clone(),
        dns: format!("{dns}:53"),
        mtu: config::TUN_MTU,
        session_name: name.to_owned(),
        server_name: server.name.clone(),
        metrics_socket: metrics.to_owned(),
        proxy_tags: built.proxy_tags.clone(),
    };
    call::<Value>(app, "start", args).await?;
    Ok(built.proxy_tags)
}

/// Brings the tunnel up without a VpnService, as a separate root process. The
/// plugin rejects with `noRoot` when the device has no root to run it with.
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
    let runtime = Runtime {
        tun: Some(tun),
        mixed_port: None,
        metrics_port: None,
        metrics_socket: Some(metrics.to_owned()),
        log: log.clone(),
    };
    let built = config::build(server, settings, &runtime)?;
    let args = json!({
        "config": built.config.to_string(),
        "mode": if settings.stealth_root_tun { "roottun" } else { "tproxy" },
        "port": STEALTH_TPROXY_PORT,
        "tunName": name,
        "ipv6": settings.ipv6,
        "serverName": server.name,
        "metricsSocket": metrics,
        "proxyTags": built.proxy_tags,
    });
    call::<Value>(app, "startStealth", args).await?;
    Ok(built.proxy_tags)
}

/// What elevated access the device offers, for the settings UI.
pub async fn privilege_state(app: &AppHandle) -> CmdResult<Value> {
    call::<Value>(app, "privilegeState", json!({})).await
}

/// Asks Shizuku for permission (interactive) and returns the new state.
pub async fn request_shizuku(app: &AppHandle) -> CmdResult<Value> {
    call::<Value>(app, "requestShizuku", json!({})).await
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
            tokio::time::sleep(WATCH_INTERVAL).await;
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

/// Measures the delay of each server with libXray's `pingBatch`. It shares
/// the process-wide core, so it is refused while a connection is up.
pub async fn measure_delays(
    app: &AppHandle,
    servers: Vec<Server>,
    on_result: impl Fn(&str, i64) + Sync,
) -> CmdResult<()> {
    if app.state::<App>().connection.lock().unwrap().status != Status::Disconnected {
        return Err(CmdError::new("delayWhileConnected", "disconnect to test delays"));
    }
    for batch in servers.chunks(PROBE_BATCH) {
        let configs: Vec<String> = batch
            .iter()
            .map(|server| {
                config::probe_outbound(server, config::TAG_PROXY)
                    .map(|outbound| json!({ "outbounds": [outbound] }).to_string())
                    .unwrap_or_default()
            })
            .collect();
        let args = json!({ "configs": configs, "url": PROBE_URL, "timeout": PROBE_TIMEOUT.as_secs() });
        match call::<Delays>(app, "measureDelays", args).await {
            Ok(result) => {
                for (index, server) in batch.iter().enumerate() {
                    on_result(&server.id, result.delays.get(index).copied().unwrap_or(-1));
                }
            }
            Err(err) if err.code == "delayWhileConnected" => return Err(err),
            Err(err) => {
                logs::push(app, &format!("[Warning] delay test failed: {}", err.message));
                for server in batch {
                    on_result(&server.id, -1);
                }
            }
        }
    }
    Ok(())
}

