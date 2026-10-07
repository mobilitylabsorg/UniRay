//! End to end through real Xray processes, entirely on loopback:
//!
//! ```text
//! test --HTTP proxy--> client xray --VLESS--> server xray --> local HTTP server
//!                      (our config)
//! ```
//!
//! Proves that a config generated from a share link carries traffic and that
//! the traffic counters appear where the app reads them. Skipped without
//! `UNIRAY_XRAY_BIN`, see `xray_validate.rs`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uniray_core::config::{build, LogSink, Runtime};
use uniray_core::links::parse_link;
use uniray_core::model::{RoutingPreset, Settings};

const UUID: &str = "5783a3e7-e373-51cd-8642-c83782b807c5";
const BODY: &str = "hello through the tunnel";

struct Xray(Child);

impl Drop for Xray {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
}

fn start_xray(xray: &str, dir: &Path, name: &str, config: &Value) -> Xray {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(config).unwrap()).unwrap();
    let child = Command::new(xray)
        .arg("run")
        .arg("-c")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start xray");
    Xray(child)
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
        assert!(Instant::now() < deadline, "port {port} did not open");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Serves `BODY` to every connection.
fn serve_http() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                BODY.len()
            );
        }
    });
    port
}

/// `GET url` through an HTTP proxy (or directly when `via` is the origin).
fn http_get(via: u16, url: &str, host: &str) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, via)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(stream, "GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn traffic_flows_and_is_counted() {
    let Ok(xray) = std::env::var("UNIRAY_XRAY_BIN") else {
        eprintln!("UNIRAY_XRAY_BIN is not set, skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("uniray-xray-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let origin_port = serve_http();
    let (server_port, mixed_port, metrics_port) = (free_port(), free_port(), free_port());

    let _server = start_xray(
        &xray,
        &dir,
        "server.json",
        &json!({
            "log": { "loglevel": "none" },
            "inbounds": [{
                "listen": "127.0.0.1",
                "port": server_port,
                "protocol": "vless",
                "settings": { "clients": [{ "id": UUID }], "decryption": "none" },
            }],
            // A server-side freedom outbound refuses private targets unless
            // told otherwise, and the origin lives on loopback.
            "outbounds": [{
                "protocol": "freedom",
                "settings": { "finalRules": [{ "action": "allow", "ip": ["127.0.0.0/8"] }] },
            }],
        }),
    );
    wait_for_port(server_port);

    // Plain VLESS is allowed towards a private address, which loopback is.
    let server = parse_link(&format!("vless://{UUID}@127.0.0.1:{server_port}?type=tcp&security=none#local")).unwrap();
    // The origin is on loopback too; route it through the proxy regardless.
    let settings = Settings { routing_preset: RoutingPreset::Global, ..Default::default() };
    let runtime = Runtime {
        tun: None,
        mixed_port: Some(mixed_port),
        metrics_port: Some(metrics_port), metrics_socket: None,
        log: LogSink::Stdout,
    };
    let built = build(&server, &settings, &runtime).unwrap();
    let _client = start_xray(&xray, &dir, "client.json", &built.config);
    wait_for_port(mixed_port);
    wait_for_port(metrics_port);

    let origin = format!("127.0.0.1:{origin_port}");
    let response = http_get(mixed_port, &format!("http://{origin}/"), &origin);
    assert!(response.starts_with("HTTP/1.1 200"), "unexpected response: {response}");
    assert!(response.ends_with(BODY), "unexpected response: {response}");

    // Counters are flushed when the connection closes.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let metrics = http_get(metrics_port, "/debug/vars", &format!("127.0.0.1:{metrics_port}"));
        let body = metrics.split_once("\r\n\r\n").expect("metrics response body").1;
        let body = &body[body.find('{').expect("JSON in metrics body")..=body.rfind('}').unwrap()];
        let vars: Value = serde_json::from_str(body).expect("metrics JSON");
        let counters = &vars["stats"]["outbound"][built.proxy_tags[0].as_str()];
        // Only the downlink is asserted: the request rides along with the
        // VLESS handshake, which Xray does not count as uplink.
        if counters["downlink"].as_u64().unwrap_or(0) >= BODY.len() as u64 {
            break;
        }
        assert!(Instant::now() < deadline, "no traffic counted: {}", vars["stats"]);
        thread::sleep(Duration::from_millis(100));
    }

    std::fs::remove_dir_all(&dir).ok();
}
