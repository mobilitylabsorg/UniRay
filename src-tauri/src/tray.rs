//! Tray icon (desktop): the connection at a glance and at hand, and the way
//! back to a hidden window.
//!
//! The menu opens the window, shows the current server (greyed out, as a
//! label), connects or disconnects, switches servers and quits. It follows the
//! app's state and its language.

use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::app::App;
use crate::core::{self, Status};

const SERVER_PREFIX: &str = "server:";

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
                (Some(false), _) => format!(" · {}", texts(app).no_connection),
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
    let view = View::of(app);
    let menu = build_menu(app, &view)?;
    *LAST_VIEW.lock().unwrap() = Some(view);

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("UniRay")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            match id {
                "show" => show_main_window(app),
                "toggle" => toggle_connection(app.clone()),
                "quit" => app.exit(0),
                _ => {
                    if let Some(server) = id.strip_prefix(SERVER_PREFIX) {
                        select_server(app.clone(), server.to_owned());
                    }
                }
            }
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

/// Rebuilds the menu when something it shows has changed. Called on every
/// state change; most (delay test results, traffic) leave the menu as it is.
pub fn refresh(app: &AppHandle) {
    let view = View::of(app);
    {
        let mut last = LAST_VIEW.lock().unwrap();
        if last.as_ref() == Some(&view) {
            return;
        }
        *last = Some(view.clone());
    }
    let Some(tray) = app.tray_by_id("main") else { return };
    match build_menu(app, &view) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => eprintln!("uniray: cannot rebuild the tray menu: {err}"),
    }
}

/// The last menu built, to skip rebuilding an identical one.
static LAST_VIEW: Mutex<Option<View>> = Mutex::new(None);

/// What the menu shows.
#[derive(Clone, Debug, PartialEq)]
struct View {
    russian: bool,
    status: Status,
    /// The server the connection uses, else the selected one.
    current: Option<String>,
    selected: Option<String>,
    groups: Vec<Group>,
}

#[derive(Clone, Debug, PartialEq)]
struct Group {
    /// `None` for the servers added by hand.
    name: Option<String>,
    servers: Vec<(String, String)>,
}

impl View {
    fn of(app: &AppHandle) -> Self {
        let state = app.state::<App>();
        let connection = state.connection.lock().unwrap().clone();
        let store = state.store.lock().unwrap();
        let data = &store.data;
        let selected = store.selected_server().map(|server| server.id.clone());
        let active = connection.server_id.as_deref().or(selected.as_deref());
        let current = active
            .and_then(|id| data.servers.iter().find(|server| server.id == id))
            .map(|server| server.name.clone());

        let entries = |subscription: Option<&str>| {
            data.servers
                .iter()
                .filter(|server| server.subscription_id.as_deref() == subscription)
                .map(|server| (server.id.clone(), server.name.clone()))
                .collect::<Vec<_>>()
        };
        let mut groups = vec![Group { name: None, servers: entries(None) }];
        groups.extend(
            data.subscriptions
                .iter()
                .map(|subscription| Group { name: Some(subscription.name.clone()), servers: entries(Some(&subscription.id)) }),
        );
        groups.retain(|group| !group.servers.is_empty());

        Self {
            russian: is_russian(data.settings.locale.as_deref()),
            status: connection.status,
            current,
            selected,
            groups,
        }
    }
}

fn build_menu(app: &AppHandle, view: &View) -> tauri::Result<Menu<Wry>> {
    let text = if view.russian { &RU } else { &EN };

    let show = MenuItem::with_id(app, "show", text.open, true, None::<&str>)?;
    let current = MenuItem::with_id(
        app,
        "current",
        view.current.as_deref().unwrap_or(text.no_server),
        false,
        None::<&str>,
    )?;
    let (toggle_label, toggle_enabled) = match view.status {
        Status::Disconnected => (text.connect, view.current.is_some()),
        Status::Connected => (text.disconnect, true),
        Status::Connecting => (text.connecting, false),
        Status::Disconnecting => (text.disconnecting, false),
    };
    let toggle = MenuItem::with_id(app, "toggle", toggle_label, toggle_enabled, None::<&str>)?;
    let servers = servers_menu(app, view, text)?;
    let quit = MenuItem::with_id(app, "quit", text.quit, true, None::<&str>)?;
    let separator = || PredefinedMenuItem::separator(app);

    Menu::with_items(
        app,
        &[&show, &separator()?, &current, &toggle, &servers, &separator()?, &quit],
    )
}

