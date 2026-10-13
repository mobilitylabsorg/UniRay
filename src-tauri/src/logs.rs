//! Core log lines: a bounded buffer for the log view plus live events.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app::App;

pub const CAPACITY: usize = 2000;
pub const EVENT: &str = "log";

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub id: u64,
    /// Unix milliseconds.
    pub time: u64,
    /// `debug`, `info`, `warning`, `error` or `access`.
    pub level: &'static str,
    pub text: String,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Records a raw line printed by the core and forwards it to the UI.
pub fn push(app: &AppHandle, raw: &str) {
    let raw = raw.trim_end();
    // Our own counter reads, once a second: noise in the log.
    if raw.is_empty() || raw.contains(uniray_core::config::TAG_METRICS_IN) {
        return;
    }
    let (level, text) = parse(raw);
    let line = LogLine {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        time: now_millis(),
        level,
        text: text.to_owned(),
    };
    {
        let state = app.state::<App>();
        let mut logs = state.logs.lock().unwrap();
        if logs.len() == CAPACITY {
            logs.pop_front();
        }
        logs.push_back(line.clone());
    }
    let _ = app.emit(EVENT, line);
}

/// The outbound an access log line says a connection to one of `targets`
/// went out through: `from 127.0.0.1:5 accepted tcp:host:443 [mixed -> proxy-7]`
/// gives `proxy-7` (a balancer logs the server it picked, not itself).
/// Targets are `host:port` or `ip:port`: through a TUN the log names the IP.
pub fn access_outbound(text: &str, targets: &[String]) -> Option<String> {
    let (_, destination) = text.split_once(" accepted ")?;
    let destination = destination.split_whitespace().next()?;
    let destination = destination.split_once(':').map_or(destination, |(_, rest)| rest);
    if !targets.iter().any(|target| target == destination) {
        return None;
    }
    let route = text.rsplit_once('[')?.1.strip_suffix(']')?;
    let (_, outbound) = route.split_once(" -> ").or_else(|| route.split_once(" >> "))?;
    let outbound = outbound.trim();
    (!outbound.is_empty()).then(|| outbound.to_owned())
}

/// The id the next line will get; lines of a run starting now have ids from
/// it on.
pub fn mark() -> u64 {
    NEXT_ID.load(Ordering::Relaxed)
}

/// The most recent problem the core reported since `mark`, to explain why it
/// stopped.
pub fn last_problem(logs: &VecDeque<LogLine>, mark: u64) -> Option<String> {
    let recent = || logs.iter().rev().take_while(|line| line.id >= mark).take(30);
    recent()
        .find(|line| line.level == "error")
        .or_else(|| recent().find(|line| line.level == "warning"))
        // A connection the core accepted explains nothing about its end.
        .or_else(|| recent().find(|line| line.level != "access"))
        .map(|line| line.text.clone())
}

/// Splits an Xray log line into level and message, dropping the timestamp:
///
/// ```text
/// 2026/10/07 00:59:53.168212 [Warning] common/errors: ...
/// 2026/10/07 00:59:53.168212 from 127.0.0.1:53170 accepted tcp:example.com:443 [mixed -> proxy]
/// ```
pub fn parse(raw: &str) -> (&'static str, &str) {
    let text = strip_timestamp(raw);
    for (prefix, level) in [
        ("[Debug]", "debug"),
        ("[Info]", "info"),
        ("[Warning]", "warning"),
        ("[Error]", "error"),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return (level, rest.trim_start());
        }
    }
    if text.starts_with("from ") && text.contains(" accepted ") {
        ("access", text)
    } else if text.starts_with("Failed to start") || text.starts_with("uniray-helper:") || text.starts_with("Error ") {
        ("error", text)
    } else {
        ("info", text)
    }
}

fn strip_timestamp(raw: &str) -> &str {
    let mut parts = raw.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(date), Some(time), Some(rest))
            if date.len() == 10
                && date.bytes().all(|b| b.is_ascii_digit() || b == b'/')
                && time.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b'.') =>
        {
            rest
        }
        _ => raw,
    }
}

