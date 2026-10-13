//! Running the TUN helper as administrator on Windows and macOS.
//!
//! Windows starts it through UAC (`ShellExecuteExW` with the `runas` verb),
//! macOS through osascript's administrator prompt. Either way it shares no
//! pipes with the app, so it is told to stop through a file; it also stops by
//! itself when the app process is gone. See `crates/uniray-helper`.

use std::ffi::OsStr;

/// Quotes one argument for a Windows command line so that the C runtime's
/// parser (which Rust's `args_os` follows) gets it back unchanged.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn quote_arg(arg: &OsStr) -> String {
    let arg = arg.to_string_lossy();
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
        return arg.into_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are escaped, then the quote.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    // Trailing backslashes would escape the closing quote.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(windows)]
pub use process::Process;

#[cfg(windows)]
mod process {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    use super::quote_arg;

    /// An elevated process; the handle is kept as an integer to be `Send`.
    pub struct Process(usize);

    /// Why an elevated start did not happen.
    pub enum LaunchError {
        /// The user declined the UAC prompt.
        Cancelled,
        Failed(String),
    }

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    impl Process {
        /// Starts `program` as administrator. Blocks while the UAC prompt is
        /// shown, so call it off the async runtime.
        pub fn launch(program: &Path, args: &[&OsStr]) -> Result<Self, LaunchError> {
            let parameters: Vec<String> = args.iter().map(|arg| quote_arg(arg)).collect();
            let verb = wide(OsStr::new("runas"));
            let file = wide(program.as_os_str());
            let parameters = wide(OsStr::new(&parameters.join(" ")));
            // SAFETY: the structure is zeroed, then filled with pointers to
            // buffers that outlive the call.
            unsafe {
                let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
                info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
                info.lpVerb = verb.as_ptr();
                info.lpFile = file.as_ptr();
                info.lpParameters = parameters.as_ptr();
                info.nShow = SW_HIDE;
                if ShellExecuteExW(&mut info) == 0 {
                    let code = GetLastError();
                    return Err(if code == ERROR_CANCELLED {
                        LaunchError::Cancelled
                    } else {
                        LaunchError::Failed(std::io::Error::from_raw_os_error(code as i32).to_string())
                    });
                }
                if info.hProcess.is_null() {
                    return Err(LaunchError::Failed("no process handle".into()));
                }
                Ok(Self(info.hProcess as usize))
            }
        }

        /// Waits for the process to end, at most `timeout_ms` (`None`: no
        /// limit). Returns whether it has ended.
        pub fn wait(&self, timeout_ms: Option<u32>) -> bool {
            // SAFETY: a process handle owned by `self`.
            unsafe { WaitForSingleObject(self.0 as _, timeout_ms.unwrap_or(INFINITE)) == WAIT_OBJECT_0 }
        }

        pub fn terminate(&self) {
            // SAFETY: a process handle owned by `self`; failure is harmless.
            unsafe {
                TerminateProcess(self.0 as _, 1);
            }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: the handle came from ShellExecuteExW and is closed once.
            unsafe {
                CloseHandle(self.0 as _);
            }
        }
    }
}

#[cfg(windows)]
pub use process::LaunchError;

/// Quotes one word for `sh`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn sh_quote(word: &OsStr) -> String {
    format!("'{}'", word.to_string_lossy().replace('\'', r"'\''"))
}

/// `script` as an AppleScript string literal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn applescript_string(script: &str) -> String {
    format!("\"{}\"", script.replace('\\', r"\\").replace('"', r#"\""#))
}

#[cfg(target_os = "macos")]
pub use macos::{LaunchError, Process};

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::{applescript_string, sh_quote};

    /// The helper, running as root; known by its pid.
    pub struct Process(libc::pid_t);

    pub enum LaunchError {
        /// The user cancelled the password prompt.
        Cancelled,
        Failed(String),
    }

    impl Process {
        /// Starts `program` as root after the system's administrator prompt.
        /// `do shell script` waits for the command, so it is started in the
        /// background and the command prints its pid. Blocks while the prompt
        /// is shown, so call it off the async runtime.
        pub fn launch(program: &Path, args: &[&OsStr]) -> Result<Self, LaunchError> {
            let command: Vec<String> = std::iter::once(program.as_os_str()).chain(args.iter().copied()).map(sh_quote).collect();
            let shell = format!("{} </dev/null >/dev/null 2>&1 & echo $!", command.join(" "));
            let script = format!("do shell script {} with administrator privileges", applescript_string(&shell));
            let output = std::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(script)
                .output()
                .map_err(|err| LaunchError::Failed(err.to_string()))?;
            if !output.status.success() {
                let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                // "User canceled. (-128)"
                return Err(if message.contains("-128") { LaunchError::Cancelled } else { LaunchError::Failed(message) });
            }
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .map(Self)
                .map_err(|_| LaunchError::Failed("the helper's pid did not come back".into()))
        }

        fn alive(&self) -> bool {
            // SAFETY: signal 0 only checks for the process. A root process
            // answers EPERM to us, which means it is there.
            let checked = unsafe { libc::kill(self.0, 0) };
            checked == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }

        /// Waits for the process to end, at most `timeout_ms` (`None`: no
        /// limit). Returns whether it has ended.
        pub fn wait(&self, timeout_ms: Option<u32>) -> bool {
            let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms.into()));
            loop {
                if !self.alive() {
                    return true;
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }

        /// A root process cannot be signalled from here; the stop file and
        /// the helper's watch on the app are what end it.
        pub fn terminate(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_launch_quotes_for_sh_and_applescript() {
        assert_eq!(sh_quote(OsStr::new("/Applications/Uni Ray.app/x")), "'/Applications/Uni Ray.app/x'");
        assert_eq!(sh_quote(OsStr::new("it's")), r"'it'\''s'");
        assert_eq!(applescript_string(r#"echo "a\b""#), r#""echo \"a\\b\"""#);
    }

    #[test]
    fn arguments_survive_the_windows_command_line() {
        let quote = |arg: &str| quote_arg(OsStr::new(arg));
        assert_eq!(quote(r"C:\Program\xray.exe"), r"C:\Program\xray.exe");
        assert_eq!(quote(r"C:\Users\Ivan Petrov\AppData"), r#""C:\Users\Ivan Petrov\AppData""#);
        // A trailing backslash must not swallow the closing quote.
        assert_eq!(quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote(""), r#""""#);
        assert_eq!(quote("12345"), "12345");
    }
}
