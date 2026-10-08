//! Desktop: Xray runs as a child process.
//!
//! In proxy mode it is a plain child of the app. TUN mode needs root, so the
//! app starts `uniray-helper` elevated and the helper runs Xray; the helper
//! stops Xray when the pipe to its stdin closes.
//!
//! A server Xray cannot run (WireGuard, AmneziaWG) also gets `uniray-engine`,
//! which Xray reaches as a SOCKS proxy (see `uniray_core::engine`): our child
//! next to Xray in proxy mode, the helper's in TUN mode.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, watch};
use uniray_core::config::{self, LogSink, Runtime, Tun};
use uniray_core::engine::{EngineRun, Peers};
use uniray_core::model::{Mode, Server, Settings};

use super::{free_port, prepare_engine, PROBE_TIMEOUT, PROBE_URL};
use crate::app::App;
use crate::error::{CmdError, CmdResult};
use crate::state::write_private;
use crate::{logs, stats, sysproxy};

/// The TUN interface name to pass to Xray, honouring platform rules.
fn interface_name(configured: &str) -> String {
    let name = configured.trim();
    let name = if name.is_empty() { uniray_core::config::DEFAULT_INTERFACE_NAME } else { name };
    // macOS requires utunN; any other name makes the interface fail to come up.
    #[cfg(target_os = "macos")]
    if !name.starts_with("utun") {
        return "utun9".into();
    }
    name.chars().take(15).collect()
}
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// TUN start includes the user typing a password into the polkit prompt.
const READY_TIMEOUT_TUN: Duration = Duration::from_secs(120);
/// How long the helper gets to stop Xray before it is given up on.
const HELPER_STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Servers measured by one probe process.
const PROBE_BATCH: usize = 32;

pub struct Running {
    /// First log line of this run, see [`logs::mark`].
    pub log_mark: u64,
    stop: Option<oneshot::Sender<()>>,
    supervisor: JoinHandle<()>,
    /// Follows the core's log file when the core is not our child (Windows TUN).
    tailer: Option<JoinHandle<()>>,
    stats: JoinHandle<()>,
    system_proxy: bool,
    mixed_port: u16,
    /// The engine, when it is our child (proxy mode).
    engine: Option<Supervised>,
}

/// A child's stop trigger and supervising task, see [`spawn_supervised`].
type Supervised = (oneshot::Sender<()>, JoinHandle<()>);
/// [`Supervised`], the flag that turns true once the child is gone and its pid.
type Spawned = (oneshot::Sender<()>, JoinHandle<()>, watch::Receiver<bool>, Option<u32>);

impl Running {
    pub fn mixed_port(&self) -> Option<u16> {
        Some(self.mixed_port)
    }

