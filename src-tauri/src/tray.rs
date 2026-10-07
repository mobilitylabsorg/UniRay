//! Tray icon (desktop): the way back to a hidden window, and out of the app.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::app::App;
use crate::core::{self, Status};

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Server, speed and latency in the tray tooltip; `None` when disconnected.
pub fn set_summary(app: &AppHandle, summary: Option<&crate::monitor::Summary>) {
    let Some(tray) = app.tray_by_id("main") else { return };
    let text = match summary {
        None => "UniRay".to_owned(),
        Some(summary) => {
            let latency = match (summary.ok, summary.latency_ms) {
                (Some(true), Some(ms)) => format!(" · {ms} ms"),
                (Some(false), _) => " · no connection".to_owned(),
                _ => String::new(),
            };
            format!(
                "UniRay · {}\n↓ {}  ↑ {}{latency}",
                summary.server,
                crate::monitor::format_speed(summary.down),
                crate::monitor::format_speed(summary.up),
            )
        }
    };
    let _ = tray.set_tooltip(Some(text));
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "UniRay", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Connect / Disconnect", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("UniRay")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "toggle" => toggle_connection(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn toggle_connection(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let status = app.state::<App>().connection.lock().unwrap().status;
        match status {
            // A failure is recorded in the connection state and shown in the window.
            Status::Disconnected => drop(core::connect(&app).await),
            Status::Connected => core::disconnect(&app).await,
            Status::Connecting | Status::Disconnecting => {}
        }
    });
}
