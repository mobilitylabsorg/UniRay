//! Commands invoked by the frontend. State changes reach it as `state` events,
//! so commands return only what is specific to the call.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uniray_core::model::{Server, Settings, Subscription};
use uniray_core::subscription;

use crate::app::{emit_state, snapshot, App, Snapshot};
use crate::core;
use crate::error::{CmdError, CmdResult};
use crate::logs::{now_secs, LogLine};
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
pub async fn update_all_subscriptions(app: AppHandle) -> CmdResult<Vec<ImportReport>> {
    subscriptions::update_all(&app).await
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
pub async fn save_settings(app: AppHandle, mut settings: Settings) -> CmdResult<()> {
    crate::edition::restrict(&mut settings);
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPayload {
    pub version: u32,
    pub exported_at: i64,
    pub settings: Settings,
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
}

#[tauri::command]
pub async fn export_backup(app: AppHandle) -> CmdResult<String> {
    let state = app.state::<App>();
    let store = state.store.lock().unwrap();
    let payload = BackupPayload {
        version: 1,
        exported_at: now_secs() as i64,
        settings: store.data.settings.clone(),
        servers: store.data.servers.clone(),
        subscriptions: store.data.subscriptions.clone(),
    };
    serde_json::to_string_pretty(&payload).map_err(|e| CmdError::new("io", e.to_string()))
}

#[tauri::command]
pub async fn import_backup(app: AppHandle, json: String, mode: String) -> CmdResult<()> {
    let payload: BackupPayload = serde_json::from_str(&json)
        .map_err(|e| CmdError::new("invalidConfig", format!("Invalid backup JSON: {e}")))?;

    let state = app.state::<App>();
    state
        .update(|store| {
            let mut settings = payload.settings;
            crate::edition::restrict(&mut settings);
            if mode == "replace" {
                store.data.settings = settings;
                store.data.servers = payload.servers;
                store.data.subscriptions = payload.subscriptions;
            } else {
                store.data.settings = settings;
                for server in payload.servers {
                    if !store
                        .data
                        .servers
                        .iter()
                        .any(|s| s.id == server.id || (s.address == server.address && s.port == server.port))
                    {
                        store.data.servers.push(server);
                    }
                }
                for sub in payload.subscriptions {
                    if !store.data.subscriptions.iter().any(|s| s.id == sub.id || s.url == sub.url) {
                        store.data.subscriptions.push(sub);
                    }
                }
            }
        })
        .map_err(CmdError::io)?;
    emit_state(&app);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceProbeResult {
    pub ok: bool,
    pub status: u16,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn probe_service(app: AppHandle, url: String) -> CmdResult<ServiceProbeResult> {
    let client = get_http_client(&app, false).await;
    let start = std::time::Instant::now();
    let res = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    let elapsed = start.elapsed().as_millis() as u64;

    match res {
        Ok(response) => {
            let status = response.status().as_u16();
            let ok = status < 400 || status == 401 || status == 403;
            Ok(ServiceProbeResult {
                ok,
                status,
                latency_ms: elapsed,
                error: if ok { None } else { Some(format!("HTTP {status}")) },
            })
        }
        Err(err) => {
            let err_str = err.to_string();
            let msg = if err.is_timeout() {
                "Timeout".into()
            } else if err_str.contains("reset") || err_str.contains("ConnectionReset") {
                "DPI / Connection reset".into()
            } else {
                err_str
            };
            Ok(ServiceProbeResult {
                ok: false,
                status: 0,
                latency_ms: elapsed,
                error: Some(msg),
            })
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestResult {
    pub bytes: usize,
    pub elapsed_ms: u64,
    pub mbps: f64,
}

#[tauri::command]
pub async fn speed_test_download(app: AppHandle, bytes: usize) -> CmdResult<SpeedTestResult> {
    let client = get_http_client(&app, false).await;
    let url = format!("https://speed.cloudflare.com/__down?bytes={bytes}");
    let start = std::time::Instant::now();
    let res = client
        .get(&url)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| CmdError::network(e.to_string()))?;

    let body = res
        .bytes()
        .await
        .map_err(|e| CmdError::network(e.to_string()))?;
    let elapsed = start.elapsed().as_millis().max(1) as u64;
    let downloaded = body.len();
    let mbps = (downloaded as f64 * 8.0) / (elapsed as f64 / 1000.0) / 1_000_000.0;

    Ok(SpeedTestResult {
        bytes: downloaded,
        elapsed_ms: elapsed,
        mbps,
    })
}

#[tauri::command]
pub async fn speed_test_upload(app: AppHandle, bytes: usize) -> CmdResult<SpeedTestResult> {
    let client = get_http_client(&app, false).await;
    let url = "https://speed.cloudflare.com/__up";
    let data = vec![0u8; bytes];
    let start = std::time::Instant::now();
    let _res = client
        .post(url)
        .body(data)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| CmdError::network(e.to_string()))?;

    let elapsed = start.elapsed().as_millis().max(1) as u64;
    let mbps = (bytes as f64 * 8.0) / (elapsed as f64 / 1000.0) / 1_000_000.0;

    Ok(SpeedTestResult {
        bytes,
        elapsed_ms: elapsed,
        mbps,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestTick {
    pub phase: String,
    pub mbps: f64,
    pub fraction: f32,
}

struct RateSmoother {
    times: std::collections::VecDeque<std::time::Instant>,
    totals: std::collections::VecDeque<u64>,
    blended: f64,
    span: Duration,
    alpha: f64,
}

impl RateSmoother {
    fn new(span: Duration, alpha: f64) -> Self {
        Self {
            times: std::collections::VecDeque::new(),
            totals: std::collections::VecDeque::new(),
            blended: f64::NAN,
            span,
            alpha,
        }
    }

    fn observe(&mut self, now: std::time::Instant, total: u64) -> f64 {
        self.times.push_back(now);
        self.totals.push_back(total);
        while self.times.len() > 2 && now.duration_since(self.times[1]) >= self.span {
            self.times.pop_front();
            self.totals.pop_front();
        }
        let dt = now.duration_since(*self.times.front().unwrap()).as_secs_f64();
        let bytes = total.saturating_sub(*self.totals.front().unwrap());
        let raw = if dt > 0.0 { (bytes as f64 * 8.0) / dt / 1_000_000.0 } else { 0.0 };
        self.blended = if self.blended.is_nan() {
            raw
        } else {
            self.blended + self.alpha * (raw - self.blended)
        };
        self.blended
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestLatencyResult {
    pub ping_ms: Option<u32>,
    pub jitter_ms: Option<u32>,
}

/// The client of the speed test and the service checks: through the tunnel,
/// or past it when `bypass_tunnel` (the direct run, for comparison).
async fn get_http_client(app: &AppHandle, bypass_tunnel: bool) -> reqwest::Client {
    let state = app.state::<App>();
    if !bypass_tunnel {
        // On desktop `state.http` goes direct, so in proxy mode it would not
        // measure the VPN at all: the core's local proxy while connected.
        let tunnel = crate::monitor::tunnel_client_builder(app).ok().and_then(|builder| {
            builder.connect_timeout(Duration::from_secs(10)).build().ok()
        });
        return tunnel.unwrap_or_else(|| state.http.clone());
    }
    #[cfg(desktop)]
    if let Some(client) = tauri::async_runtime::spawn_blocking(direct_client).await.ok().flatten() {
        return client;
    }
    #[cfg(mobile)]
    {
        if let Some(info) = crate::core::network_info(app).await {
            if let Some(addr_str) = info.address {
                if !addr_str.is_empty() {
                    if let Ok(ip) = addr_str.parse::<std::net::IpAddr>() {
                        if let Ok(client) = reqwest::Client::builder()
                            .local_address(ip)
                            .timeout(Duration::from_secs(10))
                            .build()
                        {
                            return client;
                        }
                    }
                }
            }
        }
    }
    state.http.clone()
}

/// Desktop: a client bound to the physical interface, so that it leaves past
/// the TUN as well (an unprivileged SO_BINDTODEVICE on Linux, IP_BOUND_IF on
/// macOS; on Windows a source address on that interface does it).
#[cfg(desktop)]
fn direct_client() -> Option<reqwest::Client> {
    let uplink = crate::uplink::detect()?;
    let builder = reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(10));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let builder = builder.interface(&uplink.name);
    #[cfg(windows)]
    let builder = builder.local_address(std::net::IpAddr::V4(uplink.address?));
    builder.build().ok()
}

#[tauri::command]
pub async fn speed_test_probe_latency(
    app: AppHandle,
    url: String,
    samples: Option<usize>,
    bypass_tunnel: Option<bool>,
) -> CmdResult<SpeedTestLatencyResult> {
    let client = get_http_client(&app, bypass_tunnel.unwrap_or(false)).await;
    let count = samples.unwrap_or(6);
    let mut timings = Vec::new();

    for i in 0..count {
        let start = std::time::Instant::now();
        let res = client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
            .header("Accept-Encoding", "identity")
            .timeout(Duration::from_millis(3000))
            .send()
            .await;
        if let Ok(r) = res {
            if r.status().is_success() {
                let ms = start.elapsed().as_millis().max(1) as u32;
                // Discard cold start first sample (matches Lean-main)
                if i > 0 {
                    timings.push(ms);
                }
            }
        }
    }

    if timings.is_empty() {
        return Ok(SpeedTestLatencyResult {
            ping_ms: None,
            jitter_ms: None,
        });
    }

    let min_ping = *timings.iter().min().unwrap();
    let jitter = if timings.len() < 2 {
        0
    } else {
        let diff_sum: u32 = timings.windows(2).map(|w| (w[0] as i32 - w[1] as i32).abs() as u32).sum();
        diff_sum / (timings.len() - 1) as u32
    };

    Ok(SpeedTestLatencyResult {
        ping_ms: Some(min_ping),
        jitter_ms: Some(jitter),
    })
}

#[tauri::command]
pub async fn get_underlying_network(app: AppHandle) -> CmdResult<crate::network::NetworkInfo> {
    #[cfg(mobile)]
    {
        if let Some(info) = crate::core::network_info(&app).await {
            return Ok(info);
        }
        Ok(crate::network::NetworkInfo { kind: "other".to_string(), dns: vec![], address: None })
    }
    // Desktop: the interface the default route leaves by (not our TUN) and
    // the DNS servers the system has from that network.
    #[cfg(desktop)]
    {
        let _ = app;
        let uplink = tauri::async_runtime::spawn_blocking(crate::uplink::detect).await.ok().flatten();
        Ok(crate::network::NetworkInfo {
            kind: uplink.as_ref().map_or("none", |uplink| uplink.kind).to_string(),
            dns: crate::core::system_dns().await,
            address: uplink.and_then(|uplink| uplink.address).map(|address| address.to_string()),
        })
    }
}

#[tauri::command]
pub async fn speed_test_multistream_download(
    app: AppHandle,
    duration_sec: Option<u64>,
    url: Option<String>,
    bypass_tunnel: Option<bool>,
) -> CmdResult<SpeedTestResult> {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tauri::Emitter;

    let target_url = url.unwrap_or_else(|| "https://speed.cloudflare.com/__down?bytes=50000000".to_string());
    let dur_sec = duration_sec.unwrap_or(8).max(3);
    let deadline_dur = Duration::from_secs(dur_sec);
    let start_time = std::time::Instant::now();
    let ramp_dur = Duration::from_millis(2000);

    let moved = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let client = get_http_client(&app, bypass_tunnel.unwrap_or(false)).await;

    let mut workers = Vec::new();
    for _ in 0..4 {
        let client_clone = client.clone();
        let url_clone = target_url.clone();
        let moved_clone = moved.clone();
        let stop_clone = stop.clone();
        workers.push(tokio::spawn(async move {
            while !stop_clone.load(Ordering::Relaxed) {
                let res = match client_clone
                    .get(&url_clone)
                    .header("User-Agent", "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
                    .header("Accept-Encoding", "identity")
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => r,
                    _ => break,
                };
                let mut res = res;
                while !stop_clone.load(Ordering::Relaxed) {
                    match res.chunk().await {
                        Ok(Some(chunk)) => {
                            moved_clone.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                        }
                        _ => break,
                    }
                }
            }
        }));
    }

    let mut smoother = RateSmoother::new(Duration::from_millis(1200), 0.45);
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut ramp_bytes = 0u64;
    let mut ramp_marked = false;

    while start_time.elapsed() < deadline_dur {
        interval.tick().await;
        let now = std::time::Instant::now();
        let current_bytes = moved.load(Ordering::Relaxed);
        if !ramp_marked && start_time.elapsed() >= ramp_dur {
            ramp_bytes = current_bytes;
            ramp_marked = true;
        }
        let smoothed = smoother.observe(now, current_bytes);
        let fraction = (start_time.elapsed().as_secs_f32() / deadline_dur.as_secs_f32()).min(1.0);
        let _ = app.emit("speedtest-tick", SpeedTestTick {
            phase: "download".to_string(),
            mbps: (smoothed * 100.0).round() / 100.0,
            fraction,
        });
    }

    stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.await;
    }

    let total = moved.load(Ordering::Relaxed);
    let elapsed = start_time.elapsed().as_millis().max(1) as u64;
    let (final_bytes, final_secs) = if ramp_marked && total > ramp_bytes {
        (total - ramp_bytes, start_time.elapsed().saturating_sub(ramp_dur).as_secs_f64().max(0.1))
    } else {
        (total, elapsed as f64 / 1000.0)
    };
    let mbps = (final_bytes as f64 * 8.0) / final_secs / 1_000_000.0;

    Ok(SpeedTestResult {
        bytes: total as usize,
        elapsed_ms: elapsed,
        mbps: (mbps * 100.0).round() / 100.0,
    })
}

#[tauri::command]
pub async fn speed_test_multistream_upload(
    app: AppHandle,
    duration_sec: Option<u64>,
    url: Option<String>,
    bypass_tunnel: Option<bool>,
) -> CmdResult<SpeedTestResult> {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tauri::Emitter;

    let target_url = url.unwrap_or_else(|| "https://speed.cloudflare.com/__up".to_string());
    let dur_sec = duration_sec.unwrap_or(8).max(3);
    let deadline_dur = Duration::from_secs(dur_sec);
    let start_time = std::time::Instant::now();
    let ramp_dur = Duration::from_millis(2000);

    let moved = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let client = get_http_client(&app, bypass_tunnel.unwrap_or(false)).await;

    let mut workers = Vec::new();
    let payload = vec![0x37u8; 1_048_576]; // 1MB payload per POST
    for _ in 0..4 {
        let client_clone = client.clone();
        let url_clone = target_url.clone();
        let moved_clone = moved.clone();
        let stop_clone = stop.clone();
        let data = payload.clone();
        workers.push(tokio::spawn(async move {
            let data_len = data.len() as u64;
            while !stop_clone.load(Ordering::Relaxed) {
                let res = client_clone
                    .post(&url_clone)
                    .header("User-Agent", "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36")
                    .header("Content-Type", "application/octet-stream")
                    .body(data.clone())
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await;
                match res {
                    Ok(r) if r.status().is_success() => {
                        moved_clone.fetch_add(data_len, Ordering::Relaxed);
                    }
                    _ => break,
                }
            }
        }));
    }

    let mut smoother = RateSmoother::new(Duration::from_millis(1200), 0.45);
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut ramp_bytes = 0u64;
    let mut ramp_marked = false;

    while start_time.elapsed() < deadline_dur {
        interval.tick().await;
        let now = std::time::Instant::now();
        let current_bytes = moved.load(Ordering::Relaxed);
        if !ramp_marked && start_time.elapsed() >= ramp_dur {
            ramp_bytes = current_bytes;
            ramp_marked = true;
        }
        let smoothed = smoother.observe(now, current_bytes);
        let fraction = (start_time.elapsed().as_secs_f32() / deadline_dur.as_secs_f32()).min(1.0);
        let _ = app.emit("speedtest-tick", SpeedTestTick {
            phase: "upload".to_string(),
            mbps: (smoothed * 100.0).round() / 100.0,
            fraction,
        });
    }

    stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.await;
    }

    let total = moved.load(Ordering::Relaxed);
    let elapsed = start_time.elapsed().as_millis().max(1) as u64;
    let (final_bytes, final_secs) = if ramp_marked && total > ramp_bytes {
        (total - ramp_bytes, start_time.elapsed().saturating_sub(ramp_dur).as_secs_f64().max(0.1))
    } else {
        (total, elapsed as f64 / 1000.0)
    };
    let mbps = (final_bytes as f64 * 8.0) / final_secs / 1_000_000.0;

    Ok(SpeedTestResult {
        bytes: total as usize,
        elapsed_ms: elapsed,
        mbps: (mbps * 100.0).round() / 100.0,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiFinding {
    pub id: String,
    pub title: String,
    pub verdict: String,
    pub detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiReport {
    pub findings: Vec<DpiFinding>,
    pub can_apply: bool,
}

#[tauri::command]
pub async fn run_dpi_probe(app: AppHandle) -> CmdResult<DpiReport> {
    let state = app.state::<App>();
    let mut findings = Vec::new();

    // 1. TCP control check to 1.1.1.1:443
    let tcp_start = std::time::Instant::now();
    let tcp_res = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::TcpStream::connect("1.1.1.1:443"),
    ).await;
    let tcp_ok = match tcp_res {
        Ok(Ok(_)) => {
            let ms = tcp_start.elapsed().as_millis().max(1);
            findings.push(DpiFinding {
                id: "tcp".into(),
                title: "Выход в сеть (TCP 443)".into(),
                verdict: "clear".into(),
                detail: format!("TCP 443 открывается за {ms} мс"),
            });
            true
        }
        _ => {
            findings.push(DpiFinding {
                id: "tcp".into(),
                title: "Выход в сеть (TCP 443)".into(),
                verdict: "blocked".into(),
                detail: "Порт 443 не открывается — в сети нет доступа к интернету".into(),
            });
            false
        }
    };

    // 2. TLS SNI test with blocked domain rutracker.org
    if tcp_ok {
        let tcp_sni = tokio::time::timeout(
            Duration::from_secs(4),
            tokio::net::TcpStream::connect("rutracker.org:443"),
        ).await;
        let tls_sni = state
            .http
            .get("https://rutracker.org")
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match (tcp_sni, tls_sni) {
            (Ok(Ok(_)), Ok(_)) => {
                findings.push(DpiFinding {
                    id: "tls_sni".into(),
                    title: "Фильтр по имени сайта (SNI)".into(),
                    verdict: "clear".into(),
                    detail: "Рукопожатие TLS с rutracker.org проходит без блокировки".into(),
                });
            }
            (Ok(Ok(_)), Err(_)) => {
                findings.push(DpiFinding {
                    id: "tls_sni".into(),
                    title: "Фильтр по имени сайта (SNI)".into(),
                    verdict: "blocked".into(),
                    detail: "Порт открыт, но TLS с именем rutracker.org сбрасывают — фильтрация SNI (DPI)".into(),
                });
            }
            _ => {
                findings.push(DpiFinding {
                    id: "tls_sni".into(),
                    title: "Фильтр по имени сайта (SNI)".into(),
                    verdict: "unknown".into(),
                    detail: "Адрес rutracker.org недоступен целиком — судить по SNI нельзя".into(),
                });
            }
        }
    } else {
        findings.push(DpiFinding {
            id: "tls_sni".into(),
            title: "Фильтр по имени сайта (SNI)".into(),
            verdict: "unknown".into(),
            detail: "Нет TCP-связи — проверка пропущена".into(),
        });
    }

    // 3. UDP outbound probe
    let udp_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await;
    let udp_ok = if let Ok(s) = udp_socket {
        let query: [u8; 28] = [
            0x4c, 0x4e, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x06, b'g', b'o', b'o', b'g', b'l', b'e', 0x03, b'c', b'o', b'm', 0x00,
            0x00, 0x01, 0x00, 0x01,
        ];
        let send_res = s.send_to(&query, "8.8.8.8:53").await;
        if send_res.is_ok() {
            let mut buf = [0u8; 512];
            tokio::time::timeout(Duration::from_secs(3), s.recv_from(&mut buf)).await.is_ok()
        } else {
            false
        }
    } else {
        false
    };

    if udp_ok {
        findings.push(DpiFinding {
            id: "udp".into(),
            title: "UDP наружу".into(),
            verdict: "clear".into(),
            detail: "UDP работает — Hysteria 2, TUIC и WireGuard доступны".into(),
        });
    } else {
        findings.push(DpiFinding {
            id: "udp".into(),
            title: "UDP наружу".into(),
            verdict: "blocked".into(),
            detail: "UDP до 8.8.8.8 не проходит — Hysteria 2, TUIC и WireGuard заблокированы оператором".into(),
        });
    }

    // 4. DNS tampering probe
    let sys_lookup = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host("rutracker.org:443"),
    ).await;
    match sys_lookup {
        Ok(Ok(mut addrs)) => {
            let first = addrs.next().map(|a| a.ip().to_string());
            if let Some(ip) = first {
                if ip.starts_with("127.") || ip == "0.0.0.0" {
                    findings.push(DpiFinding {
                        id: "dns".into(),
                        title: "Подмена DNS".into(),
                        verdict: "blocked".into(),
                        detail: format!("Системный DNS подменяет адрес rutracker.org на {ip}"),
                    });
                } else {
                    findings.push(DpiFinding {
                        id: "dns".into(),
                        title: "Подмена DNS".into(),
                        verdict: "clear".into(),
                        detail: "Системный DNS возвращает валидный адрес".into(),
                    });
                }
            } else {
                findings.push(DpiFinding {
                    id: "dns".into(),
                    title: "Подмена DNS".into(),
                    verdict: "blocked".into(),
                    detail: "Системный резолвер не отдаёт адрес для rutracker.org".into(),
                });
            }
        }
        _ => {
            findings.push(DpiFinding {
                id: "dns".into(),
                title: "Подмена DNS".into(),
                verdict: "blocked".into(),
                detail: "Системный DNS не отвечает или блокирует запросы к rutracker.org".into(),
            });
        }
    }

    // 5. Sustained TSPU data flow
    let sustained_start = std::time::Instant::now();
    let sustained_res = state
        .http
        .get("https://speed.cloudflare.com/__down?bytes=262144")
        .timeout(Duration::from_secs(6))
        .send()
        .await;

    match sustained_res {
        Ok(res) if res.status().is_success() => {
            match res.bytes().await {
                Ok(body) if body.len() >= 262144 => {
                    let ms = sustained_start.elapsed().as_millis().max(1);
                    findings.push(DpiFinding {
                        id: "sustained".into(),
                        title: "Обрыв на объёме (ТСПУ)".into(),
                        verdict: "clear".into(),
                        detail: format!("256 КБ переданы целиком за {ms} мс, обрыва потока нет"),
                    });
                }
                Ok(body) => {
                    let kb = body.len() / 1024;
                    findings.push(DpiFinding {
                        id: "sustained".into(),
                        title: "Обрыв на объёме (ТСПУ)".into(),
                        verdict: "blocked".into(),
                        detail: format!("Соединение оборвано после {kb} КБ — ТСПУ глушит трафик по объёму"),
                    });
                }
                Err(_) => {
                    findings.push(DpiFinding {
                        id: "sustained".into(),
                        title: "Обрыв на объёме (ТСПУ)".into(),
                        verdict: "blocked".into(),
                        detail: "Соединение разорвано в процессе передачи данных — ТСПУ глушит поток".into(),
                    });
                }
            }
        }
        _ => {
            findings.push(DpiFinding {
                id: "sustained".into(),
                title: "Обрыв на объёме (ТСПУ)".into(),
                verdict: "unknown".into(),
                detail: "Загрузка не началась — судить об объёме нельзя".into(),
            });
        }
    }

    let can_apply = findings.iter().any(|f| {
        f.verdict == "blocked" && (f.id == "tls_sni" || f.id == "sustained" || f.id == "dns")
    });

    Ok(DpiReport { findings, can_apply })
}

#[tauri::command]
pub async fn apply_dpi_fixes(app: AppHandle) -> CmdResult<()> {
    let state = app.state::<App>();
    state
        .update(|store| {
            store.data.settings.tls_fragment = true;
            if store.data.settings.tls_fingerprint.is_empty()
                || store.data.settings.tls_fingerprint == "auto"
                || store.data.settings.tls_fingerprint == "none"
            {
                store.data.settings.tls_fingerprint = "chrome".to_string();
            }
            if !store.data.settings.remote_dns.starts_with("https://") {
                store.data.settings.remote_dns = "https://1.1.1.1/dns-query".to_string();
            }
        })
        .map_err(CmdError::io)?;
    emit_state(&app);
    Ok(())
}

/// Exits the app; the connection is torn down on the way out.
#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