    pub async fn start(app: &AppHandle, server: &Server, settings: &Settings, generation: u64) -> CmdResult<Self> {
        let state = app.state::<App>();
        let tun = settings.mode == Mode::Tun;
        // TUN needs the elevated helper: pkexec on Linux, UAC on Windows.
        if tun && !cfg!(any(target_os = "linux", windows)) {
            return Err(CmdError::new("unsupported", "TUN mode is not available on this platform yet"));
        }
        let mixed_port = settings.mixed_port;
        if !port_is_free(mixed_port) {
            return Err(CmdError::new("portBusy", format!("port {mixed_port} is already in use")));
        }
        let metrics_port = free_port()?;
        let engine_run = prepare_engine(app, server, settings).await?;

        let runtime = Runtime {
            tun: tun.then(|| Tun::Managed { name: interface_name(&settings.interface_name) }),
            mixed_port: Some(mixed_port),
            metrics_port: Some(metrics_port), metrics_socket: None,
            log: LogSink::Stdout,
            engine: engine_run.as_ref().map(|run| run.runtime.clone()),
        };
        let built = config::build(server, settings, &runtime)?;
        let config_path = state.paths.run_dir.join("config.json");
        let config_bytes = serde_json::to_vec(&built.config).map_err(|err| CmdError::core(err.to_string()))?;
        write_private(&config_path, &config_bytes).map_err(CmdError::io)?;

        let log_mark = logs::mark();
        let (stop, supervisor, mut exited, tailer, engine) =
            launch(app, tun, &config_path, engine_run.as_ref(), generation).await?;

        let timeout = if tun { READY_TIMEOUT_TUN } else { READY_TIMEOUT };
        let mut ports = vec![metrics_port, mixed_port];
        ports.extend(engine_run.as_ref().map(|run| run.runtime.port));
        if let Err(err) = wait_ready(&ports, &mut exited, timeout).await {
            let _ = stop.send(());
            let _ = supervisor.await;
            stop_engine(engine).await;
            if let Some(tailer) = tailer {
                tailer.abort();
            }
            let reason = logs::last_problem(&state.logs.lock().unwrap(), log_mark);
            return Err(CmdError::core(reason.unwrap_or(err)));
        }

        let mut system_proxy = false;
        if settings.proxy_auth {
            // System proxy settings carry no password, so apps would only
            // get "authentication required" from the proxy.
            if !tun {
                logs::push(app, "[Info] the local proxy requires a password, so the system proxy is left unset");
            }
        } else if !tun {
            match sysproxy::set(mixed_port).await {
                Ok(()) => system_proxy = true,
                Err(err) => logs::push(app, &format!("[Warning] cannot set the system proxy: {err}")),
            }
        }

        Ok(Self {
            log_mark,
            stop: Some(stop),
            supervisor,
            tailer,
            stats: stats::spawn(app.clone(), stats::MetricsSource::Port(metrics_port), built.proxy_tags),
            system_proxy,
            mixed_port,
            engine,
        })
    }

