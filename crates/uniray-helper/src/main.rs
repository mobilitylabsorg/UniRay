//! Privileged supervisor for TUN mode.
//!
//! Creating a TUN interface and installing routes needs root, and the GUI must
//! not run as root. The GUI starts this helper elevated (`pkexec` on Linux)
//! and keeps a pipe to its stdin:
//!
//! ```text
//! uniray-helper <xray> <config.json> <asset dir>
//! ```
//!
//! The helper runs Xray and stops it when the pipe closes - whether the GUI
//! asked for it or died - or on SIGTERM/SIGINT. Xray's output is inherited,
//! so the GUI reads the core's log from the helper's stdout.
//!
//! On Windows the GUI starts the helper through UAC, and on macOS through
//! osascript's administrator prompt; neither allows a pipe between the two,
//! nor signals to a process of another user:
//!
//! ```text
//! uniray-helper <xray> <config.json> <asset dir> <GUI pid> <stop file> <log file>
//! ```
//!
//! There the helper writes Xray's output to the log file and stops Xray when
//! the stop file appears or the GUI process is gone.
//!
//! For a server Xray cannot run (WireGuard, AmneziaWG) the arguments end with
//! `--engine <uniray-engine> <mode> <engine config>`. The helper then starts
//! the engine before Xray, as root too - the engine only serves the process
//! it is told is Xray, and checking another root process needs root - tells
//! it Xray's pid over its stdin, and stops it after Xray. The engine exiting
//! ends the run like Xray exiting does.

// Started hidden through UAC on Windows: no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    unix::run()
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows::run()
}

#[cfg(not(any(unix, windows)))]
fn main() -> std::process::ExitCode {
    eprintln!("uniray-helper: this platform is not supported");
    std::process::ExitCode::FAILURE
}

/// Splits off a trailing `--engine <binary> <mode> <config>`.
#[cfg(any(unix, windows))]
fn split_engine(args: &[std::ffi::OsString]) -> (&[std::ffi::OsString], Option<&[std::ffi::OsString]>) {
    match args.len().checked_sub(4) {
        Some(at) if args[at] == "--engine" => (&args[..at], Some(&args[at + 1..])),
        _ => (args, None),
    }
}

/// Starts the engine, its stdin a pipe that the helper holds: the engine
/// exits when it closes.
#[cfg(any(unix, windows))]
fn engine_command(engine: &[std::ffi::OsString]) -> std::process::Command {
    let mut command = std::process::Command::new(&engine[0]);
    command.arg(&engine[1]).arg("--config").arg(&engine[2]).stdin(std::process::Stdio::piped());
    command
}

