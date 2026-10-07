use serde::Serialize;

/// An error as the frontend sees it: `code` selects the localized text,
/// `message` carries the technical detail.
#[derive(Debug, Clone, Serialize)]
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
}

pub type CmdResult<T> = Result<T, CmdError>;

impl CmdError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The core (Xray) failed to start or died.
    pub fn core(message: impl Into<String>) -> Self {
        Self::new("core", message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new("network", message)
    }

    pub fn io(err: std::io::Error) -> Self {
        Self::new("io", err.to_string())
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<uniray_core::Error> for CmdError {
    fn from(err: uniray_core::Error) -> Self {
        use uniray_core::Error::*;
        let code = match &err {
            UnsupportedScheme(_) | InvalidLink(_) | Unsupported(_) => "invalidLink",
            EmptySubscription => "emptySubscription",
            HwidLimitReached => "hwidLimit",
            HwidRequired => "hwidRequired",
            NotFoundWithoutHwid => "notFoundNoHwid",
            Http(_) => "http",
            InvalidConfig(_) => "invalidConfig",
        };
        Self::new(code, err.to_string())
    }
}
