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

use super::{free_port, ConnectionInfo, Status};
use crate::app::{emit_state, App};
use crate::network::{direct_dns, needs_reconnect, NetworkInfo};
use crate::error::{CmdError, CmdResult};
use crate::state::write_private;
use crate::{logs, stats};

const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
const PROBE_TIMEOUT_SECS: u32 = 6;
/// libXray tests at most this many configs per call.
const PROBE_BATCH: usize = 5;
/// DNS server for the core's own lookups when the configured one is not an IP.
const FALLBACK_DNS: &str = "8.8.8.8";
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// How often, in watch intervals, the network is checked for a change.
const NETWORK_CHECK_TICKS: u32 = 3;

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
}

#[derive(Deserialize)]
struct ServiceState {
    running: bool,
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
    metrics_port: u16,
    proxy_tags: Vec<String>,
}

struct RunFiles {
    error_log: PathBuf,
    access_log: PathBuf,
    session: PathBuf,
}

fn run_files(app: &AppHandle) -> RunFiles {
    let dir = &app.state::<App>().paths.run_dir;
    RunFiles {
        error_log: dir.join("error.log"),
        access_log: dir.join("access.log"),
        session: dir.join("session.json"),
    }
}

async fn service_running(app: &AppHandle) -> bool {
    call::<ServiceState>(app, "state", json!({}))
        .await
        .is_ok_and(|state| state.running)
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
        let metrics_port = free_port()?;
        let name = {
            let name = settings.interface_name.trim();
            if name.is_empty() { config::DEFAULT_INTERFACE_NAME.to_owned() } else { name.to_owned() }
        };
        let runtime = Runtime {
            tun: Some(Tun::External { name: name.clone() }),
            mixed_port: None,
            metrics_port: Some(metrics_port),
            log: LogSink::Files {
                error: files.error_log.to_string_lossy().into_owned(),
                access: files.access_log.to_string_lossy().into_owned(),
            },
        };
        let network = network_info(app).await;
        let direct = direct_dns(settings, network.as_ref());
        if let Some(network) = &network {
            logs::push(app, &format!("[Info] network: {}, resolving servers with {direct}", network.kind));
        }
        let settings = &Settings { direct_dns: direct.clone(), ..settings.clone() };
        let built = config::build(server, settings, &runtime)?;

        // Go's own resolver needs a plain IP server.
        let dns = if direct.parse::<IpAddr>().is_ok() { direct.as_str() } else { FALLBACK_DNS };
        let args = StartArgs {
            config: built.config.to_string(),
            per_app_mode: settings.per_app.mode,
            packages: settings.per_app.packages.clone(),
            dns: format!("{dns}:53"),
            mtu: config::TUN_MTU,
            session_name: name,
        };

        let log_mark = logs::mark();
        let tailer = logs::tail(app.clone(), vec![files.error_log, files.access_log]);
        if let Err(err) = call::<Value>(app, "start", args).await {
            // Give the tailer a moment to pick up what the core logged.
            tokio::time::sleep(Duration::from_millis(600)).await;
            tailer.abort();
            return Err(err);
        }

        let session = Session {
            server_id: server.id.clone(),
            since: logs::now_secs(),
            metrics_port,
            proxy_tags: built.proxy_tags.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec(&session) {
            let _ = write_private(&files.session, &bytes);
        }

        Ok(Self {
            log_mark,
            stats: stats::spawn(app.clone(), metrics_port, built.proxy_tags),
            watcher: spawn_watcher(app.clone(), generation, network),
            tailer,
        })
    }

    pub async fn stop(self, app: &AppHandle) {
        self.watcher.abort();
        self.stats.abort();
        let _ = call::<Value>(app, "stop", json!({})).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.tailer.abort();
        let _ = std::fs::remove_file(run_files(app).session);
    }
}

/// Notices when the service stops on its own (the user revoked the VPN,
/// another VPN app took over, the notification's disconnect was tapped) and
/// when the network changes in a way that needs another DNS server.
fn spawn_watcher(app: AppHandle, generation: u64, network: Option<NetworkInfo>) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut tick = 0u32;
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if !service_running(&app).await {
                // Not awaited: the handler aborts this very task.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { super::on_core_exit(&app, generation).await });
                return;
            }
            tick += 1;
            if tick % NETWORK_CHECK_TICKS != 0 {
                continue;
            }
            if let (Some(before), Some(now)) = (&network, network_info(&app).await) {
                if needs_reconnect(before, &now) {
                    logs::push(&app, &format!("[Info] network changed to {}, reconnecting", now.kind));
                    // Not awaited: reconnecting aborts this very task.
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = super::connect(&app).await;
                    });
                    return;
                }
            }
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
                stats: stats::spawn(app.clone(), session.metrics_port, session.proxy_tags),
                watcher: spawn_watcher(app.clone(), generation, network_info(app).await),
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

/// Shows server, speed and latency in the VPN notification.
pub async fn update_notification(app: &AppHandle, summary: &crate::monitor::Summary) {
    let _ = call::<Value>(app, "updateNotification", summary).await;
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
        let args = json!({ "configs": configs, "url": PROBE_URL, "timeout": PROBE_TIMEOUT_SECS });
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

