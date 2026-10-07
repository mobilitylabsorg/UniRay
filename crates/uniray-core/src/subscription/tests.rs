use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::json;

use super::*;

const LINKS: &str = "vless://5783a3e7-e373-51cd-8642-c83782b807c5@a.example.com:443?security=tls#A\ntrojan://pw@b.example.com:443#B\n";

fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

fn names(parsed: &Parsed) -> Vec<&str> {
    parsed.servers.iter().map(|server| server.name.as_str()).collect()
}

#[test]
fn base64_and_plain_bodies() {
    let plain = parse_response(200, &HashMap::new(), LINKS, true).unwrap();
    assert_eq!(names(&plain), ["A", "B"]);

    let encoded = format!("{}\n", STANDARD.encode(LINKS));
    let decoded = parse_response(200, &HashMap::new(), &encoded, true).unwrap();
    assert_eq!(names(&decoded), ["A", "B"]);
    assert_eq!(decoded.meta, Meta::default());
}

#[test]
fn bad_entries_are_reported_but_do_not_fail_the_update() {
    let body = format!("{LINKS}vless://broken\n");
    let parsed = parse_response(200, &HashMap::new(), &body, true).unwrap();
    assert_eq!(names(&parsed), ["A", "B"]);
    assert_eq!(parsed.errors.len(), 1);
}

#[test]
fn metadata_headers() {
    let title = format!("base64:{}", STANDARD.encode("Мой VPN"));
    let headers = headers(&[
        ("profile-title", &title),
        ("profile-update-interval", "12"),
        ("subscription-userinfo", "upload=10; download=20; total=1073741824; expire=1767225600"),
        ("support-url", "https://t.me/support"),
        ("profile-web-page-url", "https://panel.example.com"),
        ("x-hwid-active", "true"),
    ]);
    let meta = parse_response(200, &headers, LINKS, true).unwrap().meta;
    assert_eq!(
        meta,
        Meta {
            title: Some("Мой VPN".into()),
            update_interval_hours: Some(12),
            user_info: Some(UserInfo { upload: 10, download: 20, total: 1073741824, expire: Some(1767225600) }),
            hwid_active: true,
            support_url: Some("https://t.me/support".into()),
            web_page_url: Some("https://panel.example.com".into()),
            announce: None,
        }
    );

    let mut subscription = Subscription {
        name: "old".into(),
        last_error: Some("boom".into()),
        ..Default::default()
    };
    meta.apply(&mut subscription, 1_800_000_000);
    assert_eq!(subscription.name, "Мой VPN");
    assert_eq!(subscription.updated_at, Some(1_800_000_000));
    assert!(subscription.hwid_active);
    assert_eq!(subscription.last_error, None);
}

#[test]
fn unlimited_never_expiring_user_info() {
    assert_eq!(
        parse_user_info("upload=0;download=5;total=0;expire=0"),
        UserInfo { upload: 0, download: 5, total: 0, expire: None }
    );
}

#[test]
fn hwid_limit_reached() {
    for header in ["x-hwid-max-devices-reached", "x-hwid-limit"] {
        let headers = headers(&[("x-hwid-active", "true"), (header, "true")]);
        // Whatever the status and body, the verdict header wins.
        for status in [200, 403, 404] {
            assert_eq!(parse_response(status, &headers, LINKS, true).unwrap_err(), Error::HwidLimitReached);
        }
    }
}

#[test]
fn hwid_required() {
    let headers = headers(&[("x-hwid-active", "true"), ("x-hwid-not-supported", "true")]);
    assert_eq!(parse_response(200, &headers, LINKS, false).unwrap_err(), Error::HwidRequired);
    // Remnawave answers a bare 404 when the header is missing.
    assert_eq!(parse_response(404, &HashMap::new(), "", false).unwrap_err(), Error::NotFoundWithoutHwid);
    assert_eq!(parse_response(404, &HashMap::new(), "", true).unwrap_err(), Error::Http(404));
}

#[test]
fn http_errors_and_empty_bodies() {
    assert_eq!(parse_response(500, &HashMap::new(), LINKS, true).unwrap_err(), Error::Http(500));
    assert_eq!(parse_response(200, &HashMap::new(), "", true).unwrap_err(), Error::EmptySubscription);
    assert_eq!(
        parse_response(200, &HashMap::new(), "<html>login</html>", true).unwrap_err(),
        Error::EmptySubscription
    );
}

