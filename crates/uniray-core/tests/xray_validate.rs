//! Checks generated configs against a real Xray binary (`xray run -test`).
//!
//! Needs `UNIRAY_XRAY_BIN` (and `XRAY_LOCATION_ASSET` for the geo files);
//! without them the test is skipped, since the binary is not vendored.
//! `scripts/fetch-xray.sh` downloads it and prints both variables.

use std::path::Path;
use std::process::Command;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use uniray_core::config::{build, build_probe, LogSink, Runtime, Tun};
use uniray_core::links::parse_link;
use uniray_core::model::{new_id, RoutingPreset, Rule, RuleAction, RuleKind, Server, Settings};
use uniray_core::subscription::parse_response;

const UUID: &str = "5783a3e7-e373-51cd-8642-c83782b807c5";
const PBK: &str = "7xhH4b_VkliBxGulljcyPOH-bYUA2dl-XAdZAsfhk04";

fn links() -> Vec<String> {
    let vmess = json!({
        "v": "2", "ps": "vmess", "add": "vm.example.com", "port": "443", "id": UUID, "aid": "0", "scy": "auto",
        "net": "ws", "type": "none", "host": "vm.example.com", "path": "/v", "tls": "tls",
    });
    vec![
        format!("vless://{UUID}@example.com:443?type=tcp&security=reality&pbk={PBK}&fp=chrome&sni=www.microsoft.com&sid=6ba85179e30d4fc2&spx=%2F&flow=xtls-rprx-vision#reality"),
        format!("vless://{UUID}@203.0.113.7:443?type=ws&security=tls&sni=a.example.com&path=%2Fws&host=a.example.com&alpn=h2%2Chttp%2F1.1&allowInsecure=1#ws"),
        format!("vless://{UUID}@example.com:443?type=grpc&security=tls&sni=a.example.com&serviceName=svc&mode=multi#grpc"),
        format!("vless://{UUID}@example.com:443?type=xhttp&security=reality&pbk={PBK}&sni=a.example.com&path=%2Fx&mode=auto&extra=%7B%22xPaddingBytes%22%3A%22100-1000%22%7D#xhttp"),
        format!("vless://{UUID}@example.com:443?type=httpupgrade&security=tls&sni=a.example.com&path=%2Fu&host=a.example.com#httpupgrade"),
        // Xray refuses VLESS without transport security unless the server is private.
        format!("vless://{UUID}@10.0.0.1:80?type=tcp&headerType=http&host=a.example.com&path=%2F#raw-http"),
        format!("vmess://{}", STANDARD.encode(vmess.to_string())),
        "trojan://password@tr.example.com:443?sni=tr.example.com&fp=chrome#trojan".to_owned(),
        format!("ss://{}@ss.example.com:8388#ss", STANDARD.encode("aes-256-gcm:password")),
        "ss://2022-blake3-aes-128-gcm:MTIzNDU2Nzg5MDEyMzQ1Ng%3D%3D@ss.example.com:8388#ss2022".to_owned(),
        "hy2://secret@hy.example.com:443?sni=hy.example.com&obfs=salamander&obfs-password=mask#hysteria".to_owned(),
    ]
}

fn json_subscription_servers() -> Vec<Server> {
    // The legacy layout JSON subscriptions still use: `vnext`, `network`,
    // `publicKey`, plus the removed `allowInsecure`.
    let body = json!([{
        "remarks": "legacy json",
        "dns": { "servers": ["1.1.1.1"] },
        "inbounds": [{ "tag": "socks", "port": 10808, "protocol": "socks" }],
        "outbounds": [
            {
                "tag": "proxy",
                "protocol": "vless",
                "settings": { "vnext": [{ "address": "example.com", "port": 443, "users": [{ "id": UUID, "encryption": "none", "flow": "xtls-rprx-vision" }] }] },
                "streamSettings": {
                    "network": "tcp",
                    "security": "reality",
                    "realitySettings": { "serverName": "www.microsoft.com", "fingerprint": "chrome", "publicKey": PBK, "shortId": "6ba85179e30d4fc2" },
                },
            },
            {
                "tag": "fallback",
                "protocol": "trojan",
                "settings": { "servers": [{ "address": "tr.example.com", "port": 443, "password": "p" }] },
                "streamSettings": { "network": "ws", "security": "tls", "tlsSettings": { "serverName": "tr.example.com", "allowInsecure": true }, "wsSettings": { "path": "/" } },
            },
            { "tag": "direct", "protocol": "freedom" },
            { "tag": "block", "protocol": "blackhole" },
        ],
        "routing": { "domainStrategy": "IPIfNonMatch", "rules": [{ "ip": ["geoip:private"], "outboundTag": "direct" }] },
    }, {
        // A balancer group as panels send it: several outbounds, an observatory
        // feeding the leastPing strategy, and a rule on the provider's inbound.
        "remarks": "balancer",
        "inbounds": [{ "tag": "socks", "port": 10808, "protocol": "socks" }],
        "outbounds": [
            { "tag": "proxy-1", "protocol": "vless", "settings": { "address": "a.example.com", "port": 443, "id": UUID, "encryption": "none", "flow": "xtls-rprx-vision" },
              "streamSettings": { "network": "tcp", "security": "reality", "realitySettings": { "serverName": "www.microsoft.com", "fingerprint": "chrome", "publicKey": PBK, "shortId": "6ba85179e30d4fc2" } } },
            { "tag": "proxy-2", "protocol": "vless", "settings": { "address": "b.example.com", "port": 443, "id": UUID, "encryption": "none" },
              "streamSettings": { "network": "xhttp", "security": "tls", "tlsSettings": { "serverName": "b.example.com" }, "xhttpSettings": { "path": "/x" } } },
            { "tag": "direct", "protocol": "freedom" },
        ],
        "observatory": { "subjectSelector": ["proxy-"], "probeUrl": "https://www.gstatic.com/generate_204", "probeInterval": "1m", "enableConcurrency": true },
        "routing": {
            "balancers": [{ "tag": "auto", "selector": ["proxy-"], "strategy": { "type": "leastPing" }, "fallbackTag": "proxy-1" }],
            "rules": [{ "inboundTag": ["socks"], "balancerTag": "auto" }],
        },
    }])
    .to_string();
    parse_response(200, &Default::default(), &body, true).unwrap().servers
}

