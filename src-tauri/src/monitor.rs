//! Watching a live connection: a reachability check through the tunnel
//! (on demand and every few minutes), and a one-line summary of server, speed
//! and latency for the system UI (Android notification, desktop tray).

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};
use uniray_core::model::Settings;

use crate::app::{emit_state, App};
use crate::core::Status;
use crate::error::{CmdError, CmdResult};
use crate::logs::{self, now_secs};

/// Answers with the client's IP and country as seen from the outside, in
/// plain text: a check of the tunnel and of where it comes out.
const TRACE_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
/// How the check's connection shows up in the core's access log.
const TRACE_HOST: &str = "www.cloudflare.com:443";
const EXIT_LOG_WAIT: Duration = Duration::from_millis(1500);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the latency is measured again while connected.
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How often the notification / tray summary is refreshed.
const PRESENT_INTERVAL: Duration = Duration::from_secs(5);
/// The first check waits for the core to settle (balancers probe first).
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub ok: bool,
    /// Time of one HTTPS request through the tunnel, connection setup included.
    pub latency_ms: Option<u64>,
    /// Exit address and its country, as the outside world sees them.
    pub ip: Option<String>,
    pub country: Option<String>,
    /// The server the check went out through when the connected one is a
    /// balancer group: the group's pick, with its real name.
    pub server: Option<String>,
    pub error: Option<String>,
    /// Unix seconds.
    pub at: i64,
}

/// `socks5h://` URL of the local proxy, with its credentials when it has them.
pub fn local_proxy_url(settings: &Settings, port: u16) -> String {
    if settings.proxy_auth {
        format!(
            "socks5h://{}:{}@127.0.0.1:{port}",
            encode_userinfo(&settings.proxy_user),
            encode_userinfo(&settings.proxy_password)
        )
    } else {
        format!("socks5h://127.0.0.1:{port}")
    }
}

fn encode_userinfo(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// A client whose requests go through the tunnel. On desktop that is the
/// local proxy; on mobile the app's own traffic is routed into the VPN.
fn tunnel_client(app: &AppHandle) -> CmdResult<reqwest::Client> {
    tunnel_client_builder(app)?.timeout(CHECK_TIMEOUT).build().map_err(|err| CmdError::core(err.to_string()))
}

/// [`tunnel_client`]'s builder, for callers with their own timeouts (the
/// speed test).
pub(crate) fn tunnel_client_builder(app: &AppHandle) -> CmdResult<reqwest::ClientBuilder> {
    let state = app.state::<App>();
    let mut builder = reqwest::Client::builder();
    if cfg!(desktop) {
        let port = state
            .connection
            .lock()
            .unwrap()
            .mixed_port
            .ok_or_else(|| CmdError::new("notConnected", "not connected"))?;
        let settings = state.store.lock().unwrap().data.settings.clone();
        let proxy = reqwest::Proxy::all(local_proxy_url(&settings, port)).map_err(|err| CmdError::core(err.to_string()))?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
    }
    Ok(builder)
}

async fn run_check(app: &AppHandle) -> CheckResult {
    let failed = |error: String| CheckResult {
        ok: false,
        latency_ms: None,
        ip: None,
        country: None,
        server: None,
        error: Some(error),
        at: now_secs(),
    };
    let client = match tunnel_client(app) {
        Ok(client) => client,
        Err(err) => return failed(err.message),
    };
    let mark = logs::mark();
    let started = Instant::now();
    let response = match client.get(TRACE_URL).send().await {
        Ok(response) => response,
        Err(err) => return failed(crate::subscriptions::describe(&err)),
    };
    let latency = started.elapsed().as_millis() as u64;
    // Through a TUN the core logs the address, not the name.
    let mut targets = vec![TRACE_HOST.to_owned()];
    if let Some(peer) = response.remote_addr() {
        // `1.2.3.4:443` or `[2606:4700::6810:7c60]:443`, as the log writes it.
        targets.push(peer.to_string());
    }
    if !response.status().is_success() {
        return failed(format!("HTTP {}", response.status().as_u16()));
    }
    let body = response.text().await.unwrap_or_default();
    let field = |name: &str| {
        body.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::to_owned)
    };
    CheckResult {
        ok: true,
        latency_ms: Some(latency),
        ip: field("ip"),
        country: field("loc"),
        server: exit_server(app, mark, &targets).await,
        error: None,
        at: now_secs(),
    }
}