#[test]
fn json_configs() {
    let body = json!([
        {
            "remarks": "🇩🇪 Frankfurt",
            "outbounds": [{
                "tag": "proxy",
                "protocol": "vless",
                "settings": { "vnext": [{ "address": "de.example.com", "port": 8443, "users": [{ "id": "x", "encryption": "none" }] }] },
                "streamSettings": { "network": "ws", "security": "tls", "tlsSettings": { "serverName": "de.example.com", "allowInsecure": true } },
            }],
        },
        { "remarks": "broken", "outbounds": [] },
    ])
    .to_string();
    let parsed = parse_response(200, &HashMap::new(), &body, true).unwrap();
    assert_eq!(parsed.errors.len(), 1);
    let server = &parsed.servers[0];
    assert_eq!(server.name, "🇩🇪 Frankfurt");
    assert_eq!((server.address.as_str(), server.port), ("de.example.com", 8443));
    assert_eq!((server.protocol.as_str(), server.transport.as_str(), server.security.as_str()), ("vless", "ws", "tls"));
    assert_eq!(server.warnings.len(), 1);
    let ServerSource::Config(config) = &server.source else { panic!("expected a config") };
    assert_eq!(
        config["outbounds"][0]["streamSettings"]["tlsSettings"],
        json!({ "serverName": "de.example.com" })
    );

    // A single config object and its base64 form work too.
    let single = json!({ "outbounds": [{ "protocol": "trojan", "settings": { "address": "t.example.com", "port": 443, "password": "p" } }] })
        .to_string();
    for body in [single.clone(), STANDARD.encode(&single)] {
        let parsed = parse_response(200, &HashMap::new(), &body, true).unwrap();
        assert_eq!(parsed.servers[0].name, "t.example.com:443");
        assert!(parsed.servers[0].warnings.is_empty());
    }
}

#[test]
fn reconcile_keeps_identity_of_surviving_servers() {
    let mut old = parse_body(LINKS).0;
    old[0].delay_ms = Some(120);
    old[1].delay_ms = Some(80);

    // A is unchanged, B got a new password (same name and endpoint), C is new.
    let body = "vless://5783a3e7-e373-51cd-8642-c83782b807c5@a.example.com:443?security=tls#A\ntrojan://new@b.example.com:443#B\ntrojan://pw@c.example.com:443#C\n";
    let new = reconcile(&old, parse_body(body).0, "sub-1");

    assert_eq!((new[0].id.as_str(), new[0].delay_ms), (old[0].id.as_str(), Some(120)));
    assert_eq!((new[1].id.as_str(), new[1].delay_ms), (old[1].id.as_str(), Some(80)));
    assert!(new[1].link.as_deref().unwrap().contains("new@"));
    assert!(old.iter().all(|server| server.id != new[2].id));
    assert_eq!(new[2].delay_ms, None);
    assert!(new.iter().all(|server| server.subscription_id.as_deref() == Some("sub-1")));
}

#[test]
fn reconcile_does_not_merge_duplicates() {
    let twice = format!("{LINKS}{LINKS}");
    let old = parse_body(LINKS).0;
    let new = reconcile(&old, parse_body(&twice).0, "sub-1");
    let ids: HashSet<&str> = new.iter().map(|server| server.id.as_str()).collect();
    assert_eq!(ids.len(), 4);
}

#[test]
fn balancer_configs_are_labelled_as_such() {
    let body = json!([{
        "remarks": "🌐 Обычные",
        "outbounds": [
            { "tag": "p1", "protocol": "vless", "settings": { "address": "a.example.com", "port": 443, "id": "x", "encryption": "none" } },
            { "tag": "p2", "protocol": "vless", "settings": { "address": "b.example.com", "port": 443, "id": "x", "encryption": "none" } },
        ],
        "routing": { "balancers": [{ "tag": "auto", "selector": ["p"] }], "rules": [{ "network": "tcp,udp", "balancerTag": "auto" }] },
    }])
    .to_string();
    let parsed = parse_response(200, &HashMap::new(), &body, true).unwrap();
    assert_eq!(parsed.servers[0].protocol, "balancer");
    assert_eq!(parsed.servers[0].name, "🌐 Обычные");
}
