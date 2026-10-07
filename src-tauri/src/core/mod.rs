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
    init_device, list_apps, measure_delays, plugin, privilege_state, request_shizuku, system_colors,
    update_notification, InstalledApp, Running,
};

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use uniray_core::model::Mode;

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

/// A loopback port that is free right now.
pub(crate) fn free_port() -> CmdResult<u16> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(CmdError::io)
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
        previous.stop(app).await;
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
    if let Some(running) = running.take() {
        set_connection(app, |info| info.status = Status::Disconnecting);
        running.stop(app).await;
    }
    set_connection(app, |info| *info = ConnectionInfo::default());
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

/// Called when the core of run `generation` stopped without being asked to.
pub(crate) async fn on_core_exit(app: &AppHandle, generation: u64) {
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
    stopped.stop(app).await;
    let reason = logs::last_problem(&state.logs.lock().unwrap(), log_mark)
        .unwrap_or_else(|| "the core stopped unexpectedly".into());
    set_connection(app, |info| {
        *info = ConnectionInfo { error: Some(CmdError::core(reason)), ..Default::default() }
    });
}
