//! uniray-gateway: headless UniRay for routers.
//!
//! Runs Xray as a gateway for the LAN (traffic the router sends to the
//! container goes through a TUN interface) plus a SOCKS/HTTP proxy, keeps the
//! subscription fresh, and serves a small web UI to switch servers. Built for
//! a MikroTik container with around 64 MB of memory: no async runtime, no geo
//! databases, Xray as the only big process (plus `uniray-engine`, and for
//! NaiveProxy `naive`, for the servers Xray cannot run).

mod app;
mod engine;
mod net;
mod options;
mod web;

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use app::App;
use options::Options;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn main() {
    let options = Options::from_env();
    // The App lives as long as the process; threads borrow it freely.
    let app: &'static App = Box::leak(Box::new(App::new(options)));
    app.log(format!("uniray-gateway {} starting, HWID {}", env!("CARGO_PKG_VERSION"), app.device.hwid));

    if app.options.subscriptions.is_empty() && app.options.links.is_empty() {
        app.log("uniray: no SUB or SRV set; add one in the container's environment");
    }
    if app.options.tun {
        for result in [net::enable_forwarding(), net::ensure_tun_device()] {
            if let Err(err) = result {
                app.log(format!("uniray: {err}"));
            }
        }
    }

    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
    // As PID 1, stop Xray cleanly (it removes its routes) before exiting.
    thread::spawn(move || {
        while !STOP.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
        }
        app.log("uniray: stopping");
        app.stop_core();
        std::process::exit(0);
    });

    // The first fetch goes out directly, before the tunnel exists.
    app.update_subscriptions();
    app.restart_core();
    app.spawn_supervisor();
    app.spawn_updater();

    if let Err(err) = web::serve(app) {
        app.log(format!("uniray: {err}"));
        // Without the UI the gateway still works; keep it running.
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
}
