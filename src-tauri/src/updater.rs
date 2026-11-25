//! New versions from the GitHub releases.
//!
//! The desktop builds use Tauri's updater: the release carries `latest.json`
//! with a signed package per platform (the AppImage, the NSIS installer, the
//! macOS app as .tar.gz), checked against the public key in tauri.conf.json.
//! A copy that cannot replace itself - the portable Windows zip, a Linux build
//! that is not an AppImage, an app run from its disk image - is told of the
//! new version and pointed at the release instead.
//!
//! Android has no such updater: the GitHub edition reads the latest release
//! and offers its APK for the device's architecture. The Play edition leaves
//! updates to Play.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app::App;
use crate::error::{CmdError, CmdResult};

pub const AVAILABLE_EVENT: &str = "update-available";
#[cfg(desktop)]
pub const PROGRESS_EVENT: &str = "update-progress";

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// The first check after start, then one every this often.
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);

/// A version newer than this one.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    /// The release's notes (the version's CHANGELOG.md section), Markdown.
    pub notes: String,
    pub date: Option<String>,
    /// The app can download and install it itself.
    pub install: bool,
    /// Where to get it by hand: the release page, or the APK on Android.
    pub url: String,
}

#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<UpdateInfo>>,
    #[cfg(desktop)]
    update: Mutex<Option<tauri_plugin_updater::Update>>,
}

#[cfg(desktop)]
#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

fn release_page(version: &str) -> String {
    format!("{REPOSITORY}/releases/tag/v{version}")
}

/// The last update a check found, if any.
#[tauri::command]
pub fn update_status(app: AppHandle) -> Option<UpdateInfo> {
    app.state::<Updates>().found.lock().unwrap().clone()
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    check(&app).await
}

async fn check(app: &AppHandle) -> CmdResult<Option<UpdateInfo>> {
    let found = find(app).await?;
    *app.state::<Updates>().found.lock().unwrap() = found.clone();
    Ok(found)
}

