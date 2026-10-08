//! `uniray-engine` for the servers Xray cannot run (WireGuard, AmneziaWG,
//! Naive; see `uniray_core::engine`): a child of the gateway next to Xray,
//! serving Xray, and only Xray, as a SOCKS5 proxy on loopback.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use uniray_core::engine::{EngineRun, Peers};

use crate::app::App;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_GRACE: Duration = Duration::from_secs(3);
/// The engine is a Go program like Xray, which gets the container's
/// GOMEMLIMIT: a tunnel needs far less.
const MEMORY_LIMIT: &str = "16MiB";
pub const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

pub struct Engine {
    child: Child,
    /// Held open: the engine exits when it closes, with the gateway.
    stdin: Option<ChildStdin>,
}

impl Engine {
    /// Starts the engine for Xray's process `xray_pid` and waits until it
    /// listens.
    pub fn start(app: &'static App, binary: &Path, run: &EngineRun, xray_pid: u32, dir: &Path) -> Result<Self, String> {
        let config = dir.join("engine.json");
        write_private(&config, &run.config_for(&Peers::Pids(vec![xray_pid]), false))?;
        let spawned = Command::new(binary)
            .arg(run.mode)
            .arg("--config")
            .arg(&config)
            .env("GOMEMLIMIT", MEMORY_LIMIT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                let _ = std::fs::remove_file(&config);
                return Err(format!("cannot start {}: {err}", binary.display()));
            }
        };
        let stdin = child.stdin.take();
        app.forward_output(child.stderr.take());
        // "ready <port>" is the engine's only line on stdout.
        let (ready, readied) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if line.starts_with("ready ") {
                        let _ = ready.send(());
                    }
                }
            });
        }
        let mut engine = Self { child, stdin };
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if readied.recv_timeout(Duration::from_millis(100)).is_ok() {
                return Ok(engine);
            }
            if let Some(status) = engine.exited() {
                // The engine removes its config once read; not if it failed early.
                let _ = std::fs::remove_file(&config);
                return Err(format!("the engine exited during startup ({status}), see the log"));
            }
            if Instant::now() >= deadline {
                engine.stop();
                let _ = std::fs::remove_file(&config);
                return Err("the engine did not start listening in time".into());
            }
        }
    }

    /// How the engine ended, once it has.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Closing stdin stops the engine, which stops naive with it.
    pub fn stop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && self.exited().is_none() {
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The delay of one server: the engine fetches [`PROBE_URL`] through it
/// itself. It brings up a whole tunnel, so the caller runs one at a time.
pub fn probe(binary: &Path, run: &EngineRun, dir: &Path, id: &str) -> Option<u64> {
    let config = dir.join(format!("probe-{id}.json"));
    write_private(&config, &run.config_for(&Peers::Pids(Vec::new()), false)).ok()?;
    let child = Command::new(binary)
        .arg(run.mode)
        .arg("--config")
        .arg(&config)
        .arg("--probe")
        .arg(PROBE_URL)
        .arg("--timeout")
        .arg(format!("{}s", PROBE_TIMEOUT.as_secs()))
        .env("GOMEMLIMIT", MEMORY_LIMIT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        let _ = std::fs::remove_file(&config);
        return None;
    };
    // The engine's own timeout covers the request; this, a tunnel that
    // never comes up.
    let (done, finished) = mpsc::channel();
    let stdout = child.stdout.take();
    thread::spawn(move || {
        let delay = stdout.and_then(|stdout| {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .find_map(|line| line.strip_prefix("delay ")?.trim().parse::<i64>().ok())
        });
        let _ = done.send(delay);
    });
    let delay = finished.recv_timeout(PROBE_TIMEOUT + Duration::from_secs(10)).ok().flatten();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&config);
    delay.and_then(|delay| u64::try_from(delay).ok())
}

/// The engine's config holds the server's keys: readable by us alone.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|err| format!("cannot write {}: {err}", path.display()))
}