    /// Stops the core. Returns what went wrong tearing it down, if anything
    /// the user needs to know about; on desktop there is nothing of the kind.
    pub async fn stop(mut self, app: &AppHandle) -> Option<CmdError> {
        self.stats.abort();
        if self.system_proxy {
            if let Err(err) = sysproxy::reset().await {
                logs::push(app, &format!("[Warning] cannot reset the system proxy: {err}"));
            }
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = self.supervisor.await;
        // After Xray, which has nothing to send it any more.
        stop_engine(self.engine.take()).await;
        if let Some(tailer) = self.tailer.take() {
            // Let it pick up the core's last words first.
            tokio::time::sleep(Duration::from_millis(500)).await;
            tailer.abort();
        }
        let run_dir = &app.state::<App>().paths.run_dir;
        let _ = std::fs::remove_file(run_dir.join("config.json"));
        // The engine removes it once read; not if it never started.
        let _ = std::fs::remove_file(run_dir.join(ENGINE_CONFIG));
        None
    }
}

/// The engine's config in the run directory, see `uniray_core::engine`.
const ENGINE_CONFIG: &str = "engine.json";

async fn stop_engine(engine: Option<Supervised>) {
    if let Some((stop, supervisor)) = engine {
        let _ = stop.send(());
        let _ = supervisor.await;
    }
}

type Launched = (
    oneshot::Sender<()>,
    JoinHandle<()>,
    watch::Receiver<bool>,
    Option<JoinHandle<()>>,
    Option<Supervised>,
);

/// Starts the core: directly in proxy mode, through the elevated helper in
/// TUN mode (pkexec on Linux, UAC on Windows), and the engine with it.
async fn launch(
    app: &AppHandle,
    tun: bool,
    config_path: &Path,
    engine: Option<&EngineRun>,
    generation: u64,
) -> CmdResult<Launched> {
    let state = app.state::<App>();
    let engine_config = state.paths.run_dir.join(ENGINE_CONFIG);
    if tun {
        // The helper tells the engine Xray's pid once it has started it.
        if let Some(run) = engine {
            write_private(&engine_config, &run.config_for(&Peers::Pids(Vec::new()), false)).map_err(CmdError::io)?;
        }
        #[cfg(windows)]
        return spawn_elevated(app, config_path, engine.map(|run| (run.mode, engine_config.as_path())), generation).await;
        #[cfg(not(windows))]
        {
            let staged = root_readable(app)?;
            let mut command = Command::new("pkexec");
            command
                .arg(&staged.helper)
                .arg(&staged.xray)
                .arg(config_path)
                .arg(&staged.assets)
                .stdin(Stdio::piped());
            if let Some(run) = engine {
                command.arg("--engine").arg(&staged.engine).arg(run.mode).arg(&engine_config);
            }
            let (stop, supervisor, exited, _) = spawn_supervised(app, command, Some(generation))?;
            return Ok((stop, supervisor, exited, None, None));
        }
    }

    let command = xray_command(&state.paths.xray, config_path, &state.paths.assets);
    let (stop, supervisor, exited, xray_pid) = spawn_supervised(app, command, Some(generation))?;
    let Some(run) = engine else {
        return Ok((stop, supervisor, exited, None, None));
    };
    // Started after Xray, so that its config names Xray's process. Xray
    // only connects to it for traffic, which waits for `wait_ready`.
    let spawned = write_private(&engine_config, &run.config_for(&Peers::Pids(xray_pid.into_iter().collect()), false))
        .map_err(CmdError::io)
        .and_then(|()| {
            let mut command = Command::new(&state.paths.engine);
            command.arg(run.mode).arg("--config").arg(&engine_config).stdin(Stdio::piped());
            #[cfg(windows)]
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            spawn_supervised(app, command, Some(generation))
        });
    match spawned {
        Ok((engine_stop, engine_supervisor, _, _)) => {
            Ok((stop, supervisor, exited, None, Some((engine_stop, engine_supervisor))))
        }
        Err(err) => {
            let _ = stop.send(());
            let _ = supervisor.await;
            Err(err)
        }
    }
}

/// What root runs in TUN mode, from where root can read it.
#[cfg(not(windows))]
struct RootReadable {
    helper: PathBuf,
    xray: PathBuf,
    engine: PathBuf,
    assets: PathBuf,
}

/// The helper, the core and the asset directory, from where root can read
/// them. An AppImage runs from a FUSE mount that only the user who started it
/// may read - root included - so pkexec could not even open the helper there
/// ("Permission denied"). From an AppImage they are copied out to the app's
/// data directory first; an installed package needs nothing of the kind.
#[cfg(not(windows))]
fn root_readable(app: &AppHandle) -> CmdResult<RootReadable> {
    let paths = &app.state::<App>().paths;
    if std::env::var_os("APPIMAGE").is_none() {
        return Ok(RootReadable {
            helper: paths.helper.clone(),
            xray: paths.xray.clone(),
            engine: paths.engine.clone(),
            assets: paths.assets.clone(),
        });
    }
    let binaries = [paths.helper.as_path(), &paths.xray, &paths.engine];
    let (binaries, assets) =
        stage_for_root(&paths.run_dir.join("elevated"), &binaries, &paths.assets).map_err(CmdError::io)?;
    let [helper, xray, engine] = <[PathBuf; 3]>::try_from(binaries).expect("one copy per binary");
    Ok(RootReadable { helper, xray, engine, assets })
}

/// Copies the binaries (under their own names) and the files of the asset
/// directory into `dir`, which only the user can write to.
#[cfg(not(windows))]
fn stage_for_root(dir: &Path, binaries: &[&Path], assets: &Path) -> std::io::Result<(Vec<PathBuf>, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let staged_assets = dir.join("assets");
    std::fs::create_dir_all(&staged_assets)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let copy = |from: &Path, to: PathBuf, mode: u32| -> std::io::Result<PathBuf> {
        // Replaced, not overwritten in place: a running copy cannot be written.
        let _ = std::fs::remove_file(&to);
        std::fs::copy(from, &to)?;
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode))?;
        Ok(to)
    };
    let binaries = binaries
        .iter()
        .map(|binary| {
            let name = binary.file_name().ok_or_else(|| std::io::Error::other("a binary path has no file name"))?;
            copy(binary, dir.join(name), 0o755)
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    for entry in std::fs::read_dir(assets)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            copy(&entry.path(), staged_assets.join(entry.file_name()), 0o644)?;
        }
    }
    Ok((binaries, staged_assets))
}

