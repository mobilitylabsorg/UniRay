use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::json;

use super::*;

fn outbound(server: &Server) -> &Value {
    match &server.source {
        ServerSource::Outbound(outbound) => outbound,
        ServerSource::Config(_) => panic!("expected an outbound"),
    }
}

const UUID: &str = "5783a3e7-e373-51cd-8642-c83782b807c5";

#[test]
fn vless_reality_vision() {
    let link = format!(
        "vless://{UUID}@203.0.113.7:443?type=tcp&security=reality&pbk=7xhH4b_VkliBxGulljcyPOH-bYUA2dl-XAdZAsfhk04&fp=firefox&sni=www.microsoft.com&sid=6ba85179e30d4fc2&spx=%2Fabc&flow=xtls-rprx-vision#%F0%9F%87%B3%F0%9F%87%B1%20Amsterdam"
    );
    let server = parse_link(&link).unwrap();
    assert_eq!(server.name, "🇳🇱 Amsterdam");
    assert_eq!((server.protocol.as_str(), server.address.as_str(), server.port), ("vless", "203.0.113.7", 443));
    assert_eq!((server.transport.as_str(), server.security.as_str()), ("raw", "reality"));
    assert_eq!(server.link.as_deref(), Some(link.as_str()));
    assert!(server.warnings.is_empty());
    assert_eq!(
        outbound(&server),
        &json!({
            "protocol": "vless",
            "settings": {
                "address": "203.0.113.7",
                "port": 443,
                "id": UUID,
                "encryption": "none",
                "flow": "xtls-rprx-vision",
            },
            "streamSettings": {
                "method": "raw",
                "security": "reality",
                "realitySettings": {
                    "serverName": "www.microsoft.com",
                    "fingerprint": "firefox",
                    "password": "7xhH4b_VkliBxGulljcyPOH-bYUA2dl-XAdZAsfhk04",
                    "shortId": "6ba85179e30d4fc2",
                    "spiderX": "/abc",
                },
            },
        })
    );
}

#[test]
fn vless_ws_tls_with_stray_slash_and_ipv6() {
    let server = parse_link(&format!(
        "vless://{UUID}@[2001:db8::1]:8443/?type=ws&security=tls&path=%2Fws%3Fed%3D2048&host=cdn.example.com&alpn=h2%2Chttp%2F1.1"
    ))
    .unwrap();
    assert_eq!(server.address, "2001:db8::1");
    assert_eq!(server.name, "2001:db8::1:8443");
    let stream = &outbound(&server)["streamSettings"];
    assert_eq!(stream["method"], "websocket");
    assert_eq!(stream["wsSettings"], json!({ "path": "/ws?ed=2048", "host": "cdn.example.com" }));
    // No `sni`: falls back to the Host header.
    assert_eq!(stream["tlsSettings"], json!({ "serverName": "cdn.example.com", "alpn": ["h2", "http/1.1"] }));
}

#[test]
fn vless_xhttp_extra_and_grpc() {
    let server = parse_link(&format!(
        "vless://{UUID}@example.com:443?type=xhttp&security=tls&sni=example.com&path=%2Fx&mode=packet-up&extra=%7B%22xPaddingBytes%22%3A%22100-1000%22%7D#x"
    ))
    .unwrap();
    assert_eq!(
        outbound(&server)["streamSettings"]["xhttpSettings"],
        json!({ "path": "/x", "mode": "packet-up", "extra": { "xPaddingBytes": "100-1000" } })
    );

    let server = parse_link(&format!(
        "vless://{UUID}@example.com:443?type=grpc&security=tls&serviceName=svc&mode=multi&authority=a.example.com"
    ))
    .unwrap();
    assert_eq!(
        outbound(&server)["streamSettings"]["grpcSettings"],
        json!({ "serviceName": "svc", "authority": "a.example.com", "multiMode": true })
    );
}

#[test]
fn allow_insecure_is_dropped_with_warning() {
    let server = parse_link(&format!(
        "vless://{UUID}@example.com:443?security=tls&sni=example.com&allowInsecure=1&pcs=ABCDEF#x"
    ))
    .unwrap();
    let tls = &outbound(&server)["streamSettings"]["tlsSettings"];
    assert!(tls.get("allowInsecure").is_none());
    assert_eq!(tls["pinnedPeerCertSha256"], "ABCDEF");
    assert_eq!(server.warnings.len(), 1);
}

#[test]
fn vmess_base64_json() {
    let doc = json!({
        "v": "2", "ps": "vmess ws", "add": "vm.example.com", "port": "443", "id": UUID, "aid": "0",
        "scy": "chacha20-poly1305", "net": "ws", "type": "none", "host": "h.example.com", "path": "/v",
        "tls": "tls", "sni": "s.example.com", "alpn": "http/1.1", "fp": "chrome",
    });
    let server = parse_link(&format!("vmess://{}", STANDARD.encode(doc.to_string()))).unwrap();
    assert_eq!((server.name.as_str(), server.address.as_str(), server.port), ("vmess ws", "vm.example.com", 443));
    assert_eq!(
        outbound(&server),
        &json!({
            "protocol": "vmess",
            "settings": { "address": "vm.example.com", "port": 443, "id": UUID, "security": "chacha20-poly1305" },
            "streamSettings": {
                "method": "websocket",
                "wsSettings": { "path": "/v", "host": "h.example.com" },
                "security": "tls",
                "tlsSettings": { "serverName": "s.example.com", "fingerprint": "chrome", "alpn": ["http/1.1"] },
            },
        })
    );
}

