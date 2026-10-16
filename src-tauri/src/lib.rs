mod app;
mod commands;
mod core;
mod deeplink;
mod device;
mod error;
mod handoff;
mod logs;
mod monitor;
mod network;
mod state;
mod stats;
mod subscriptions;
#[cfg(desktop)]
mod sysproxy;
#[cfg(desktop)]
mod tray;

use tauri::Manager;
#[cfg(desktop)]
use tauri::RunEvent;

use crate::app::{App, Paths};
use crate::state::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        // A second launch brings the running instance forward instead of
        // fighting it over the ports and the system proxy; with its
        // `deep-link` feature it hands a uniray:// link it was started with
        // (Windows, Linux) to the running one.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }));
    }
    #[cfg(mobile)]
    {
        builder = builder.plugin(core::plugin());
    }
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let mut store = Store::load(data_dir.join("state.json"));
            let device = device::detect(&mut store, &app.package_info().version.to_string());
            #[cfg(desktop)]
            let mixed_port = store.data.settings.mixed_port;
            let paths = Paths {
                run_dir: data_dir.join("run"),
                #[cfg(desktop)]
                assets: app.path().resolve("resources", tauri::path::BaseDirectory::Resource)?,
                #[cfg(desktop)]
                xray: sidecar("uniray-xray")?,
                #[cfg(desktop)]
                helper: sidecar("uniray-helper")?,
                #[cfg(desktop)]
                engine: sidecar("uniray-engine")?,
                #[cfg(desktop)]
                naive: sidecar("uniray-naive")?,
            };
            app.manage(App::new(store, device, paths));
            deeplink::install(&handle);

            #[cfg(desktop)]
            {
                if let Err(err) = tray::create(&handle) {
                    eprintln!("uniray: no tray icon: {err}");
                }
                tauri::async_runtime::spawn(sysproxy::reset_if_stale(mixed_port));
            }
            #[cfg(desktop)]
            subscriptions::spawn_auto_update(handle);
            #[cfg(mobile)]
            tauri::async_runtime::spawn(async move {
                // Subscription requests carry the device's HWID, so learn it first.
                core::init_device(&handle).await;
                subscriptions::spawn_auto_update(handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                window.app_handle().state::<App>().visible.store(*focused, std::sync::atomic::Ordering::Relaxed);
            }
            // Closing the window keeps the connection up; quitting is explicit
            // (tray menu or the button in settings).
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            deeplink::take_deep_links,
            commands::import,
            commands::delete_servers,
            commands::select_server,
            commands::add_subscription,
            commands::update_subscription,
            commands::update_all_subscriptions,
            commands::delete_subscription,
            commands::save_settings,
            commands::connect,
            commands::disconnect,
            commands::test_delays,
            commands::check_connection,
            commands::list_apps,
            commands::system_colors,
            commands::privilege_state,
            commands::request_shizuku,
            commands::set_keep_awake,
            commands::handoff_start,
            commands::handoff_stop,
            commands::get_logs,
            commands::clear_logs,
            commands::export_backup,
            commands::import_backup,
            commands::probe_service,
            commands::speed_test_download,
            commands::speed_test_upload,
            commands::speed_test_multistream_download,
            commands::speed_test_multistream_upload,
            commands::speed_test_probe_latency,
            commands::get_underlying_network,
            commands::run_dpi_probe,
            commands::apply_dpi_fixes,
            commands::quit,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // On desktop, stop the core and restore the system proxy before
            // leaving. On mobile the VPN service outlives the activity.
            #[cfg(desktop)]
            if let RunEvent::Exit = event {
                tauri::async_runtime::block_on(core::disconnect(app));
            }
            #[cfg(mobile)]
            let _ = (app, event);
        });
}

/// Path of a bundled sidecar binary: it sits next to the app executable.
#[cfg(desktop)]
fn sidecar(name: &str) -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| std::io::Error::other("the executable has no parent directory"))?;
    Ok(dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
}