/// Follows log files of a core whose output we do not get on a pipe (the
/// in-process core on mobile, the elevated one on Windows and macOS), feeding
/// new lines to [`push`] until the returned task is aborted.
#[cfg(any(mobile, windows, target_os = "macos"))]
pub fn tail(app: AppHandle, paths: Vec<std::path::PathBuf>) -> tauri::async_runtime::JoinHandle<()> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    tauri::async_runtime::spawn(async move {
        let mut offsets = vec![0u64; paths.len()];
        let mut pending = vec![String::new(); paths.len()];
        loop {
            for (index, path) in paths.iter().enumerate() {
                let Ok(mut file) = tokio::fs::File::open(path).await else { continue };
                if file.seek(std::io::SeekFrom::Start(offsets[index])).await.is_err() {
                    continue;
                }
                let mut chunk = Vec::new();
                let Ok(read) = file.read_to_end(&mut chunk).await else { continue };
                offsets[index] += read as u64;
                pending[index].push_str(&String::from_utf8_lossy(&chunk));
                // Keep an unfinished last line for the next round.
                while let Some(end) = pending[index].find('\n') {
                    let line: String = pending[index].drain(..=end).collect();
                    push(&app, &line);
                }
            }
            let visible = app.state::<crate::app::App>().visible.load(std::sync::atomic::Ordering::Relaxed);
            let sleep_interval = if visible {
                if cfg!(mobile) {
                    std::time::Duration::from_millis(1000)
                } else {
                    std::time::Duration::from_millis(400)
                }
            } else {
                std::time::Duration::from_secs(10)
            };
            tokio::time::sleep(sleep_interval).await;
        }
    })
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_secs() -> i64 {
    (now_millis() / 1000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xray_lines() {
        assert_eq!(
            parse("2026/10/07 00:59:53.168212 [Warning] common/errors: deprecated"),
            ("warning", "common/errors: deprecated")
        );
        assert_eq!(
            parse("2026/10/07 00:59:53.168212 from 127.0.0.1:53170 accepted tcp:example.com:443 [mixed -> proxy]"),
            ("access", "from 127.0.0.1:53170 accepted tcp:example.com:443 [mixed -> proxy]")
        );
        assert_eq!(parse("Xray 26.9.30 (Xray, Penetrates Everything.)"), ("info", "Xray 26.9.30 (Xray, Penetrates Everything.)"));
        assert_eq!(parse("Failed to start: main: bad config").0, "error");
        assert_eq!(parse("uniray-helper: cannot start xray").0, "error");
    }

    #[test]
    fn access_lines_name_the_outbound() {
        let targets = ["www.cloudflare.com:443".to_owned(), "104.16.124.96:443".to_owned()];
        let outbound = |line: &str| access_outbound(line, &targets);
        assert_eq!(
            outbound("from tcp:127.0.0.1:49290 accepted tcp:www.cloudflare.com:443 [mixed -> proxy-fi]").as_deref(),
            Some("proxy-fi")
        );
        // Through a TUN the destination is the address the app connected to.
        assert_eq!(
            outbound("from tcp:172.19.0.1:41234 accepted tcp:104.16.124.96:443 [tun >> proxy-7]").as_deref(),
            Some("proxy-7")
        );
        assert_eq!(outbound("from 1.2.3.4:5 accepted tcp:104.16.124.97:443 [tun -> proxy]"), None);
        assert_eq!(outbound("from 1.2.3.4:5 accepted tcp:example.com:443 [mixed -> proxy]"), None);
        assert_eq!(outbound("[Warning] www.cloudflare.com:443 [x -> y]"), None);
    }

    #[test]
    fn last_problem_prefers_errors() {
        let line = |id, level, text: &str| LogLine { id, time: 0, level, text: text.into() };
        let logs: VecDeque<LogLine> = [
            line(1, "error", "the real cause"),
            line(2, "warning", "a later warning"),
            line(3, "info", "bye"),
        ]
        .into();
        assert_eq!(last_problem(&logs, 1).as_deref(), Some("the real cause"));
        // Lines of earlier runs do not explain this one.
        assert_eq!(last_problem(&logs, 2).as_deref(), Some("a later warning"));
        assert_eq!(last_problem(&logs, 3).as_deref(), Some("bye"));
        assert_eq!(last_problem(&logs, 4), None);
        // Access lines are traffic, not a reason.
        let logs: VecDeque<LogLine> = [
            line(1, "info", "started"),
            line(2, "access", "from 10.0.0.45:41314 accepted tcp:149.154.167.92:5222 [tun -> proxy]"),
        ]
        .into();
        assert_eq!(last_problem(&logs, 1).as_deref(), Some("started"));
        assert_eq!(last_problem(&logs, 2), None);
        assert_eq!(last_problem(&VecDeque::new(), 0), None);
    }
}
