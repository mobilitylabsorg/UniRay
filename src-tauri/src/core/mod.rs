//! Connection lifecycle. The platform module knows how to run the core
//! (a child process on desktop, the VPN service on mobile); this one keeps the
//! status the UI shows consistent with it.

#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
mod elevated;
#[cfg(mobile)]
mod mobile;

#[cfg(desktop)]
pub use desktop::{measure_delays, Running};
#[cfg(mobile)]
pub use mobile::{
    init_device, lan_address, list_apps, measure_delays, network_info, plugin, privilege_state, request_shizuku, set_keep_awake, system_colors,
    update_notification, InstalledApp, Running,
};

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use uniray_core::engine::{self, EngineRun};
use uniray_core::model::{Mode, Server, Settings};

use crate::app::{emit_state, App};
use crate::error::{CmdError, CmdResult};
use crate::logs;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub status: Status,
    pub server_id: Option<String>,
    pub mode: Mode,
    /// Unix seconds when the connection was established.
    pub since: Option<i64>,
    /// Why the last connection attempt failed or the connection dropped.
    pub error: Option<CmdError>,
    /// Port of the local SOCKS/HTTP inbound while connected.
    pub mixed_port: Option<u16>,
    /// The last reachability check of this connection.
    pub check: Option<crate::monitor::CheckResult>,
}

/// Delay tests fetch this through each server. Shared by every platform so that
/// the same server reads the same everywhere.
#[allow(dead_code)]
pub(crate) const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
/// How long one server gets before its delay test counts as a timeout.
pub(crate) const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Measures true TCP handshake round-trip time (RTT) to `host:port`.
/// Completing the TCP 3-way handshake (SYN -> SYN/ACK -> ACK) gives the exact network latency
/// to the server port and proves that the port is not blocked by censorship/firewall.
pub async fn tcp_rtt(host: &str, port: u16, timeout: std::time::Duration) -> i64 {
    let target = format!("{host}:{port}");
    let started = std::time::Instant::now();
    let connect = async {
        let addrs = tokio::net::lookup_host(&target).await.ok()?;
        for addr in addrs {
            if let Ok(_stream) = tokio::net::TcpStream::connect(addr).await {
                return Some(started.elapsed().as_millis() as i64);
            }
        }
        None
    };
    tokio::time::timeout(timeout, connect)
        .await
        .ok()
        .flatten()
        .unwrap_or(-1)
}

/// A loopback port that is free right now.
pub(crate) fn free_port() -> CmdResult<u16> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(CmdError::io)
}

/// `settings` with the network's own DNS server as the direct DNS, when the
/// user asked for it ([`Settings::direct_dns_from_network`]) and the network
/// names one; as they are otherwise.
pub(crate) async fn with_provider_dns(app: &AppHandle, settings: &Settings) -> Settings {
    let mut settings = settings.clone();
    if !settings.direct_dns_from_network {
        return settings;
    }
    #[cfg(desktop)]
    let servers = desktop::system_dns().await;
    #[cfg(mobile)]
    let servers = mobile::system_dns(app).await;
    match crate::network::provider_dns(&servers) {
        // One routing rule per DNS host sends its queries direct or through
        // the proxy; the same host for both would send the VPN's queries
        // around it.
        Some(ip) if settings.remote_dns.contains(&ip.to_string()) => logs::push(
            app,
            &format!("[Warning] direct DNS: the network's ({ip}) is also the DNS through the VPN; using {}", settings.direct_dns),
        ),
        Some(ip) => {
            logs::push(app, &format!("[Info] direct DNS: the network's own, {ip}"));
            settings.direct_dns = ip.to_string();
        }
        None => logs::push(
            app,
            &format!("[Warning] direct DNS: the network names no DNS server; using {}", settings.direct_dns),
        ),
    }
    settings
}

/// Engine servers probed at once: each is a process with a tunnel of its own.
const ENGINE_PROBES_AT_ONCE: usize = 4;

/// The delays of servers Xray does not run (WireGuard, AmneziaWG, Naive):
/// for each, the engine fetches [`PROBE_URL`] through the server itself and
/// reports the time, serving nothing meanwhile.
pub(crate) async fn probe_engines(app: &AppHandle, servers: &[Server], on_result: &(impl Fn(&str, i64) + Sync)) {
    let settings = app.state::<App>().store.lock().unwrap().data.settings.clone();
    for batch in servers.chunks(ENGINE_PROBES_AT_ONCE) {
        futures_util::future::join_all(batch.iter().map(|server| async {
            let delay = match prepare_engine(app, server, &settings).await {
                Ok(Some(run)) => run_engine_probe(app, &run, &settings.probe_url).await,
                Ok(None) => -1,
                Err(err) => {
                    logs::push(app, &format!("[Warning] delay test of {}: {}", server.name, err.message));
                    -1
                }
            };
            on_result(&server.id, delay);
        }))
        .await;
    }
}

/// The delay an engine probe printed (`delay <ms>`), -1 without one.
#[cfg(desktop)]
pub(crate) fn parse_probe(stdout: &str) -> i64 {
    stdout.lines().find_map(|line| line.strip_prefix("delay ")?.trim().parse().ok()).unwrap_or(-1)
}

#[cfg(desktop)]
use desktop::run_engine_probe;
#[cfg(mobile)]
use mobile::run_engine_probe;

