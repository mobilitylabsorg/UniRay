//! Commands invoked by the frontend. State changes reach it as `state` events,
//! so commands return only what is specific to the call.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};
use uniray_core::model::{Server, Settings};
use uniray_core::subscription;

use crate::app::{emit_state, snapshot, App, Snapshot};
use crate::core;
use crate::error::{CmdError, CmdResult};
use crate::logs::LogLine;
use crate::subscriptions::{self, ImportReport};

#[tauri::command]
pub fn get_state(app: AppHandle) -> Snapshot {
    snapshot(&app)
}

/// Imports pasted text: a subscription URL, share links, a JSON config, or
/// base64 of those.
#[tauri::command]
pub async fn import(app: AppHandle, text: String) -> CmdResult<ImportReport> {
    let text = text.trim();
    let is_url = text.starts_with("https://") || text.starts_with("http://");
    if is_url && !text.contains(char::is_whitespace) {
        return subscriptions::add(&app, text).await;
    }

    let (servers, errors) = subscription::parse_body(text);
    if servers.is_empty() {
        let reason = errors.into_iter().next().unwrap_or_else(|| "no servers found in the text".into());
        return Err(CmdError::new("invalidLink", reason));
    }
    let report = ImportReport { added: servers.len(), errors };
    app.state::<App>()
        .update(|store| {
            if store.data.settings.selected_server.is_none() {
                store.data.settings.selected_server = servers.first().map(|server| server.id.clone());
            }
            store.data.servers.extend(servers);
        })
        .map_err(CmdError::io)?;
    emit_state(&app);
    Ok(report)
}

#[tauri::command]
pub async fn delete_servers(app: AppHandle, ids: Vec<String>) -> CmdResult<()> {
    let state = app.state::<App>();
    let active = state.connection.lock().unwrap().server_id.clone();
    if active.is_some_and(|id| ids.contains(&id)) {
        core::disconnect(&app).await;
    }
    state
        .update(|store| {
            store.data.servers.retain(|server| !ids.contains(&server.id));
            if store.data.settings.selected_server.as_ref().is_some_and(|id| ids.contains(id)) {
                store.data.settings.selected_server = None;
            }
        })
        .map_err(CmdError::io)?;
    emit_state(&app);
    Ok(())
}

