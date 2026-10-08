//! Writes the Xray and engine configs the app would for a WireGuard or
//! AmneziaWG `.conf`, for the engine's chain test (`engine/e2e/e2e.sh`):
//!
//! ```text
//! engine_configs <server.conf> <out dir> <mixed port> <engine port> [tun]
//! ```
//!
//! Everything goes through the server (the Global preset), with a TUN named
//! `uniray0` when `tun` is given. The engine is told no peers: the helper
//! hands it Xray's pid.

use std::path::Path;

use uniray_core::config::{build, LogSink, Runtime, Tun};
use uniray_core::engine::{prepare, Peers};
use uniray_core::model::{RoutingPreset, Settings};
use uniray_core::wireguard::server_from_conf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [conf, out, mixed_port, engine_port, rest @ ..] = args.as_slice() else {
        eprintln!("usage: engine_configs <server.conf> <out dir> <mixed port> <engine port> [tun]");
        std::process::exit(2);
    };
    let server = server_from_conf(&std::fs::read_to_string(conf).expect("read the .conf")).expect("parse the .conf");
    let settings = Settings { routing_preset: RoutingPreset::Global, ..Default::default() };
    let run = prepare(&server, engine_port.parse().expect("engine port"), false)
        .expect("prepare the engine")
        .expect("a server for the engine");
    let runtime = Runtime {
        tun: rest.first().is_some_and(|mode| mode == "tun").then(|| Tun::Managed { name: "uniray0".into() }),
        mixed_port: Some(mixed_port.parse().expect("mixed port")),
        metrics_port: None,
        metrics_socket: None,
        log: LogSink::Stdout,
        engine: Some(run.runtime.clone()),
    };
    let built = build(&server, &settings, &runtime).expect("build the Xray config");
    let out = Path::new(out);
    std::fs::write(out.join("xray.json"), serde_json::to_vec_pretty(&built.config).unwrap()).unwrap();
    std::fs::write(out.join("engine.json"), run.config_for(&Peers::Pids(Vec::new()))).unwrap();
}
