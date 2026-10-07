//! Traffic counters, polled from the core's metrics endpoint.
//!
//! Xray serves expvar JSON on `metrics.listen`; the counters of the proxying
//! outbounds live under `stats.outbound.<tag>`. This works the same for a
//! child process and for the in-process core on mobile.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};

use crate::app::App;

pub const EVENT: &str = "traffic";
const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Traffic {
    /// Bytes sent and received through the proxy since connecting.
    pub up: u64,
    pub down: u64,
    /// Current rates in bytes per second.
    pub up_speed: u64,
    pub down_speed: u64,
}

/// Where the core serves its counters.
#[derive(Clone, Debug)]
pub enum MetricsSource {
    /// A loopback port (desktop).
    Port(u16),
    /// A Unix socket in the app's private directory (Android): no port for
    /// other apps to find.
    #[cfg(mobile)]
    Socket(std::path::PathBuf),
}

pub fn spawn(app: AppHandle, source: MetricsSource, proxy_tags: Vec<String>) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let http = app.state::<App>().http.clone();
        let mut previous: Option<(Instant, u64, u64)> = None;
        // A one-shot note in the app log: whether the counters are reachable.
        // Speed that stays zero usually means the socket does not answer or the
        // outbound tags do not match where the traffic is counted.
        let mut diagnosed = false;
        let mut misses = 0u32;
        loop {
            tokio::time::sleep(INTERVAL).await;
            let Some(body) = read_vars(&http, &source).await else {
                misses += 1;
                if misses == 5 && !diagnosed {
                    crate::logs::push(&app, "[Warning] traffic counters: the metrics endpoint is not answering");
                    diagnosed = true;
                }
                continue;
            };
            let Ok(vars) = serde_json::from_slice::<Value>(&body) else { continue };
            let (up, down) = totals(&vars, &proxy_tags);
            if !diagnosed {
                crate::logs::push(
                    &app,
                    &format!("[Info] traffic counters active: up={up} down={down} over {} tag(s)", proxy_tags.len()),
                );
                diagnosed = true;
            }
            let now = Instant::now();
            let (up_speed, down_speed) = match previous {
                Some((then, last_up, last_down)) => {
                    let seconds = now.duration_since(then).as_secs_f64().max(0.001);
                    (
                        (up.saturating_sub(last_up) as f64 / seconds) as u64,
                        (down.saturating_sub(last_down) as f64 / seconds) as u64,
                    )
                }
                None => (0, 0),
            };
            previous = Some((now, up, down));
            let traffic = Traffic { up, down, up_speed, down_speed };
            *app.state::<App>().traffic.lock().unwrap() = traffic;
            let _ = app.emit(EVENT, traffic);
        }
    })
}

/// The body of `GET /debug/vars`.
async fn read_vars(http: &reqwest::Client, source: &MetricsSource) -> Option<Vec<u8>> {
    match source {
        MetricsSource::Port(port) => {
            let response = http.get(format!("http://127.0.0.1:{port}/debug/vars")).send().await.ok()?;
            Some(response.bytes().await.ok()?.to_vec())
        }
        #[cfg(mobile)]
        MetricsSource::Socket(path) => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let timeout = std::time::Duration::from_secs(2);
            let mut stream = tokio::time::timeout(timeout, tokio::net::UnixStream::connect(path)).await.ok()?.ok()?;
            stream.write_all(b"GET /debug/vars HTTP/1.0\r\nHost: localhost\r\n\r\n").await.ok()?;
            let mut response = Vec::new();
            tokio::time::timeout(timeout, stream.read_to_end(&mut response)).await.ok()?.ok()?;
            let body = response.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
            Some(response[body..].to_vec())
        }
    }
}

/// Sums uplink and downlink over the given outbound tags.
fn totals(vars: &Value, tags: &[String]) -> (u64, u64) {
    let outbounds = &vars["stats"]["outbound"];
    tags.iter().fold((0, 0), |(up, down), tag| {
        let counters = &outbounds[tag.as_str()];
        (
            up + counters["uplink"].as_u64().unwrap_or(0),
            down + counters["downlink"].as_u64().unwrap_or(0),
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn sums_proxy_outbounds_only() {
        let vars = json!({
            "stats": {
                "inbound": { "mixed": { "uplink": 1, "downlink": 2 } },
                "outbound": {
                    "proxy": { "uplink": 100, "downlink": 2000 },
                    "out-1": { "uplink": 5, "downlink": 7 },
                    "direct": { "uplink": 9000, "downlink": 9000 },
                },
            },
        });
        assert_eq!(totals(&vars, &["proxy".into(), "out-1".into(), "missing".into()]), (105, 2007));
        // Before any traffic the section is absent altogether.
        assert_eq!(totals(&json!({}), &["proxy".into()]), (0, 0));
    }
}
