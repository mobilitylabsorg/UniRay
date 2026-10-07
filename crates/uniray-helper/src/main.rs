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
//! On Windows the GUI starts the helper through UAC, which allows neither a
//! pipe nor signals between the two:
//!
//! ```text
//! uniray-helper.exe <xray> <config.json> <asset dir> <GUI pid> <stop file> <log file>
//! ```
//!
//! There the helper writes Xray's output to the log file and stops Xray when
//! the stop file appears or the GUI process is gone.

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

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::io::Read;
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

    pub fn run() -> ExitCode {
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        let [xray, config, assets] = args.as_slice() else {
            eprintln!("usage: uniray-helper <xray> <config.json> <asset dir>");
            return ExitCode::from(2);
        };

        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe {
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        }
        thread::spawn(|| {
            let mut sink = [0u8; 64];
            let mut stdin = std::io::stdin();
            // Anything but more data - EOF or an error - means the GUI is gone.
            while matches!(stdin.read(&mut sink), Ok(read) if read > 0) {}
            STOP.store(true, Ordering::SeqCst);
        });

        let mut command = Command::new(xray);
        command
            .arg("run")
            .arg("-c")
            .arg(config)
            .env("XRAY_LOCATION_ASSET", assets)
            .stdin(Stdio::null());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: prctl is async-signal-safe and touches no shared state.
            unsafe {
                command.pre_exec(|| {
                    // Do not let Xray outlive a helper that was killed outright.
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                eprintln!("uniray-helper: cannot start {}: {err}", xray.to_string_lossy());
                return ExitCode::FAILURE;
            }
        };

        loop {
            match child.try_wait() {
                Ok(Some(status)) => return exit_code(status.code()),
                Ok(None) => {}
                Err(err) => {
                    eprintln!("uniray-helper: waiting for xray failed: {err}");
                    return ExitCode::FAILURE;
                }
            }
            if STOP.load(Ordering::SeqCst) {
                stop(&mut child);
                return ExitCode::SUCCESS;
            }
            thread::sleep(POLL);
        }
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
        let [xray, config, assets, gui_pid, stop_file, log_file] = args.as_slice() else {
            eprintln!("usage: uniray-helper <xray> <config.json> <asset dir> <GUI pid> <stop file> <log file>");
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
                return ExitCode::FAILURE;
            }
        };
        bind_to_self(&child);

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
            if stop_file.exists() || !gui_alive() {
                // Xray has no console to receive Ctrl+Break; Windows removes
                // the Wintun adapter and its routes when the process ends.
                let _ = child.kill();
                let _ = child.wait();
                break 0;
            }
            thread::sleep(POLL);
        };
        let _ = std::fs::remove_file(&stop_file);
        if !gui.is_null() {
            // SAFETY: the handle came from OpenProcess above.
            unsafe { CloseHandle(gui) };
        }
        ExitCode::from(code)
    }

    /// Takes Xray down with the helper, should the helper itself be killed.
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
