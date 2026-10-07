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

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    unix::run()
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("uniray-helper: this platform is not supported yet");
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