/// The engine run for `server`, if Xray cannot run it (see
/// `uniray_core::engine`), on a free port. Resolving the server's name
/// blocks, so it happens off the async runtime.
pub(crate) async fn prepare_engine(app: &AppHandle, server: &Server, settings: &Settings) -> CmdResult<Option<EngineRun>> {
    if server.source.is_xray() {
        return Ok(None);
    }
    let (server, ipv6, port) = (server.clone(), settings.ipv6, free_port()?);
    // On Android the service fills it in: only it knows where the system
    // extracted the native libraries.
    #[cfg(desktop)]
    let naive = app.state::<App>().paths.naive.clone();
    #[cfg(mobile)]
    let naive = std::path::PathBuf::new();
    let run = tauri::async_runtime::spawn_blocking(move || engine::prepare(&server, port, ipv6, &naive))
        .await
        .map_err(|err| CmdError::core(err.to_string()))??;
    for warning in run.iter().flat_map(|run| &run.warnings) {
        logs::push(app, warning);
    }
    Ok(run)
}

fn set_connection(app: &AppHandle, update: impl FnOnce(&mut ConnectionInfo)) {
    update(&mut app.state::<App>().connection.lock().unwrap());
    emit_state(app);
}

/// Connects to the selected server, replacing any running connection.
pub async fn connect(app: &AppHandle) -> CmdResult<()> {
    let state = app.state::<App>();
    let mut running = state.running.lock().await;
    crate::monitor::stop(app);
    if let Some(previous) = running.take() {
        if let Some(err) = previous.stop(app).await {
            logs::push(app, &format!("[Error] {}", err.message));
        }
    }

    let (server, settings) = {
        let store = state.store.lock().unwrap();
        (store.selected_server().cloned(), store.data.settings.clone())
    };
    let Some(server) = server else {
        let err = CmdError::new("noServer", "no server selected");
        set_connection(app, |info| {
            *info = ConnectionInfo { error: Some(err.clone()), ..Default::default() }
        });
        return Err(err);
    };

    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    set_connection(app, |info| {
        *info = ConnectionInfo {
            status: Status::Connecting,
            server_id: Some(server.id.clone()),
            mode: settings.mode,
            ..Default::default()
        }
    });

    match Running::start(app, &server, &settings, generation).await {
        Ok(started) => {
            let mixed_port = started.mixed_port();
            *running = Some(started);
            set_connection(app, |info| {
                info.status = Status::Connected;
                info.since = Some(logs::now_secs());
                info.mixed_port = mixed_port;
            });
            crate::monitor::start(app);
            Ok(())
        }
        Err(err) => {
            set_connection(app, |info| {
                *info = ConnectionInfo { error: Some(err.clone()), ..Default::default() }
            });
            Err(err)
        }
    }
}

pub async fn disconnect(app: &AppHandle) {
    let state = app.state::<App>();
    let mut running = state.running.lock().await;
    crate::monitor::stop(app);
    let mut error = None;
    if let Some(running) = running.take() {
        set_connection(app, |info| info.status = Status::Disconnecting);
        error = running.stop(app).await;
    }
    // A teardown that left something behind is worth telling: on Android
    // stealth it can cut the device off the network.
    set_connection(app, |info| *info = ConnectionInfo { error, ..Default::default() });
}

/// Applies changed settings or a newly selected server to a live connection.
pub async fn reconnect_if_connected(app: &AppHandle) -> CmdResult<()> {
    let connected = app.state::<App>().connection.lock().unwrap().status == Status::Connected;
    if connected {
        connect(app).await
    } else {
        Ok(())
    }
}

/// How a run ended when the app did not end it.
#[cfg_attr(desktop, allow(dead_code))] // only the mobile service reports why
pub(crate) enum RunEnd {
    /// The core died; its log may say why.
    Crashed,
    /// The tunnel service was stopped from outside the app (the notification's
    /// disconnect): an ordinary disconnect.
    Stopped,
    /// The tunnel service stopped and said why.
    Failed(String),
}

/// Called when the core of run `generation` stopped without being asked to.
#[cfg(desktop)]
pub(crate) async fn on_core_exit(app: &AppHandle, generation: u64) {
    on_run_end(app, generation, RunEnd::Crashed).await
}

/// Called when run `generation` ended without the app ending it.
pub(crate) async fn on_run_end(app: &AppHandle, generation: u64, end: RunEnd) {
    let state = app.state::<App>();
    // Waits out a connect or disconnect in progress; whichever it is, it has
    // dealt with this run by the time the lock is free, unless the run is
    // still the installed one.
    let mut running = state.running.lock().await;
    if state.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let Some(stopped) = running.take() else {
        return;
    };
    let log_mark = stopped.log_mark;
    crate::monitor::stop(app);
    let teardown = stopped.stop(app).await;
    let error = match end {
        RunEnd::Stopped => teardown,
        RunEnd::Failed(reason) => Some(CmdError::new("coreStopped", reason)),
        RunEnd::Crashed => {
            let reason = logs::last_problem(&state.logs.lock().unwrap(), log_mark)
                .unwrap_or_else(|| "the core stopped unexpectedly".into());
            Some(CmdError::new("coreStopped", reason))
        }
    };
    set_connection(app, |info| *info = ConnectionInfo { error, ..Default::default() });
}

#[cfg(all(test, desktop))]
mod probe_tests {
    #[test]
    fn engine_probe_output_is_read() {
        assert_eq!(super::parse_probe("delay 42\n"), 42);
        assert_eq!(super::parse_probe("delay -1\n"), -1);
        assert_eq!(super::parse_probe(""), -1);
        assert_eq!(super::parse_probe("garbage\ndelay 7"), 7);
    }
}
