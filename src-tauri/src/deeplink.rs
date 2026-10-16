//! `uniray://` links: launcher shortcuts on Android, and links from a browser
//! or another app on every platform.
//!
//!   uniray://connect          uniray://disconnect          uniray://toggle
//!   uniray://import-sub?url=https://...   (opens the import dialog with it)
//!
//! tauri-plugin-deep-link delivers them (on Windows and Linux through the
//! single-instance plugin, as the OS starts a second copy of the app with the
//! link). They are parsed here, kept until the UI takes them (the link that
//! launched the app arrives before the UI has loaded), and announced with the
//! `deep-link` event.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_deep_link::DeepLinkExt;

/// What a link asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum DeepLink {
    Connect,
    Disconnect,
    Toggle,
    /// A subscription to offer for import; the user still confirms it, as
    /// any web page can open such a link.
    Import { url: String },
}

/// Parses a `uniray://` link; anything else, or an unknown action, is `None`.
pub fn parse(link: &Url) -> Option<DeepLink> {
    if link.scheme() != "uniray" {
        return None;
    }
    match link.host_str()? {
        "connect" => Some(DeepLink::Connect),
        "disconnect" => Some(DeepLink::Disconnect),
        "toggle" => Some(DeepLink::Toggle),
        "import-sub" | "import" => {
            let (_, url) = link.query_pairs().find(|(key, _)| key == "url")?;
            // Only a subscription to fetch: no other schemes from a link.
            let parsed = Url::parse(&url).ok()?;
            matches!(parsed.scheme(), "https" | "http").then(|| DeepLink::Import { url: url.into_owned() })
        }
        _ => None,
    }
}

/// Links that arrived and the UI has not taken yet.
#[derive(Default)]
struct Pending(Mutex<Vec<DeepLink>>);

pub fn install(app: &AppHandle) {
    app.manage(Pending::default());
    // An AppImage or the portable zip is not installed: register the scheme
    // for this user at runtime (a .desktop file, a registry key).
    #[cfg(any(windows, target_os = "linux"))]
    if let Err(err) = app.deep_link().register_all() {
        eprintln!("uniray: uniray:// links not registered: {err}");
    }
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        queue(app, &urls);
    }
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| queue(&handle, &event.urls()));
}

fn queue(app: &AppHandle, urls: &[Url]) {
    let links: Vec<DeepLink> = urls.iter().filter_map(parse).collect();
    if links.is_empty() {
        return;
    }
    app.state::<Pending>().0.lock().unwrap().extend(links);
    #[cfg(desktop)]
    crate::tray::show_main_window(app);
    let _ = app.emit("deep-link", ());
}

/// The links that arrived since the last call, oldest first.
#[tauri::command]
pub fn take_deep_links(app: AppHandle) -> Vec<DeepLink> {
    std::mem::take(&mut *app.state::<Pending>().0.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(text: &str) -> Option<DeepLink> {
        parse(&Url::parse(text).unwrap())
    }

    #[test]
    fn connection_actions() {
        assert_eq!(link("uniray://connect"), Some(DeepLink::Connect));
        assert_eq!(link("uniray://disconnect/"), Some(DeepLink::Disconnect));
        assert_eq!(link("uniray://toggle"), Some(DeepLink::Toggle));
        assert_eq!(link("uniray://reboot"), None);
        assert_eq!(link("https://connect"), None);
    }

    #[test]
    fn import_takes_only_web_subscriptions() {
        assert_eq!(
            link("uniray://import-sub?url=https%3A%2F%2Fexample.com%2Fsub%3Fa%3D1"),
            Some(DeepLink::Import { url: "https://example.com/sub?a=1".into() })
        );
        assert_eq!(link("uniray://import-sub?url=javascript:alert(1)"), None);
        assert_eq!(link("uniray://import-sub?url=file:///etc/passwd"), None);
        assert_eq!(link("uniray://import-sub"), None);
        // Quotes stay data: nothing here reaches a script.
        assert_eq!(
            link("uniray://import-sub?url=https://e.com/'%29;alert(1)//"),
            Some(DeepLink::Import { url: "https://e.com/');alert(1)//".into() })
        );
    }
}
