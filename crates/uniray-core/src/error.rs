use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("unsupported link scheme: {0}")]
    UnsupportedScheme(String),
    #[error("invalid link: {0}")]
    InvalidLink(String),
    #[error("unsupported link option: {0}")]
    Unsupported(String),
    #[error("subscription contains no servers")]
    EmptySubscription,
    /// Remnawave refused the request because the device limit of the user is reached.
    #[error("device limit reached")]
    HwidLimitReached,
    /// The panel requires `x-hwid` and it was not sent.
    #[error("subscription requires HWID")]
    HwidRequired,
    /// 404 to a request without `x-hwid`: either a wrong URL or a panel that
    /// hides subscriptions from clients that do not identify the device.
    #[error("subscription not found (HTTP 404); the panel may require HWID, which is turned off")]
    NotFoundWithoutHwid,
    #[error("subscription server returned HTTP {0}")]
    Http(u16),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