/// Tells the engine which process is Xray.
#[cfg(any(unix, windows))]
fn register_peer(engine: &mut std::process::Child, xray: &std::process::Child) {
    use std::io::Write;
    if let Some(stdin) = engine.stdin.as_mut() {
        let _ = writeln!(stdin, "peer {}", xray.id());
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    static STOP: AtomicBool = AtomicBool::new(false);

    /// How long Xray gets to remove its interface and routes before SIGKILL.
    const GRACE: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(100);

    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    /// The GUI could not keep a pipe to us (macOS): it is watched instead.
    struct Detached {
        gui: libc::pid_t,
        stop_file: PathBuf,
    }

    impl Detached {
        fn should_stop(&self) -> bool {
            // SAFETY: kill with signal 0 only checks for the process. A GUI
            // running as another user answers EPERM, which means it is there.
            let gui_alive = unsafe { libc::kill(self.gui, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            !gui_alive || self.stop_file.exists()
        }
    }

    pub fn run() -> ExitCode {
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        let (args, engine) = super::split_engine(&args);
        let (xray, config, assets, detached) = match args {
            [xray, config, assets] => (xray, config, assets, None),
            [xray, config, assets, gui, stop_file, log_file] => {
                let Ok(gui) = gui.to_string_lossy().parse::<libc::pid_t>() else {
                    eprintln!("uniray-helper: bad GUI pid");
                    return ExitCode::from(2);
                };
                if let Err(err) = log_to(log_file) {
                    eprintln!("uniray-helper: cannot open the log file: {err}");
                    return ExitCode::FAILURE;
                }
                (xray, config, assets, Some(Detached { gui, stop_file: PathBuf::from(stop_file) }))
            }
            _ => {
                eprintln!(
                    "usage: uniray-helper <xray> <config.json> <asset dir> [<GUI pid> <stop file> <log file>] \
                     [--engine <engine> <mode> <config>]"
                );
                return ExitCode::from(2);
            }
        };

        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe {
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        }
        if detached.is_none() {
            thread::spawn(|| {
                let mut sink = [0u8; 64];
                let mut stdin = std::io::stdin();
                // Anything but more data - EOF or an error - means the GUI is gone.
                while matches!(stdin.read(&mut sink), Ok(read) if read > 0) {}
                STOP.store(true, Ordering::SeqCst);
            });
        }

        let mut engine = match engine.map(|engine| spawn(super::engine_command(engine))).transpose() {
            Ok(engine) => engine,
            Err(err) => {
                eprintln!("uniray-helper: cannot start the engine: {err}");
                return ExitCode::FAILURE;
            }
        };
        let mut command = Command::new(xray);
        command
            .arg("run")
            .arg("-c")
            .arg(config)
            .env("XRAY_LOCATION_ASSET", assets)
            .stdin(Stdio::null());
        let mut child = match spawn(command) {
            Ok(child) => child,
            Err(err) => {
                eprintln!("uniray-helper: cannot start {}: {err}", xray.to_string_lossy());
                if let Some(engine) = engine.as_mut() {
                    stop_engine(engine);
                }
                return ExitCode::FAILURE;
            }
        };
        if let Some(engine) = engine.as_mut() {
            super::register_peer(engine, &child);
        }

        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break exit_code(status.code()),
                Ok(None) => {}
                Err(err) => {
                    eprintln!("uniray-helper: waiting for xray failed: {err}");
                    break ExitCode::FAILURE;
                }
            }
            if let Some(engine) = engine.as_mut() {
                if !matches!(engine.try_wait(), Ok(None)) {
                    eprintln!("uniray-helper: the engine exited");
                    stop(&mut child);
                    break ExitCode::FAILURE;
                }
            }
            if STOP.load(Ordering::SeqCst) || detached.as_ref().is_some_and(Detached::should_stop) {
                stop(&mut child);
                break ExitCode::SUCCESS;
            }
            thread::sleep(POLL);
        };
        if let Some(engine) = engine.as_mut() {
            stop_engine(engine);
        }
        if let Some(detached) = &detached {
            let _ = std::fs::remove_file(&detached.stop_file);
        }
        code
    }

    /// Sends our output, and so Xray's and the engine's, to `path`.
    fn log_to(path: &OsString) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            // SAFETY: dup2 onto the standard descriptors, from a file we hold open.
            if unsafe { libc::dup2(log.as_raw_fd(), fd) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn spawn(mut command: Command) -> std::io::Result<Child> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: prctl is async-signal-safe and touches no shared state.
            unsafe {
                command.pre_exec(|| {
                    // Do not let it outlive a helper that was killed outright.
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
        }
        command.spawn()
    }

    /// Closes the engine's stdin, which makes it exit, and waits for that.
    fn stop_engine(engine: &mut Child) {
        drop(engine.stdin.take());
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if !matches!(engine.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = engine.kill();
        let _ = engine.wait();
    }

    fn stop(child: &mut Child) {
        // SAFETY: the pid belongs to our child, which has not been reaped yet.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    fn exit_code(code: Option<i32>) -> ExitCode {
        match code {
            Some(0) => ExitCode::SUCCESS,
            Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            None => ExitCode::FAILURE,
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::thread;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const POLL: Duration = Duration::from_millis(200);

    pub fn run() -> ExitCode {
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        let (args, engine) = super::split_engine(&args);
        let [xray, config, assets, gui_pid, stop_file, log_file] = args else {
            eprintln!(
                "usage: uniray-helper <xray> <config.json> <asset dir> <GUI pid> <stop file> <log file> \
                 [--engine <engine> <mode> <config>]"
            );
            return ExitCode::from(2);
        };
        let stop_file = PathBuf::from(stop_file);
        let Ok(gui_pid) = gui_pid.to_string_lossy().parse::<u32>() else {
            eprintln!("uniray-helper: bad GUI pid");
            return ExitCode::from(2);
        };
        let output = match File::create(log_file) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("uniray-helper: cannot create the log file: {err}");
                return ExitCode::FAILURE;
            }
        };
        let errors = match output.try_clone() {
            Ok(file) => file,
            Err(_) => return ExitCode::FAILURE,
        };
        let mut engine = match engine.map(|engine| spawn_engine(engine, &output)).transpose() {
            Ok(engine) => engine,
            Err(err) => {
                let _ = writeln!(&output, "uniray-helper: cannot start the engine: {err}");
                return ExitCode::FAILURE;
            }
        };

        let mut child = match Command::new(xray)
            .arg("run")
            .arg("-c")
            .arg(config)
            .env("XRAY_LOCATION_ASSET", assets)
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                eprintln!("uniray-helper: cannot start {}: {err}", xray.to_string_lossy());
                if let Some(engine) = engine.as_mut() {
                    stop_engine(engine);
                }
                return ExitCode::FAILURE;
            }
        };
        bind_to_self(&child);
        if let Some(engine) = engine.as_mut() {
            super::register_peer(engine, &child);
        }

        // SAFETY: OpenProcess with a plain pid; a null handle is handled below.
        let gui = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, gui_pid) };
        let gui_alive = || {
            // Without a handle only the stop file can end the run.
            // SAFETY: a valid process handle, polled without waiting.
            gui.is_null() || unsafe { WaitForSingleObject(gui, 0) } != WAIT_OBJECT_0
        };

        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code().map_or(1, |code| code as u8),
                Ok(None) => {}
                Err(_) => break 1,
            }
            let engine_gone = engine.as_mut().is_some_and(|engine| !matches!(engine.try_wait(), Ok(None)));
            if engine_gone || stop_file.exists() || !gui_alive() {
                // Xray has no console to receive Ctrl+Break; Windows removes
                // the Wintun adapter and its routes when the process ends.
                let _ = child.kill();
                let _ = child.wait();
                break u8::from(engine_gone);
            }
            thread::sleep(POLL);
        };
        if let Some(engine) = engine.as_mut() {
            stop_engine(engine);
        }
        let _ = std::fs::remove_file(&stop_file);
        if !gui.is_null() {
            // SAFETY: the handle came from OpenProcess above.
            unsafe { CloseHandle(gui) };
        }
        ExitCode::from(code)
    }

    fn spawn_engine(engine: &[OsString], log: &File) -> std::io::Result<Child> {
        let mut command = super::engine_command(engine);
        command.stdout(log.try_clone()?).stderr(log.try_clone()?).creation_flags(CREATE_NO_WINDOW);
        let child = command.spawn()?;
        bind_to_self(&child);
        Ok(child)
    }

    /// Closes the engine's stdin, which makes it exit, and waits for that.
    fn stop_engine(engine: &mut Child) {
        drop(engine.stdin.take());
        for _ in 0..25 {
            if !matches!(engine.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = engine.kill();
        let _ = engine.wait();
    }

    /// Takes a child down with the helper, should the helper itself be killed.
    fn bind_to_self(child: &Child) {
        // SAFETY: plain Win32 calls; the job handle stays open for the life of
        // the helper and closes with it, which kills the job's processes.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            AssignProcessToJobObject(job, child.as_raw_handle() as _);
        }
    }
}