fn settings_variants() -> Vec<Settings> {
    let rule = |kind, value: &str, action| Rule { id: new_id(), kind, value: value.into(), action, enabled: true };
    vec![
        Settings::default(),
        Settings {
            routing_preset: RoutingPreset::Global,
            ipv6: true,
            allow_lan: true,
            proxy_auth: true,
            proxy_user: "user".into(),
            proxy_password: "pass".into(),
            ..Default::default()
        },
        Settings {
            routing_preset: RoutingPreset::BypassRu,
            remote_dns: "https://dns.google/dns-query".into(),
            direct_dns: "tcp://77.88.8.8:53".into(),
            log_level: "debug".into(),
            custom_rules: vec![
                rule(RuleKind::Domain, "ads.example.com", RuleAction::Block),
                rule(RuleKind::Domain, "geosite:google", RuleAction::Proxy),
                rule(RuleKind::Domain, "regexp:.*\\.bank\\.example$", RuleAction::Direct),
                rule(RuleKind::Ip, "10.10.0.0/16", RuleAction::Direct),
                rule(RuleKind::Ip, "geoip:cn", RuleAction::Block),
            ],
            ..Default::default()
        },
    ]
}

fn runtimes(dir: &Path) -> Vec<Runtime> {
    let log = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let files = LogSink::Files { error: log("error.log"), access: log("access.log") };
    vec![
        Runtime { tun: None, mixed_port: Some(28080), metrics_port: Some(28081), metrics_socket: None, log: LogSink::Stdout },
        Runtime { tun: Some(Tun::Managed { name: "uniraytest0".into() }), mixed_port: Some(28080), metrics_port: Some(28081), metrics_socket: None, log: LogSink::Stdout },
        Runtime { tun: Some(Tun::External { name: "rmnet3".into() }), mixed_port: None, metrics_port: None, metrics_socket: Some("@uniray-validate-metrics".into()), log: files },
        Runtime {
            tun: Some(Tun::Gateway { name: "uniray0".into(), routes: vec!["8.0.0.0/7".into(), "11.0.0.0/8".into(), "12.0.0.0/6".into()] }),
            mixed_port: Some(28080),
            metrics_port: Some(28081), metrics_socket: None,
            log: LogSink::Stdout,
        },
    ]
}

fn xray_accepts(xray: &str, dir: &Path, what: &str, config: &Value) {
    let path = dir.join("config.json");
    std::fs::write(&path, serde_json::to_vec_pretty(config).unwrap()).unwrap();
    let output = Command::new(xray).arg("run").arg("-test").arg("-c").arg(&path).output().expect("run xray");
    assert!(
        output.status.success(),
        "xray rejected the config for {what}:\n{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        serde_json::to_string_pretty(config).unwrap(),
    );
}

#[test]
fn xray_accepts_generated_configs() {
    let Ok(xray) = std::env::var("UNIRAY_XRAY_BIN") else {
        eprintln!("UNIRAY_XRAY_BIN is not set, skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("uniray-xray-validate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut servers: Vec<Server> = links().iter().map(|link| parse_link(link).unwrap()).collect();
    servers.extend(json_subscription_servers());
    assert_eq!(servers.len(), 13);

    let (settings, runtimes) = (settings_variants(), runtimes(&dir));
    let mut checked = 0;
    for server in &servers {
        for (settings_index, settings) in settings.iter().enumerate() {
            for (runtime_index, runtime) in runtimes.iter().enumerate() {
                let built = build(server, settings, runtime).unwrap();
                let what = format!("{} (settings #{settings_index}, runtime #{runtime_index})", server.name);
                xray_accepts(&xray, &dir, &what, &built.config);
                checked += 1;
            }
        }
    }

    let refs: Vec<&Server> = servers.iter().collect();
    let ports: Vec<u16> = (0..refs.len() as u16).map(|index| 29000 + index).collect();
    xray_accepts(&xray, &dir, "delay probe", &build_probe(&refs, &ports).unwrap());

    std::fs::remove_dir_all(&dir).ok();
    eprintln!("xray accepted {checked} configs and the probe config");
}