/// Windows TUN: the helper runs as administrator after a UAC prompt. It
/// cannot share pipes with us, so it logs to a file we follow, and stops when
/// the stop file appears or this process is gone.
#[cfg(windows)]
async fn spawn_elevated(
    app: &AppHandle,
    config_path: &Path,
    engine: Option<(&str, &Path)>,
    generation: u64,
) -> CmdResult<Launched> {
    use std::ffi::{OsStr, OsString};
    use std::sync::Arc;

    use super::elevated::{LaunchError, Process};

    let state = app.state::<App>();
    let run_dir = state.paths.run_dir.clone();
    let stop_file = run_dir.join("helper.stop");
    let log_file = run_dir.join("core.log");
    let _ = std::fs::remove_file(&stop_file);
    write_private(&log_file, b"").map_err(CmdError::io)?;

    let helper = state.paths.helper.clone();
    let mut args: Vec<OsString> = vec![
        state.paths.xray.clone().into_os_string(),
        config_path.as_os_str().to_owned(),
        state.paths.assets.clone().into_os_string(),
        std::process::id().to_string().into(),
        stop_file.clone().into_os_string(),
        log_file.clone().into_os_string(),
    ];
    if let Some((mode, config)) = engine {
        args.extend(["--engine".into(), state.paths.engine.clone().into_os_string(), mode.into(), config.into()]);
    }
    let launched = tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        Process::launch(&helper, &args)
    })
    .await
    .map_err(|err| CmdError::core(err.to_string()))?;
    let process = Arc::new(launched.map_err(|err| match err {
        LaunchError::Cancelled => CmdError::new("elevationCancelled", "administrator rights were not granted"),
        LaunchError::Failed(message) => CmdError::core(format!("cannot start the helper: {message}")),
    })?);

    let tailer = logs::tail(app.clone(), vec![log_file]);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (exited_tx, exited_rx) = watch::channel(false);
    let app = app.clone();
    let supervisor = tauri::async_runtime::spawn(async move {
        let waiter = {
            let process = process.clone();
            tauri::async_runtime::spawn_blocking(move || process.wait(None))
        };
        tokio::select! {
            _ = waiter => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { super::on_core_exit(&app, generation).await });
            }
            // Resolves on an explicit stop and when the trigger is dropped.
            _ = stop_rx => {
                let _ = std::fs::write(&stop_file, b"stop");
                let timeout = HELPER_STOP_TIMEOUT.as_millis() as u32;
                let waiting = process.clone();
                let ended = tauri::async_runtime::spawn_blocking(move || waiting.wait(Some(timeout)))
                    .await
                    .unwrap_or(false);
                if !ended {
                    process.terminate();
                }
            }
        }
        let _ = exited_tx.send(true);
    });
    Ok((stop_tx, supervisor, exited_rx, Some(tailer), None))
}

fn xray_command(xray: &std::path::Path, config: &std::path::Path, assets: &std::path::Path) -> Command {
    let mut command = Command::new(xray);
    command
        .arg("run")
        .arg("-c")
        .arg(config)
        .env("XRAY_LOCATION_ASSET", assets)
        .stdin(Stdio::null());
    // No console window for the core on Windows.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    // If the app is killed or crashes, take Xray down with it: an orphaned
    // core would keep the proxy port busy for the next start. (On Windows a
    // job object does the same, see `bind_to_app`.)
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches no shared state.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    command
}

