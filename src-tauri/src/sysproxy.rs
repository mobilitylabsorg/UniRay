//! System proxy settings (desktop).
//!
//! On Linux this is the GNOME proxy configuration via `gsettings`, which GTK
//! applications and Chromium-based browsers follow. Desktops without it are
//! not an error: the local proxy still works for anything pointed at it, and
//! TUN mode needs no proxy settings at all.

use sysproxy::Sysproxy;

const BYPASS: &str = "localhost,127.0.0.0/8,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16";
const HOST: &str = "127.0.0.1";

pub async fn set(port: u16) -> Result<(), String> {
    blocking(move || {
        Sysproxy {
            enable: true,
            host: HOST.into(),
            port,
            bypass: BYPASS.into(),
        }
        .set_system_proxy()
    })
    .await
}

pub async fn reset() -> Result<(), String> {
    blocking(|| {
        Sysproxy {
            enable: false,
            host: HOST.into(),
            port: 0,
            bypass: String::new(),
        }
        .set_system_proxy()
    })
    .await
}

/// Clears a system proxy left behind by a run that was killed before it could
/// clean up: it points at our local port, where nothing listens any more.
pub async fn reset_if_stale(port: u16) {
    let current = blocking(Sysproxy::get_system_proxy).await;
    if let Ok(proxy) = current {
        if proxy.enable && proxy.host == HOST && proxy.port == port {
            let _ = reset().await;
        }
    }
}

async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> sysproxy::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(call)
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}
