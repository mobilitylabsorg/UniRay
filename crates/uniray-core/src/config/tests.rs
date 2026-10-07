use serde_json::json;

use super::*;
use crate::links::parse_link;
use crate::model::{new_id, RoutingPreset, Rule, RuleAction, RuleKind};

const VLESS: &str = "vless://5783a3e7-e373-51cd-8642-c83782b807c5@vpn.example.com:443?security=tls&sni=vpn.example.com#S";
const VLESS_IP: &str = "vless://5783a3e7-e373-51cd-8642-c83782b807c5@203.0.113.7:443?security=tls&sni=vpn.example.com#S";

fn proxy_runtime() -> Runtime {
    Runtime {
        tun: None,
        mixed_port: Some(10808),
        metrics_port: Some(10809),
        log: LogSink::Files { error: "/tmp/error.log".into(), access: "/tmp/access.log".into() },
    }
}

fn tags(config: &Value, section: &str) -> Vec<String> {
    config[section]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["tag"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn proxy_mode_config() {
    let server = parse_link(VLESS).unwrap();
    let built = build(&server, &Settings::default(), &proxy_runtime()).unwrap();
    let config = &built.config;

    assert_eq!(built.proxy_tags, ["proxy"]);
    assert_eq!(tags(config, "inbounds"), ["mixed"]);
    assert_eq!(tags(config, "outbounds"), ["proxy", "direct", "block"]);
    assert_eq!(config["inbounds"][0]["listen"], "127.0.0.1");
    assert_eq!(config["inbounds"][0]["port"], 10808);

    // The server's own domain is resolved by the direct DNS, through `direct`.
    assert_eq!(config["outbounds"][0]["streamSettings"]["sockopt"]["domainStrategy"], "UseIPv4");
    assert_eq!(
        config["dns"]["servers"],
        json!([
            { "address": "77.88.8.8", "domains": ["full:vpn.example.com"], "skipFallback": true },
            "https://1.1.1.1/dns-query",
        ])
    );
    assert_eq!(
        config["routing"]["rules"],
        json!([
            { "inboundTag": ["dns"], "ip": ["77.88.8.8"], "outboundTag": "direct" },
            { "inboundTag": ["dns"], "ip": ["1.1.1.1"], "outboundTag": "proxy" },
            { "domain": ["geosite:private"], "outboundTag": "direct" },
            { "ip": ["geoip:private"], "outboundTag": "direct" },
        ])
    );

    assert_eq!(
        config["log"],
        json!({ "loglevel": "warning", "error": "/tmp/error.log", "access": "/tmp/access.log" })
    );
    assert_eq!(config["metrics"]["listen"], "127.0.0.1:10809");
    assert_eq!(config["policy"]["system"]["statsOutboundDownlink"], true);
}

#[test]
fn ip_server_needs_no_dns_bootstrap() {
    let server = parse_link(VLESS_IP).unwrap();
    let settings = Settings { routing_preset: RoutingPreset::Global, ..Default::default() };
    let config = build(&server, &settings, &proxy_runtime()).unwrap().config;
    assert!(config["outbounds"][0]["streamSettings"].get("sockopt").is_none());
    assert_eq!(config["dns"]["servers"], json!(["https://1.1.1.1/dns-query"]));
}

#[test]
fn lan_and_ipv6_settings() {
    let server = parse_link(VLESS_IP).unwrap();
    let settings = Settings { allow_lan: true, ipv6: true, ..Default::default() };
    let config = build(&server, &settings, &proxy_runtime()).unwrap().config;
    assert_eq!(config["inbounds"][0]["listen"], "0.0.0.0");
    assert_eq!(config["dns"]["queryStrategy"], "UseIP");
    assert_eq!(config["outbounds"][1]["streamSettings"]["sockopt"]["domainStrategy"], "UseIP");
}

#[test]
fn bypass_ru_resolves_ru_domains_directly() {
    let server = parse_link(VLESS_IP).unwrap();
    let settings = Settings {
        routing_preset: RoutingPreset::BypassRu,
        custom_rules: vec![Rule {
            id: new_id(),
            kind: RuleKind::Domain,
            value: "bank.example".into(),
            action: RuleAction::Direct,
            enabled: true,
        }],
        ..Default::default()
    };
    let config = build(&server, &settings, &proxy_runtime()).unwrap().config;
    assert_eq!(
        config["dns"]["servers"][0]["domains"],
        json!(["domain:bank.example", "geosite:category-ru", "domain:ru", "domain:su", "domain:xn--p1ai"])
    );
    let rules = config["routing"]["rules"].as_array().unwrap();
    assert_eq!(rules[2], json!({ "domain": ["domain:bank.example"], "outboundTag": "direct" }));
    assert_eq!(rules.last().unwrap(), &json!({ "ip": ["geoip:ru"], "outboundTag": "direct" }));
}

#[test]
fn managed_tun_hijacks_dns_and_routes_everything() {
    let server = parse_link(VLESS).unwrap();
    let runtime = Runtime { tun: Some(Tun::Managed { name: "uniray0".into() }), ..proxy_runtime() };
    let config = build(&server, &Settings::default(), &runtime).unwrap().config;

    assert_eq!(tags(&config, "inbounds"), ["mixed", "tun"]);
    assert_eq!(tags(&config, "outbounds"), ["proxy", "direct", "block", "dns-out"]);
    let tun = &config["inbounds"][1]["settings"];
    assert_eq!(tun["name"], "uniray0");
    assert_eq!(tun["autoSystemRoutingTable"], json!(["0.0.0.0/0", "::/0"]));
    assert_eq!(tun["autoOutboundsInterface"], "auto");
    assert_eq!(
        config["routing"]["rules"][0],
        json!({ "inboundTag": ["tun"], "port": "53", "outboundTag": "dns-out" })
    );
}

#[test]
fn external_tun_leaves_the_interface_to_the_os() {
    let server = parse_link(VLESS).unwrap();
    let runtime = Runtime { tun: Some(Tun::External { name: "rmnet3".into() }), mixed_port: None, log: LogSink::Stdout, ..proxy_runtime() };
    let config = build(&server, &Settings::default(), &runtime).unwrap().config;
    assert_eq!(tags(&config, "inbounds"), ["tun"]);
    // Logging to stdout is Xray's default: no paths in the config.
    assert_eq!(config["log"], json!({ "loglevel": "warning" }));
    // An explicit name keeps Xray from enumerating interfaces to pick one.
    assert_eq!(config["inbounds"][0]["settings"], json!({ "name": "rmnet3", "mtu": 1500 }));
}

#[test]
fn dns_server_hosts() {
    assert_eq!(dns_host("1.1.1.1"), "1.1.1.1");
    assert_eq!(dns_host("tcp://8.8.8.8:53"), "8.8.8.8");
    assert_eq!(dns_host("https://dns.example/dns-query"), "dns.example");
    assert_eq!(dns_host("https://[2606:4700:4700::1111]/dns-query"), "2606:4700:4700::1111");
    assert_eq!(dns_host("2606:4700:4700::1111"), "2606:4700:4700::1111");
    assert_eq!(
        dns_server_rule("https://dns.example/dns-query", "proxy"),
        json!({ "inboundTag": ["dns"], "domain": ["full:dns.example"], "outboundTag": "proxy" })
    );
}

fn config_server(config: Value) -> Server {
    Server {
        id: new_id(),
        name: "custom".into(),
        protocol: "vless".into(),
        address: "vpn.example.com".into(),
        port: 443,
        transport: "raw".into(),
        security: "tls".into(),
        source: ServerSource::Config(config),
        link: None,
        subscription_id: None,
        delay_ms: None,
        warnings: Vec::new(),
    }
}

#[test]
fn full_config_keeps_provider_routing() {
    let source = json!({
        "remarks": "custom",
        "api": { "tag": "api" },
        "inbounds": [{ "tag": "socks", "port": 1080, "protocol": "socks" }],
        "outbounds": [
            { "tag": "main", "protocol": "vless", "settings": { "address": "vpn.example.com", "port": 443, "id": "x", "encryption": "none" } },
            { "protocol": "trojan", "settings": { "address": "b.example.com", "port": 443, "password": "p" } },
            { "tag": "direct", "protocol": "freedom" },
        ],
        "routing": { "rules": [{ "domain": ["geosite:google"], "outboundTag": "main" }] },
        "policy": { "levels": { "0": { "handshake": 4 } } },
    });
    let runtime = Runtime { tun: Some(Tun::External { name: "rmnet3".into() }), ..proxy_runtime() };
    let built = build(&config_server(source), &Settings::default(), &runtime).unwrap();
    let config = &built.config;

    assert_eq!(built.proxy_tags, ["main", "out-1"]);
    assert_eq!(tags(config, "inbounds"), ["mixed", "tun"]);
    assert_eq!(tags(config, "outbounds"), ["main", "out-1", "direct", "dns-out"]);
    assert!(config.get("api").is_none());
    assert_eq!(
        config["routing"]["rules"],
        json!([
            { "inboundTag": ["tun"], "port": "53", "outboundTag": "dns-out" },
            // The core resolves the servers through the direct DNS, directly.
            { "inboundTag": ["dns"], "ip": ["77.88.8.8"], "outboundTag": "direct" },
            { "domain": ["geosite:google"], "outboundTag": "main" },
        ])
    );
    assert_eq!(
        config["dns"]["servers"][0],
        json!({ "address": "77.88.8.8", "domains": ["full:vpn.example.com", "full:b.example.com"], "skipFallback": true })
    );
    assert_eq!(config["dns"]["tag"], "dns");
    assert_eq!(config["outbounds"][0]["streamSettings"]["sockopt"]["domainStrategy"], "UseIPv4");
    assert_eq!(config["outbounds"][1]["streamSettings"]["sockopt"]["domainStrategy"], "UseIPv4");
    // Not a proxy, not touched.
    assert!(config["outbounds"][2].get("streamSettings").is_none());
    // Existing policy is extended, not replaced.
    assert_eq!(config["policy"]["levels"]["0"]["handshake"], 4);
    assert_eq!(config["policy"]["system"]["statsOutboundUplink"], true);
}

#[test]
fn full_config_without_outbounds_is_rejected() {
    let err = build(&config_server(json!({ "outbounds": [] })), &Settings::default(), &proxy_runtime()).unwrap_err();
    assert_eq!(err, Error::InvalidConfig("no outbounds".into()));
}

#[test]
fn probe_config_wires_each_server_to_its_port() {
    let (a, b) = (parse_link(VLESS).unwrap(), parse_link(VLESS_IP).unwrap());
    let config = build_probe(&[&a, &b], &[20001, 20002]).unwrap();
    assert_eq!(tags(&config, "inbounds"), ["in-0", "in-1"]);
    assert_eq!(tags(&config, "outbounds"), ["out-0", "out-1"]);
    assert_eq!(config["inbounds"][1]["port"], 20002);
    assert_eq!(config["outbounds"][1]["settings"]["address"], "203.0.113.7");
    assert_eq!(config["routing"]["rules"][1], json!({ "inboundTag": ["in-1"], "outboundTag": "out-1" }));
}

/// The shape Remnawave and similar panels use for balancer groups.
fn balancer_config() -> Value {
    json!({
        "remarks": "🌐 Balanced",
        "inbounds": [
            { "tag": "socks", "port": 10808, "protocol": "socks" },
            { "tag": "http", "port": 10809, "protocol": "http" },
        ],
        "outbounds": [
            { "tag": "proxy-1", "protocol": "vless", "settings": { "address": "a.example.com", "port": 443, "id": "x", "encryption": "none" } },
            { "tag": "proxy-2", "protocol": "vless", "settings": { "address": "b.example.com", "port": 443, "id": "x", "encryption": "none" } },
            { "tag": "direct", "protocol": "freedom" },
        ],
        "observatory": { "subjectSelector": ["proxy-"], "probeUrl": "https://www.gstatic.com/generate_204", "probeInterval": "1m" },
        "routing": {
            "balancers": [{ "tag": "auto", "selector": ["proxy-"], "strategy": { "type": "leastPing" }, "fallbackTag": "proxy-1" }],
            "rules": [
                { "inboundTag": ["api"], "outboundTag": "api" },
                { "inboundTag": ["socks", "http"], "balancerTag": "auto" },
            ],
        },
        "api": { "tag": "api", "services": ["StatsService"] },
    })
}

#[test]
fn balancer_config_keeps_observatory_and_follows_our_inbounds() {
    let server = Server { protocol: "balancer".into(), ..config_server(balancer_config()) };
    let runtime = Runtime { tun: Some(Tun::Managed { name: "rmnet3".into() }), ..proxy_runtime() };
    let config = build(&server, &Settings::default(), &runtime).unwrap().config;

    // The balancer needs the observatory's measurements to pick a server.
    assert_eq!(config["observatory"]["subjectSelector"], json!(["proxy-"]));
    assert_eq!(config["routing"]["balancers"][0]["tag"], "auto");
    assert!(config.get("api").is_none());

    let rules = config["routing"]["rules"].as_array().unwrap();
    // [0] answers DNS from the tunnel, [1] resolves the servers directly.
    assert_eq!(rules[1]["outboundTag"], "direct");
    // The provider's inbounds are gone; the rule now takes traffic from ours.
    assert_eq!(rules[3], json!({ "inboundTag": ["mixed", "tun"], "balancerTag": "auto" }));
    // Tags that are not the provider's inbounds are left alone.
    assert_eq!(rules[2], json!({ "inboundTag": ["api"], "outboundTag": "api" }));
}

#[test]
fn gateway_tun_routes_only_the_given_prefixes() {
    let server = parse_link(VLESS).unwrap();
    let routes = vec!["8.0.0.0/7".to_owned(), "11.0.0.0/8".to_owned()];
    let runtime = Runtime { tun: Some(Tun::Gateway { name: "uniray0".into(), routes: routes.clone() }), ..proxy_runtime() };
    let config = build(&server, &Settings::default(), &runtime).unwrap().config;
    let tun = &config["inbounds"][1]["settings"];
    assert_eq!(tun["autoSystemRoutingTable"], json!(routes));
    assert_eq!(tun["gateway"], json!([TUN_ADDRESS_V4]));
    assert_eq!(tun["autoOutboundsInterface"], "auto");
    // DNS arriving through the tunnel is still answered by the core.
    assert_eq!(config["routing"]["rules"][0]["outboundTag"], "dns-out");
}

#[test]
fn proxy_everything_drops_split_routing_and_geodata() {
    let mut config = json!({
        "outbounds": [
            { "tag": "proxy-1", "protocol": "vless" },
            { "tag": "direct", "protocol": "freedom" },
            { "tag": "block", "protocol": "blackhole" },
        ],
        "dns": {
            "hosts": { "dns.example": "1.1.1.1" },
            "servers": [
                { "address": "https://ai-dns.example/dns-query", "domains": ["geosite:category-ai-!cn"], "skipFallback": true },
                { "address": "77.88.8.8", "domains": ["domain:ya.ru"], "expectIPs": ["geoip:ru"] },
                { "address": "https://1.1.1.1/dns-query", "expectIPs": ["geoip:!private"] },
                "8.8.8.8",
            ],
        },
        "routing": { "rules": [
            { "protocol": ["bittorrent"], "outboundTag": "block" },
            { "domain": ["geosite:category-ads-all"], "outboundTag": "block" },
            { "ip": ["geoip:private"], "outboundTag": "direct" },
            { "domain": ["domain:vk.com", "domain:ya.ru"], "outboundTag": "direct" },
            { "domain": ["domain:netflix.com"], "outboundTag": "proxy-1" },
            { "network": "tcp,udp", "balancerTag": "auto" },
            { "network": "tcp", "outboundTag": "proxy-1" },
        ] },
    });
    proxy_everything(&mut config, "https://1.1.1.1/dns-query");
    assert_eq!(
        config["routing"]["rules"],
        json!([
            // The provider's policy stays; a geo-based block cannot.
            { "protocol": ["bittorrent"], "outboundTag": "block" },
            { "network": "tcp,udp", "balancerTag": "auto" },
            { "network": "tcp", "outboundTag": "proxy-1" },
        ])
    );
    assert_eq!(config["dns"]["servers"], json!([{ "address": "https://1.1.1.1/dns-query" }, "8.8.8.8"]));
    assert_eq!(config["dns"]["hosts"], json!({ "dns.example": "1.1.1.1" }));
    assert!(!config.to_string().contains("geoip:") && !config.to_string().contains("geosite:"));
}

#[test]
fn proxy_everything_falls_back_to_a_dns_server() {
    let mut config = json!({
        "outbounds": [{ "tag": "proxy", "protocol": "vless" }],
        "dns": { "servers": [{ "address": "77.88.8.8", "domains": ["geosite:category-ru"] }] },
    });
    proxy_everything(&mut config, "https://1.1.1.1/dns-query");
    assert_eq!(config["dns"]["servers"], json!(["https://1.1.1.1/dns-query"]));
}

#[test]
fn provider_servers_by_ip_need_no_bootstrap() {
    let source = json!({
        "outbounds": [{ "tag": "proxy", "protocol": "vless", "settings": { "address": "203.0.113.7", "port": 443, "id": "x", "encryption": "none" } }],
    });
    let built = build(&config_server(source), &Settings::default(), &proxy_runtime()).unwrap();
    assert!(built.config.get("dns").is_none());
    assert!(built.config["outbounds"][0].get("streamSettings").is_none());
}

#[test]
fn proxy_everything_never_falls_back_to_direct() {
    let mut config = json!({
        "outbounds": [
            { "tag": "direct", "protocol": "freedom" },
            { "tag": "proxy-de", "protocol": "vless" },
            { "tag": "proxy-fi", "protocol": "vless" },
        ],
        "routing": {
            "balancers": [
                { "tag": "auto", "selector": ["proxy"], "strategy": { "type": "leastLoad" }, "fallbackTag": "direct" },
                { "tag": "pinned", "selector": ["proxy-fi"], "fallbackTag": "proxy-fi" },
            ],
            "rules": [{ "network": "tcp,udp", "balancerTag": "auto" }],
        },
    });
    proxy_everything(&mut config, "https://1.1.1.1/dns-query");
    assert_eq!(config["routing"]["balancers"][0]["fallbackTag"], "proxy-de");
    // A fallback that already is a server stays.
    assert_eq!(config["routing"]["balancers"][1]["fallbackTag"], "proxy-fi");
}

#[test]
fn local_proxy_can_require_a_password() {
    let server = parse_link(VLESS_IP).unwrap();
    let open = build(&server, &Settings::default(), &proxy_runtime()).unwrap().config;
    assert_eq!(open["inbounds"][0]["settings"], json!({ "udp": true }));

    let settings = Settings {
        proxy_auth: true,
        proxy_user: "me".into(),
        proxy_password: "secret".into(),
        ..Default::default()
    };
    let locked = build(&server, &settings, &proxy_runtime()).unwrap().config;
    assert_eq!(
        locked["inbounds"][0]["settings"],
        json!({ "auth": "password", "accounts": [{ "user": "me", "pass": "secret" }], "udp": true })
    );
}

#[test]
fn every_install_gets_its_own_proxy_password() {
    let (a, b) = (Settings::default(), Settings::default());
    assert_eq!(a.proxy_password.len(), 16);
    assert_ne!(a.proxy_password, b.proxy_password);
    assert!(!a.proxy_auth);
}