/// Puts the child into a job object that kills its processes when the app's
/// handle to it closes, which Windows does when the app exits for any reason.
#[cfg(windows)]
fn bind_to_app(child: &Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    static JOB: OnceLock<usize> = OnceLock::new();
    // SAFETY: plain Win32 calls on handles this process owns; the job handle
    // is deliberately kept open for the life of the process.
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        job as usize
    });
    if let Some(handle) = child.raw_handle() {
        // SAFETY: the handle belongs to our live child.
        unsafe {
            AssignProcessToJobObject(job as _, handle as _);
        }
    }
}

/// Spawns the core, forwards its output to the log and supervises it.
///
/// Returns the stop trigger, the supervising task, a flag that turns true
/// once the process is gone and its pid. With `generation` set, an exit that
/// nobody asked for is reported through [`super::on_core_exit`].
fn spawn_supervised(
    app: &AppHandle,
    mut command: Command,
    generation: Option<u64>,
) -> CmdResult<Spawned> {
    // Only a real run is logged; an unread pipe would eventually block a probe.
    let output = || if generation.is_some() { Stdio::piped() } else { Stdio::null() };
    command.stdout(output()).stderr(output()).kill_on_drop(true);
    let program = command.as_std().get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|err| CmdError::core(format!("cannot start {program}: {err}")))?;
    #[cfg(windows)]
    bind_to_app(&child);
    let pid = child.id();

    forward_output(app, child.stdout.take());
    forward_output(app, child.stderr.take());
    let stdin = child.stdin.take();
    let (stop_tx, stop_rx) = oneshot::channel();
    let (exited_tx, exited_rx) = watch::channel(false);
    let app = app.clone();
    let supervisor = tauri::async_runtime::spawn(async move {
        supervise(&app, child, stdin, stop_rx, generation).await;
        let _ = exited_tx.send(true);
    });
    Ok((stop_tx, supervisor, exited_rx, pid))
}

async fn supervise(
    app: &AppHandle,
    mut child: Child,
    stdin: Option<ChildStdin>,
    stop: oneshot::Receiver<()>,
    generation: Option<u64>,
) {
    tokio::select! {
        _ = child.wait() => {
            if let Some(generation) = generation {
                // Not awaited: the handler stops this very task's `Running`.
                let app = app.clone();
                tauri::async_runtime::spawn(async move { super::on_core_exit(&app, generation).await });
            }
        }
        // Resolves on an explicit stop and when the trigger is dropped.
        _ = stop => match stdin {
            // The helper runs as root and cannot be signalled; closing its
            // stdin makes it stop Xray, which removes the interface and routes.
            // The engine, too, exits when its stdin closes.
            Some(stdin) => {
                drop(stdin);
                if tokio::time::timeout(HELPER_STOP_TIMEOUT, child.wait()).await.is_err() {
                    let _ = child.start_kill();
                }
            }
            None => {
                let _ = child.kill().await;
            }
        },
    }
}

fn forward_output(app: &AppHandle, stream: Option<impl AsyncRead + Unpin + Send + 'static>) {
    let Some(stream) = stream else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            logs::push(&app, &line);
        }
    });
}

/// Waits until every port accepts connections, failing early when the process
/// exits first.
async fn wait_ready(ports: &[u16], exited: &mut watch::Receiver<bool>, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if *exited.borrow() {
            return Err("the core exited during startup".into());
        }
        let mut ready = true;
        for port in ports {
            ready &= TcpStream::connect((Ipv4Addr::LOCALHOST, *port)).await.is_ok();
        }
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the core did not become ready in time".into());
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            _ = exited.changed() => {}
        }
    }
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
}

/// Measures the delay of each server: the time of one HTTPS request through
/// it, in milliseconds, or `-1` when it fails. `on_result` is called as the
/// results arrive.
///
/// A throwaway Xray process exposes one local SOCKS port per server, so the
/// measurement does not disturb the live connection.
pub async fn measure_delays(
    app: &AppHandle,
    servers: Vec<Server>,
    on_result: impl Fn(&str, i64) + Sync,
) -> CmdResult<()> {
    for batch in servers.chunks(PROBE_BATCH) {
        if let Err(err) = probe_batch(app, batch, &on_result).await {
            logs::push(app, &format!("[Warning] delay test failed: {}", err.message));
            for server in batch {
                on_result(&server.id, -1);
            }
        }
    }
    Ok(())
}