#[test]
fn vmess_numeric_port_and_no_tls() {
    let doc = json!({ "ps": "", "add": "192.0.2.1", "port": 8080, "id": UUID, "net": "tcp", "tls": "" });
    let server = parse_link(&format!("vmess://{}", STANDARD.encode(doc.to_string()))).unwrap();
    assert_eq!(server.name, "192.0.2.1:8080");
    assert_eq!(outbound(&server)["settings"]["security"], "auto");
    assert_eq!(outbound(&server)["streamSettings"], json!({ "method": "raw", "security": "none" }));
}

#[test]
fn trojan_defaults_to_tls() {
    let server = parse_link("trojan://p%40ss%3Aword@tr.example.com:443?sni=tr.example.com#Trojan").unwrap();
    assert_eq!(server.security, "tls");
    assert_eq!(
        outbound(&server)["settings"],
        json!({ "address": "tr.example.com", "port": 443, "password": "p@ss:word" })
    );
}

#[test]
fn shadowsocks_sip002_plain_and_legacy() {
    let expected = json!({ "address": "ss.example.com", "port": 8388, "method": "aes-256-gcm", "password": "pass/word" });

    let userinfo = STANDARD.encode("aes-256-gcm:pass/word");
    let sip002 = parse_link(&format!("ss://{userinfo}@ss.example.com:8388#SS")).unwrap();
    assert_eq!(outbound(&sip002)["settings"], expected);
    assert_eq!(sip002.name, "SS");

    let legacy = STANDARD.encode("aes-256-gcm:pass/word@ss.example.com:8388");
    assert_eq!(outbound(&parse_link(&format!("ss://{legacy}#SS")).unwrap())["settings"], expected);

    let plain = parse_link("ss://2022-blake3-aes-256-gcm:a2V5MQ%3D%3D%3Aa2V5Mg%3D%3D@ss.example.com:8388").unwrap();
    assert_eq!(outbound(&plain)["settings"]["method"], "2022-blake3-aes-256-gcm");
    assert_eq!(outbound(&plain)["settings"]["password"], "a2V5MQ==:a2V5Mg==");
    assert!(outbound(&plain).get("streamSettings").is_none());
}

#[test]
fn shadowsocks_plugin_is_rejected() {
    let userinfo = STANDARD.encode("aes-256-gcm:pw");
    let err = parse_link(&format!("ss://{userinfo}@h.example:1?plugin=obfs-local")).unwrap_err();
    assert_eq!(err, Error::Unsupported("shadowsocks plugins".into()));
}

#[test]
fn hysteria2_with_obfs_and_port_hopping() {
    let server =
        parse_link("hy2://secret@hy.example.com:443,20000-30000?sni=hy.example.com&obfs=salamander&obfs-password=mask&insecure=1#Hy")
            .unwrap();
    assert_eq!((server.protocol.as_str(), server.port), ("hysteria", 443));
    assert_eq!(server.warnings.len(), 2);
    assert_eq!(
        outbound(&server),
        &json!({
            "protocol": "hysteria",
            "settings": { "version": 2, "address": "hy.example.com", "port": 443 },
            "streamSettings": {
                "method": "hysteria",
                "hysteriaSettings": { "version": 2, "auth": "secret" },
                "security": "tls",
                "tlsSettings": { "serverName": "hy.example.com", "alpn": ["h3"] },
                "finalmask": { "udp": [{ "type": "salamander", "settings": { "password": "mask" } }] },
            },
        })
    );
}

#[test]
fn invalid_links_report_why() {
    assert_eq!(parse_link("wireguard://x@h:1").unwrap_err(), Error::UnsupportedScheme("wireguard".into()));
    assert!(matches!(parse_link("not a link"), Err(Error::InvalidLink(_))));
    assert!(matches!(parse_link(&format!("vless://{UUID}@host")), Err(Error::InvalidLink(_))));
    assert!(matches!(parse_link("vless://@host:443"), Err(Error::InvalidLink(_))));
    assert!(matches!(
        parse_link(&format!("vless://{UUID}@host:443?security=reality")),
        Err(Error::InvalidLink(_))
    ));
    assert_eq!(
        parse_link(&format!("vless://{UUID}@host:443?type=quic")).unwrap_err(),
        Error::Unsupported("transport quic".into())
    );
}

#[test]
fn parse_many_skips_noise_and_collects_errors() {
    let text = format!(
        "# my servers\n\nvless://{UUID}@a.example:443?security=tls#A\n  trojan://pw@b.example:443#B  \nvless://broken\nhttps://example.com/not-a-proxy\n"
    );
    let imported = parse_many(&text);
    assert_eq!(imported.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(imported.errors.len(), 1);
    assert_ne!(imported.servers[0].id, imported.servers[1].id);
}