/// Checks soon after start and then twice a day, while the setting allows.
pub fn spawn_checks(app: AppHandle) {
    if cfg!(play_store) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let enabled = app.state::<App>().store.lock().unwrap().data.settings.check_updates;
            if enabled {
                match check(&app).await {
                    Ok(Some(info)) => {
                        let _ = app.emit(AVAILABLE_EVENT, info);
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("uniray: update check: {err}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// The local proxy while connected, so the check goes through the VPN as the
/// rest of the system's traffic does (GitHub may be slow or cut off without).
/// Xray's mixed inbound speaks HTTP too, which the updater's client can use.
#[cfg(desktop)]
fn local_proxy(app: &AppHandle) -> Option<tauri::Url> {
    let state = app.state::<App>();
    let port = state.connection.lock().unwrap().mixed_port?;
    let settings = state.store.lock().unwrap().data.settings.clone();
    let mut url = tauri::Url::parse("http://127.0.0.1").ok()?;
    url.set_port(Some(port)).ok()?;
    if settings.proxy_auth {
        url.set_username(&settings.proxy_user).ok()?;
        url.set_password(Some(&settings.proxy_password)).ok()?;
    }
    Some(url)
}

#[cfg(desktop)]
async fn find(app: &AppHandle) -> CmdResult<Option<UpdateInfo>> {
    use tauri_plugin_updater::UpdaterExt;

    let fail = |err: tauri_plugin_updater::Error| CmdError::new("update", err.to_string());
    let mut builder = app.updater_builder().timeout(Duration::from_secs(60));
    if let Some(proxy) = local_proxy(app) {
        builder = builder.proxy(proxy);
    }
    let Some(update) = builder.build().map_err(fail)?.check().await.map_err(fail)? else {
        *app.state::<Updates>().update.lock().unwrap() = None;
        return Ok(None);
    };
    let info = UpdateInfo {
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.to_string()),
        install: can_install(),
        url: release_page(&update.version),
    };
    *app.state::<Updates>().update.lock().unwrap() = Some(update);
    Ok(Some(info))
}

/// Whether this copy of the app can replace itself.
#[cfg(desktop)]
fn can_install() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    if cfg!(target_os = "linux") {
        // The updater replaces the AppImage file; nothing else is ours to replace.
        std::env::var_os("APPIMAGE").is_some()
    } else if cfg!(windows) {
        // The installer leaves its uninstaller next to the app; the portable
        // zip has none, and the installer would put a second copy elsewhere.
        exe.parent().is_some_and(|dir| dir.join("uninstall.exe").is_file())
    } else {
        // Not from the disk image, nor from the read-only copy macOS runs an
        // app from before it has been moved out of Downloads.
        let path = exe.to_string_lossy();
        !path.starts_with("/Volumes/") && !path.contains("/AppTranslocation/")
    }
}

/// Downloads the update found last, stops the connection and installs it,
/// then starts the new version. On Windows the installer takes over and this
/// process ends.
#[cfg(desktop)]
#[tauri::command]
pub async fn update_install(app: AppHandle) -> CmdResult<()> {
    let fail = |err: tauri_plugin_updater::Error| CmdError::new("update", err.to_string());
    let pending = app.state::<Updates>().update.lock().unwrap().clone();
    let update = match pending {
        Some(update) => update,
        None => {
            check(&app).await?;
            let found = app.state::<Updates>().update.lock().unwrap().clone();
            found.ok_or_else(|| CmdError::new("update", "no update to install"))?
        }
    };
    if !can_install() {
        return Err(CmdError::new("updateManual", "this copy of the app cannot update itself"));
    }
    let mut downloaded = 0u64;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                let _ = app.emit(PROGRESS_EVENT, Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(fail)?;
    // The VPN's processes hold the files the update replaces (on Windows), and
    // the system proxy must not point at a port nobody listens on.
    crate::core::disconnect(&app).await;
    update.install(bytes).map_err(fail)?;
    app.restart();
}

#[cfg(mobile)]
#[tauri::command]
pub async fn update_install(_app: AppHandle) -> CmdResult<()> {
    Err(CmdError::new("updateManual", "install the APK from the release"))
}

#[cfg(mobile)]
async fn find(app: &AppHandle) -> CmdResult<Option<UpdateInfo>> {
    if cfg!(play_store) {
        return Ok(None);
    }
    let current = app.package_info().version.to_string();
    let api = format!("{}/releases/latest", REPOSITORY.replace("https://github.com/", "https://api.github.com/repos/"));
    let release: serde_json::Value = crate::subscriptions::get_json(app, &api).await?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches('v').to_owned();
    if !newer(&version, &current) {
        return Ok(None);
    }
    let apk = match std::env::consts::ARCH {
        "aarch64" => "UniRay-android-arm64.apk",
        "arm" => "UniRay-android-armv7.apk",
        _ => "",
    };
    let url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|asset| !apk.is_empty() && asset["name"].as_str() == Some(apk))
        .and_then(|asset| asset["browser_download_url"].as_str())
        .map_or_else(|| release_page(&version), str::to_owned);
    Ok(Some(UpdateInfo {
        notes: changelog_part(release["body"].as_str().unwrap_or_default()),
        date: release["published_at"].as_str().map(str::to_owned),
        version,
        current,
        install: false,
        url,
    }))
}

/// The CHANGELOG.md section of a release's description: it sits between the
/// download buttons above and GitHub's list of changes below.
#[cfg_attr(not(mobile), allow(dead_code))]
fn changelog_part(body: &str) -> String {
    let start = body.find("### ").unwrap_or(0);
    let rest = &body[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].trim().to_owned()
}

/// `a` is a later version than `b` (dotted numbers, a leading `v` ignored).
#[cfg_attr(not(mobile), allow(dead_code))]
fn newer(a: &str, b: &str) -> bool {
    let parse = |version: &str| -> Vec<u64> {
        version
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map_while(|part| part.parse().ok())
            .collect()
    };
    parse(a) > parse(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.4.1", "0.4.0"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.4.0", "0.4.0"));
        assert!(!newer("0.3.9", "0.4.0"));
        assert!(!newer("", "0.4.0"));
    }

    #[test]
    fn takes_the_changelog_from_a_release() {
        let body = "## Скачать\n\n| | |\n| --- | --- |\n\n### Новое\n\n- Обновления.\n\n## What's Changed\n* x";
        assert_eq!(changelog_part(body), "### Новое\n\n- Обновления.");
    }
}