async fn probe_batch(app: &AppHandle, servers: &[Server], on_result: &(impl Fn(&str, i64) + Sync)) -> CmdResult<()> {
    let state = app.state::<App>();
    let ports = servers.iter().map(|_| free_port()).collect::<CmdResult<Vec<u16>>>()?;
    let probe = config::build_probe(&servers.iter().collect::<Vec<_>>(), &ports)?;
    let config_path = state.paths.run_dir.join(format!("probe-{}.json", ports[0]));
    let config_bytes = serde_json::to_vec(&probe).map_err(|err| CmdError::core(err.to_string()))?;
    write_private(&config_path, &config_bytes).map_err(CmdError::io)?;

    let command = xray_command(&state.paths.xray, &config_path, &state.paths.assets);
    let spawned = spawn_supervised(app, command, None);
    let result = match spawned {
        Ok((stop, supervisor, mut exited, _)) => {
            let ready = wait_ready(&ports, &mut exited, READY_TIMEOUT).await;
            if ready.is_ok() {
                join_all(servers.iter().zip(&ports).map(|(server, port)| async move {
                    on_result(&server.id, probe_delay(*port).await);
                }))
                .await;
            }
            let _ = stop.send(());
            let _ = supervisor.await;
            ready.map_err(CmdError::core)
        }
        Err(err) => Err(err),
    };
    let _ = std::fs::remove_file(config_path);
    result
}

async fn probe_delay(port: u16) -> i64 {
    let request = async {
        let proxy = reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}")).ok()?;
        let client = reqwest::Client::builder().proxy(proxy).build().ok()?;
        let started = Instant::now();
        let response = client.get(PROBE_URL).send().await.ok()?;
        response.status().is_success().then(|| started.elapsed().as_millis() as i64)
    };
    tokio::time::timeout(PROBE_TIMEOUT, request)
        .await
        .ok()
        .flatten()
        .unwrap_or(-1)
}

#[cfg(all(test, not(windows)))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::stage_for_root;

    #[test]
    fn staging_copies_what_root_runs() {
        let root = std::env::temp_dir().join(format!("uniray-stage-{}", std::process::id()));
        let source = root.join("mount");
        std::fs::create_dir_all(source.join("resources")).unwrap();
        std::fs::write(source.join("uniray-helper"), b"helper").unwrap();
        std::fs::write(source.join("uniray-xray"), b"xray").unwrap();
        std::fs::write(source.join("resources/geoip.dat"), b"ip").unwrap();
        std::fs::write(source.join("resources/geosite.dat"), b"site").unwrap();

        std::fs::write(source.join("uniray-engine"), b"engine").unwrap();

        let dir = root.join("run/elevated");
        let binaries = [source.join("uniray-helper"), source.join("uniray-xray"), source.join("uniray-engine")];
        let stage = || {
            let binaries: Vec<&std::path::Path> = binaries.iter().map(|binary| binary.as_path()).collect();
            stage_for_root(&dir, &binaries, &source.join("resources")).unwrap()
        };
        let (staged, assets) = stage();
        let [helper, xray, engine] = <[std::path::PathBuf; 3]>::try_from(staged).unwrap();
        assert_eq!(helper, dir.join("uniray-helper"));
        assert_eq!(std::fs::read(&helper).unwrap(), b"helper");
        assert_eq!(std::fs::read(&xray).unwrap(), b"xray");
        assert_eq!(std::fs::read(&engine).unwrap(), b"engine");
        assert_eq!(std::fs::read(assets.join("geosite.dat")).unwrap(), b"site");
        assert_eq!(std::fs::metadata(&helper).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        // Again over the previous copies, as on the next connect.
        stage();

        std::fs::remove_dir_all(&root).unwrap();
    }
}