/// "Switch server": the servers, grouped by subscription when there is more
/// than one group, the selected one checked.
fn servers_menu(app: &AppHandle, view: &View, text: &Texts) -> tauri::Result<Submenu<Wry>> {
    let item = |(id, name): &(String, String)| {
        let checked = view.selected.as_deref() == Some(id.as_str());
        CheckMenuItem::with_id(app, format!("{SERVER_PREFIX}{id}"), name, true, checked, None::<&str>)
    };
    let busy = matches!(view.status, Status::Connecting | Status::Disconnecting);
    let enabled = !busy && !view.groups.is_empty();

    if let [group] = view.groups.as_slice() {
        let items = group.servers.iter().map(item).collect::<tauri::Result<Vec<_>>>()?;
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item as &dyn IsMenuItem<Wry>).collect();
        return Submenu::with_id_and_items(app, "servers", text.switch_server, enabled, &refs);
    }
    let mut submenus = Vec::new();
    for (index, group) in view.groups.iter().enumerate() {
        let items = group.servers.iter().map(item).collect::<tauri::Result<Vec<_>>>()?;
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item as &dyn IsMenuItem<Wry>).collect();
        let name = group.name.as_deref().unwrap_or(text.manual);
        submenus.push(Submenu::with_id_and_items(app, format!("group:{index}"), name, true, &refs)?);
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = submenus.iter().map(|menu| menu as &dyn IsMenuItem<Wry>).collect();
    Submenu::with_id_and_items(app, "servers", text.switch_server, enabled, &refs)
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

/// Selects `id`, as the server list does: a live connection switches to it.
fn select_server(app: AppHandle, id: String) {
    tauri::async_runtime::spawn(async move {
        // A failed reconnect is recorded in the connection state.
        let _ = crate::commands::select_server(app.clone(), id).await;
        // The check mark toggles on its own when clicked; put it back in
        // line with the selection even when nothing changed.
        *LAST_VIEW.lock().unwrap() = None;
        refresh(&app);
    });
}

/// The language the window uses: the setting, else the system's.
fn is_russian(preference: Option<&str>) -> bool {
    match preference {
        Some("ru") => true,
        Some("en") => false,
        _ => sys_locale::get_locale().is_some_and(|locale| locale.to_lowercase().starts_with("ru")),
    }
}

fn texts(app: &AppHandle) -> &'static Texts {
    let preference = app.state::<App>().store.lock().unwrap().data.settings.locale.clone();
    if is_russian(preference.as_deref()) { &RU } else { &EN }
}

struct Texts {
    open: &'static str,
    no_server: &'static str,
    connect: &'static str,
    disconnect: &'static str,
    connecting: &'static str,
    disconnecting: &'static str,
    switch_server: &'static str,
    manual: &'static str,
    quit: &'static str,
    no_connection: &'static str,
}

const EN: Texts = Texts {
    open: "Open window",
    no_server: "No server selected",
    connect: "Connect",
    disconnect: "Disconnect",
    connecting: "Connecting…",
    disconnecting: "Disconnecting…",
    switch_server: "Switch server",
    manual: "Added manually",
    quit: "Quit",
    no_connection: "no connection",
};

const RU: Texts = Texts {
    open: "Открыть окно",
    no_server: "Сервер не выбран",
    connect: "Подключить",
    disconnect: "Отключить",
    connecting: "Подключение…",
    disconnecting: "Отключение…",
    switch_server: "Сменить сервер",
    manual: "Добавленные вручную",
    quit: "Выйти",
    no_connection: "нет соединения",
};