/// Which server of a balancer group carried the check: the core's access log
/// names the outbound it picked, and the subscription has a server with the
/// same endpoint and a real name.
async fn exit_server(app: &AppHandle, mark: u64, targets: &[String]) -> Option<String> {
    // The log line may trail the response a little (files are tailed).
    let deadline = Instant::now() + EXIT_LOG_WAIT;
    let tag = loop {
        let found = {
            let state = app.state::<App>();
            let logs = state.logs.lock().unwrap();
            logs.iter()
                .rev()
                .take_while(|line| line.id >= mark)
                .find_map(|line| logs::access_outbound(&line.text, targets))
        };
        if found.is_some() || Instant::now() >= deadline {
            break found?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let state = app.state::<App>();
    let connected = state.connection.lock().unwrap().server_id.clone()?;
    let store = state.store.lock().unwrap();
    let servers = &store.data.servers;
    let connected = servers.iter().find(|server| server.id == connected)?;
    uniray_core::config::exit_server(servers, connected, &tag).map(|server| server.name.clone())
}

/// Checks the connection now and records the result.
pub async fn check_now(app: &AppHandle) -> CmdResult<CheckResult> {
    let status = app.state::<App>().connection.lock().unwrap().status;
    if status != Status::Connected {
        return Err(CmdError::new("notConnected", "not connected"));
    }
    let result = run_check(app).await;
    {
        let state = app.state::<App>();
        let mut connection = state.connection.lock().unwrap();
        // The connection may have gone or changed while the check ran.
        if connection.status == Status::Connected {
            connection.check = Some(result.clone());
        }
    }
    emit_state(app);
    Ok(result)
}

/// Starts watching the current connection; replaces a previous watcher.
pub fn start(app: &AppHandle) {
    let checker = {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_CHECK_DELAY).await;
            loop {
                #[cfg(mobile)]
                let visible = app.state::<App>().visible.load(std::sync::atomic::Ordering::Relaxed);
                #[cfg(not(mobile))]
                let visible = true;
                if visible {
                    let _ = check_now(&app).await;
                }
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        })
    };
    let presenter = {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                #[cfg(mobile)]
                let visible = app.state::<App>().visible.load(std::sync::atomic::Ordering::Relaxed);
                #[cfg(not(mobile))]
                let visible = true;
                let interval = if visible { PRESENT_INTERVAL } else { Duration::from_secs(15) };
                if visible {
                    present(&app, summary(&app)).await;
                }
                tokio::time::sleep(interval).await;
            }
        })
    };
    let previous = app.state::<App>().monitor.lock().unwrap().replace((checker, presenter));
    abort(previous);
}

/// Stops watching and puts the system UI back to its idle state.
pub fn stop(app: &AppHandle) {
    let previous = app.state::<App>().monitor.lock().unwrap().take();
    abort(previous);
    *app.state::<App>().traffic.lock().unwrap() = Default::default();
    #[cfg(desktop)]
    crate::tray::set_summary(app, None);
}

fn abort(handles: Option<(JoinHandle<()>, JoinHandle<()>)>) {
    if let Some((checker, presenter)) = handles {
        checker.abort();
        presenter.abort();
    }
}

/// What the system UI shows about a live connection.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub server: String,
    /// Bytes per second.
    pub down: u64,
    pub up: u64,
    /// Latency of the last check; `None` before the first one.
    pub latency_ms: Option<u64>,
    /// Whether the last check went through; `None` before the first one.
    pub ok: Option<bool>,
}

fn summary(app: &AppHandle) -> Summary {
    let state = app.state::<App>();
    let connection = state.connection.lock().unwrap().clone();
    let traffic = *state.traffic.lock().unwrap();
    let server = connection
        .server_id
        .and_then(|id| {
            let store = state.store.lock().unwrap();
            store.data.servers.iter().find(|server| server.id == id).map(|server| server.name.clone())
        })
        .unwrap_or_default();
    // A balancer's pick, when the last check found it.
    let server = connection.check.as_ref().and_then(|check| check.server.clone()).unwrap_or(server);
    Summary {
        server,
        down: traffic.down_speed,
        up: traffic.up_speed,
        latency_ms: connection.check.as_ref().and_then(|check| check.latency_ms),
        ok: connection.check.as_ref().map(|check| check.ok),
    }
}

async fn present(app: &AppHandle, summary: Summary) {
    #[cfg(desktop)]
    crate::tray::set_summary(app, Some(&summary));
    #[cfg(mobile)]
    crate::core::update_notification(app, &summary).await;
}

/// `1.2 MB/s`, for the tray tooltip.
#[cfg(desktop)]
pub fn format_speed(bytes_per_second: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes_per_second as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    format!("{value:.digits$} {}/s", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_carries_encoded_credentials() {
        let mut settings = Settings::default();
        assert_eq!(local_proxy_url(&settings, 10808), "socks5h://127.0.0.1:10808");
        settings.proxy_auth = true;
        settings.proxy_user = "me".into();
        settings.proxy_password = "p@ss:w/rd".into();
        assert_eq!(local_proxy_url(&settings, 10808), "socks5h://me:p%40ss%3Aw%2Frd@127.0.0.1:10808");
    }

    #[test]
    fn speeds_read_naturally() {
        assert_eq!(format_speed(0), "0 B/s");
        assert_eq!(format_speed(1536), "1.5 KB/s");
        assert_eq!(format_speed(3 * 1024 * 1024), "3.0 MB/s");
        assert_eq!(format_speed(250 * 1024 * 1024), "250 MB/s");
    }
}