/// Selects the server to connect to; a live connection switches to it.
#[tauri::command]
pub async fn select_server(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<App>();
    let found = state
        .update(|store| {
            let found = store.data.servers.iter().any(|server| server.id == id);
            if found {
                store.data.settings.selected_server = Some(id.clone());
            }
            found
        })
        .map_err(CmdError::io)?;
    if !found {
        return Err(CmdError::new("notFound", "no such server"));
    }
    emit_state(&app);
    let active = state.connection.lock().unwrap().server_id.clone();
    if active.as_deref() != Some(id.as_str()) {
        core::reconnect_if_connected(&app).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn add_subscription(app: AppHandle, url: String) -> CmdResult<ImportReport> {
    subscriptions::add(&app, &url).await
}

#[tauri::command]
pub async fn update_subscription(app: AppHandle, id: String) -> CmdResult<ImportReport> {
    subscriptions::update(&app, &id).await
}

#[tauri::command]
pub async fn delete_subscription(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<App>();
    let active_belongs = {
        let active = state.connection.lock().unwrap().server_id.clone();
        let store = state.store.lock().unwrap();
        active.is_some_and(|active| {
            store
                .data
                .servers
                .iter()
                .any(|server| server.id == active && server.subscription_id.as_deref() == Some(id.as_str()))
        })
    };
    if active_belongs {
        core::disconnect(&app).await;
    }
    subscriptions::remove(&app, &id).await
}

/// Saves the settings; a live connection is restarted when they affect it.
#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> CmdResult<()> {
    if settings.mixed_port == 0 {
        return Err(CmdError::new("invalidSettings", "the local port must be between 1 and 65535"));
    }
    for (what, value) in [("remote DNS", &settings.remote_dns), ("direct DNS", &settings.direct_dns)] {
        if value.trim().is_empty() {
            return Err(CmdError::new("invalidSettings", format!("{what} must not be empty")));
        }
    }
    let previous = app
        .state::<App>()
        .update(|store| {
            // The selection has its own command and may have moved meanwhile.
            let selected_server = store.data.settings.selected_server.clone();
            let next = Settings { selected_server, ..settings };
            std::mem::replace(&mut store.data.settings, next.clone())
        })
        .map_err(CmdError::io)?;
    emit_state(&app);

    let current = app.state::<App>().store.lock().unwrap().data.settings.clone();
    if affects_core(&previous) != affects_core(&current) {
        core::reconnect_if_connected(&app).await?;
    }
    Ok(())
}

/// The settings with everything that does not shape the core config blanked.
fn affects_core(settings: &Settings) -> Settings {
    Settings {
        send_hwid: false,
        locale: None,
        selected_server: None,
        // Applied to the running tunnel without a reconnect (set_keep_awake).
        keep_awake: false,
        ..settings.clone()
    }
}

#[tauri::command]
pub async fn connect(app: AppHandle) -> CmdResult<()> {
    core::connect(&app).await
}

#[tauri::command]
pub async fn disconnect(app: AppHandle) {
    core::disconnect(&app).await;
}

/// Measures the delay of the given servers, or of all of them. Results arrive
/// through `state` events as they come in.
#[tauri::command]
pub async fn test_delays(app: AppHandle, ids: Option<Vec<String>>) -> CmdResult<()> {
    let state = app.state::<App>();
    if state.testing.swap(true, Ordering::SeqCst) {
        return Err(CmdError::new("busy", "a delay test is already running"));
    }
    let servers = {
        let mut store = state.store.lock().unwrap();
        let wanted = |server: &Server| ids.as_ref().is_none_or(|ids| ids.contains(&server.id));
        for server in store.data.servers.iter_mut().filter(|server| wanted(server)) {
            server.delay_ms = None;
        }
        store.data.servers.iter().filter(|server| wanted(server)).cloned().collect::<Vec<_>>()
    };
    emit_state(&app);

    let measured = core::measure_delays(&app, servers, |id, delay| {
        {
            let mut store = state.store.lock().unwrap();
            if let Some(server) = store.data.servers.iter_mut().find(|server| server.id == id) {
                server.delay_ms = Some(delay);
            }
        }
        emit_state(&app);
    })
    .await;

    state.testing.store(false, Ordering::SeqCst);
    let saved = state.store.lock().unwrap().save();
    emit_state(&app);
    measured?;
    saved.map_err(CmdError::io)
}

/// The system color palette for the UI (Material You on Android 12+), or null
/// where the platform offers none.
#[cfg(mobile)]
#[tauri::command]
pub async fn system_colors(app: AppHandle) -> CmdResult<serde_json::Value> {
    core::system_colors(&app).await
}

#[cfg(desktop)]
#[tauri::command]
pub async fn system_colors() -> CmdResult<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

/// Installed apps, for the per-app proxy list (Android).
#[cfg(mobile)]
#[tauri::command]
pub async fn list_apps(app: AppHandle) -> CmdResult<Vec<core::InstalledApp>> {
    core::list_apps(&app).await
}

#[cfg(desktop)]
#[tauri::command]
pub async fn list_apps() -> CmdResult<Vec<()>> {
    Ok(Vec::new())
}

/// What elevated access the device offers, so the UI can show it and gate the
/// stealth option on root. Android only; elsewhere there is nothing to report.
#[cfg(mobile)]
#[tauri::command]
pub async fn privilege_state(app: AppHandle) -> CmdResult<serde_json::Value> {
    core::privilege_state(&app).await
}

#[cfg(desktop)]
#[tauri::command]
pub async fn privilege_state() -> CmdResult<serde_json::Value> {
    Ok(serde_json::json!({ "level": "none", "shizukuAvailable": false, "shizukuGranted": false }))
}

/// Asks Shizuku for permission (interactive) and returns the new state.
#[cfg(mobile)]
#[tauri::command]
pub async fn request_shizuku(app: AppHandle) -> CmdResult<serde_json::Value> {
    core::request_shizuku(&app).await
}

#[cfg(desktop)]
#[tauri::command]
pub async fn request_shizuku() -> CmdResult<serde_json::Value> {
    Ok(serde_json::json!({ "level": "none", "shizukuAvailable": false, "shizukuGranted": false }))
}

/// Starts the TV's import page (handoff.rs) and returns its URL, for the
/// QR code. Whatever is sent there is imported like pasted text; the
/// frontend hears of it through the `handoff` event.
#[cfg(mobile)]
#[tauri::command]
pub async fn handoff_start(app: AppHandle) -> CmdResult<String> {
    use tauri::Emitter;
    let ip = core::lan_address(&app).await?;
    let handle = app.clone();
    let handoff = crate::handoff::start(ip, move |text| {
        let report = tauri::async_runtime::block_on(import(handle.clone(), text)).map_err(|err| err.message)?;
        let added = report.added;
        let _ = handle.emit("handoff", report);
        Ok(added)
    })
    .map_err(|err| CmdError::new("handoff", err))?;
    let url = handoff.url.clone();
    *app.state::<App>().handoff.lock().unwrap() = Some(handoff);
    Ok(url)
}

#[cfg(desktop)]
#[tauri::command]
pub async fn handoff_start() -> CmdResult<String> {
    Err(CmdError::new("unsupported", "importing from a phone is for the TV app"))
}

/// Stops the TV's import page, if it is up.
#[tauri::command]
pub fn handoff_stop(app: AppHandle) {
    app.state::<App>().handoff.lock().unwrap().take();
}

/// Applies the "stay awake with the screen off" setting to the running
/// tunnel and, when turning it on, asks for the battery optimization
/// exemption. Android only.
#[cfg(mobile)]
#[tauri::command]
pub async fn set_keep_awake(app: AppHandle, enabled: bool) -> CmdResult<serde_json::Value> {
    core::set_keep_awake(&app, enabled).await
}

#[cfg(desktop)]
#[tauri::command]
pub async fn set_keep_awake(enabled: bool) -> CmdResult<serde_json::Value> {
    let _ = enabled;
    Ok(serde_json::json!({ "batteryExempt": true }))
}

/// Checks that traffic gets through the tunnel, and where it comes out.
#[tauri::command]
pub async fn check_connection(app: AppHandle) -> CmdResult<crate::monitor::CheckResult> {
    crate::monitor::check_now(&app).await
}

#[tauri::command]
pub fn get_logs(app: AppHandle) -> Vec<LogLine> {
    app.state::<App>().logs.lock().unwrap().iter().cloned().collect()
}

#[tauri::command]
pub fn clear_logs(app: AppHandle) {
    app.state::<App>().logs.lock().unwrap().clear();
}

/// Exits the app; the connection is torn down on the way out.
#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
