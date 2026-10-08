use std::io::Write;
use std::net::SocketAddr;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::json;

use super::*;
use crate::links::parse_many;

const PRIVATE: &str = "TzyWzvJtdUuH7lLv/jB+Db5HIQFadCGVuKtvlOh+tXc=";
const PUBLIC: &str = "rHm/b5YH6qXaJ5i+vbGS0nBRbpsR9Yo67MLslDrryTg=";
const PSK: &str = "vfNuCgNhM1OFL2C1LnDTGtHqKdQzBvU05G5V/FqszO4=";

/// What AmneziaVPN exports for an AmneziaWG 1.0 server.
fn awg_conf() -> String {
    format!(
        "[Interface]
Address = 10.8.1.2/32
DNS = 1.1.1.1, 1.0.0.1
PrivateKey = {PRIVATE}
Jc = 4
Jmin = 10
Jmax = 50
S1 = 117
S2 = 19
H1 = 1592434219
H2 = 2028405313
H3 = 1471523009
H4 = 1281489627

[Peer]
PublicKey = {PUBLIC}
PresharedKey = {PSK}
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.com:51820
PersistentKeepalive = 25
"
    )
}

fn hex(key: &str) -> String {
    key_hex(key).unwrap()
}

#[test]
fn amneziawg_conf_is_parsed() {
    let server = server_from_conf(&awg_conf()).unwrap();
    assert_eq!(server.protocol, "amneziawg");
    assert_eq!(server.security, "awg");
    assert_eq!(server.transport, "udp");
    assert_eq!((server.address.as_str(), server.port), ("vpn.example.com", 51820));
    assert_eq!(server.name, "vpn.example.com:51820");
    assert!(server.warnings.is_empty(), "{:?}", server.warnings);
    let ServerSource::WireGuard(config) = &server.source else { panic!("expected WireGuard") };
    assert_eq!(config.addresses, ["10.8.1.2/32"]);
    assert_eq!(config.dns, ["1.1.1.1", "1.0.0.1"]);
    assert_eq!(config.obfuscation["Jc"], "4");
    assert_eq!(config.obfuscation["H4"], "1281489627");
    assert_eq!(config.peers[0].preshared_key.as_deref(), Some(PSK));
    assert_eq!(config.peers[0].keepalive.as_deref(), Some("25"));
    // It is not Xray's to run.
    assert!(!server.source.is_xray());
}

#[test]
fn uapi_has_device_options_before_peers_and_hex_keys() {
    let (config, _, _) = parse_conf(&awg_conf()).unwrap();
    let endpoint: SocketAddr = "203.0.113.7:51820".parse().unwrap();
    let uapi = config.to_uapi(&[endpoint]).unwrap();
    let expected = format!(
        "private_key={}\njc=4\njmin=10\njmax=50\ns1=117\ns2=19\nh1=1592434219\nh2=2028405313\nh3=1471523009\nh4=1281489627\n\
         replace_peers=true\npublic_key={}\npreshared_key={}\nendpoint=203.0.113.7:51820\n\
         persistent_keepalive_interval=25\nreplace_allowed_ips=true\nallowed_ip=0.0.0.0/0\nallowed_ip=::/0\n",
        hex(PRIVATE),
        hex(PUBLIC),
        hex(PSK),
    );
    assert_eq!(uapi, expected);
    // Endpoints must come resolved, one per peer.
    assert!(config.to_uapi(&[]).is_err());
}

#[test]
fn amneziawg_2_options_are_accepted_and_checked() {
    let conf = format!(
        "[Interface]\nPrivateKey = {PRIVATE}\nAddress = 10.0.0.2/32\nS3 = 8\nS4 = 16\nH1 = 100-200\n\
         I1 = <b 0xc6000000010843290a47ba8ba2ed000044d0e3efd9326adb60561baa3bc4b52471b2d459ddcc9a508dffddc97e4d40d811><r 16>\n\
         [Peer]\nPublicKey = {PUBLIC}\nEndpoint = [2001:db8::1]:443\n"
    );
    let server = server_from_conf(&conf).unwrap();
    assert_eq!((server.address.as_str(), server.port), ("2001:db8::1", 443));
    let ServerSource::WireGuard(config) = &server.source else { panic!() };
    assert_eq!(config.obfuscation["H1"], "100-200");
    assert!(config.obfuscation["I1"].starts_with("<b 0xc6"));

    let backwards = conf.replace("H1 = 100-200", "H1 = 200-100");
    assert!(server_from_conf(&backwards).is_err());
    let not_a_number = conf.replace("S3 = 8", "S3 = eight");
    assert!(server_from_conf(&not_a_number).is_err());
}

#[test]
fn plain_wireguard_conf_defaults_and_warnings() {
    let conf = format!(
        "# Name = Office\n[Interface]\nPrivateKey = {PRIVATE}\nAddress = 10.0.0.2/24, fd00::2/64\nMTU = 1380\n\
         ListenPort = 51820\nPostUp = iptables -A FORWARD -j ACCEPT\n\n[Peer]\nPublicKey = {PUBLIC}\nEndpoint = 198.51.100.1:51820\n"
    );
    let server = server_from_conf(&conf).unwrap();
    assert_eq!(server.protocol, "wireguard");
    assert_eq!(server.security, "none");
    assert_eq!(server.name, "Office");
    // ListenPort means nothing to a client; PostUp scripts are not run.
    assert_eq!(server.warnings, ["PostUp is not supported and was ignored"]);
    let ServerSource::WireGuard(config) = &server.source else { panic!() };
    assert_eq!(config.mtu, Some(1380));
    assert_eq!(config.peers[0].allowed_ips, ["0.0.0.0/0", "::/0"]);
    assert_eq!(config.peers[0].keepalive, None);
}

#[test]
fn broken_confs_are_refused() {
    let without_peer = format!("[Interface]\nPrivateKey = {PRIVATE}\nAddress = 10.0.0.2/32\n");
    assert!(parse_conf(&without_peer).is_err());
    let bad_key = awg_conf().replace(PUBLIC, "not-a-key");
    assert!(parse_conf(&bad_key).is_err());
    let no_address = awg_conf().replace("Address = 10.8.1.2/32\n", "");
    assert!(parse_conf(&no_address).is_err());
    let bad_endpoint = awg_conf().replace("vpn.example.com:51820", "vpn.example.com");
    assert!(parse_conf(&bad_endpoint).is_err());
}

#[test]
fn conf_round_trips() {
    let (config, _, _) = parse_conf(&awg_conf()).unwrap();
    let (again, _, warnings) = parse_conf(&config.to_conf()).unwrap();
    assert_eq!(again, config);
    assert!(warnings.is_empty());
    // A server keeps its .conf as the thing to share.
    let server = server_from_conf(&awg_conf()).unwrap();
    assert_eq!(server.link.as_deref(), Some(config.to_conf().as_str()));
}

/// An AmneziaVPN key the way the app packs one: Qt qCompress, base64url.
fn amnezia_key(doc: &serde_json::Value) -> String {
    let json = serde_json::to_vec(doc).unwrap();
    let mut packed = (json.len() as u32).to_be_bytes().to_vec();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&json).unwrap();
    packed.extend(encoder.finish().unwrap());
    format!("vpn://{}", URL_SAFE_NO_PAD.encode(packed))
}

#[test]
fn amnezia_vpn_key_is_parsed() {
    let last_config = json!({ "config": awg_conf(), "hostName": "vpn.example.com", "port": 51820 }).to_string();
    let doc = json!({
        "containers": [
            { "container": "amnezia-xray", "xray": {} },
            { "container": "amnezia-awg", "awg": { "last_config": last_config, "port": "51820" } },
        ],
        "defaultContainer": "amnezia-awg",
        "description": "My Amnezia",
        "hostName": "vpn.example.com",
    });
    let key = amnezia_key(&doc);
    let server = crate::links::parse_link(&key).unwrap();
    assert_eq!(server.protocol, "amneziawg");
    assert_eq!(server.name, "My Amnezia");
    assert_eq!(server.link.as_deref(), Some(key.as_str()));

    // A key for another protocol says what it has.
    let xray_only = json!({ "containers": [{ "container": "amnezia-xray", "xray": {} }], "defaultContainer": "amnezia-xray" });
    let err = crate::links::parse_link(&amnezia_key(&xray_only)).unwrap_err().to_string();
    assert!(err.contains("amnezia-xray"), "{err}");
    assert!(crate::links::parse_link("vpn://not-base64!").is_err());
}

#[test]
fn wireguard_link_is_parsed() {
    let link = format!(
        "wireguard://{}@198.51.100.1:51820?publickey={}&address=10.0.0.2%2F32,fd00::2%2F128&mtu=1280&reserved=1,2,3#Cloud",
        PRIVATE.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D"),
        PUBLIC.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D"),
    );
    let server = crate::links::parse_link(&link).unwrap();
    assert_eq!(server.protocol, "wireguard");
    assert_eq!(server.name, "Cloud");
    assert_eq!(server.warnings.len(), 1, "reserved is reported");
    let ServerSource::WireGuard(config) = &server.source else { panic!() };
    assert_eq!(config.private_key, PRIVATE);
    assert_eq!(config.addresses, ["10.0.0.2/32", "fd00::2/128"]);
    assert_eq!(config.mtu, Some(1280));
    assert_eq!(config.peers[0].endpoint, "198.51.100.1:51820");
}

#[test]
fn pasted_text_mixes_confs_and_links() {
    let text = format!(
        "{}\nvless://5783a3e7-e373-51cd-8642-c83782b807c5@example.com:443?security=tls#One\n{}",
        awg_conf(),
        awg_conf().replace("vpn.example.com", "second.example.com"),
    );
    let imported = parse_many(&text);
    assert!(imported.errors.is_empty(), "{:?}", imported.errors);
    let protocols: Vec<&str> = imported.servers.iter().map(|server| server.protocol.as_str()).collect();
    assert_eq!(protocols, ["amneziawg", "amneziawg", "vless"]);
    assert_eq!(imported.servers[1].address, "second.example.com");
}

#[test]
fn import_takes_a_pasted_conf() {
    // What the app's import does with pasted text (or a subscription body):
    // `[Interface]` must not be mistaken for a JSON array.
    let (servers, errors) = crate::subscription::parse_body(&awg_conf());
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].protocol, "amneziawg");
    // JSON configs still go the JSON way.
    let (servers, _) = crate::subscription::parse_body(r#"[{"outbounds":[{"protocol":"freedom","tag":"proxy"}]}]"#);
    assert_eq!(servers.len(), 1);
    assert!(matches!(servers[0].source, ServerSource::Config(_)));
}

#[test]
fn stored_servers_keep_their_config() {
    let server = server_from_conf(&awg_conf()).unwrap();
    let stored = serde_json::to_string(&server).unwrap();
    let loaded: Server = serde_json::from_str(&stored).unwrap();
    assert_eq!(loaded, server);
}

#[test]
fn awg_1_5_extras_stay_out_of_the_uapi() {
    let conf = awg_conf().replace("[Peer]", "J1 = <b 0xaa>\nItime = 120\n[Peer]");
    let (config, _, _) = parse_conf(&conf).unwrap();
    assert_eq!(config.unsupported_options(), ["J1", "Itime"]);
    let uapi = config.to_uapi(&["203.0.113.7:51820".parse().unwrap()]).unwrap();
    assert!(!uapi.contains("j1=") && !uapi.contains("itime="), "{uapi}");
    // The export keeps them.
    assert!(config.to_conf().contains("Itime = 120"));
}

#[test]
fn endpoints_resolve_in_peer_order() {
    let (mut config, _, _) = parse_conf(&awg_conf()).unwrap();
    let mut second = config.peers[0].clone();
    second.endpoint = "[2001:db8::5]:443".into();
    config.peers.push(second);
    config.peers[0].endpoint = "localhost:51820".into();
    let resolved = config.resolve_endpoints(false).unwrap();
    assert_eq!(resolved[0], "127.0.0.1:51820".parse::<SocketAddr>().unwrap());
    assert_eq!(resolved[1], "[2001:db8::5]:443".parse::<SocketAddr>().unwrap());
    config.peers[0].endpoint = "no-such-host.invalid:1".into();
    assert!(config.resolve_endpoints(false).is_err());
}

#[test]
fn keepalive_takes_an_awg_2_range() {
    let conf = awg_conf().replace("PersistentKeepalive = 25", "PersistentKeepalive = 25-35");
    assert_ne!(conf, awg_conf(), "the fixture has PersistentKeepalive = 25");
    let (config, _, _) = parse_conf(&conf).unwrap();
    assert_eq!(config.peers[0].keepalive.as_deref(), Some("25-35"));
    let uapi = config.to_uapi(&["203.0.113.7:51820".parse().unwrap()]).unwrap();
    assert!(uapi.contains("persistent_keepalive_interval=25-35\n"), "{uapi}");
    assert!(config.to_conf().contains("PersistentKeepalive = 25-35"));

    for (value, kept) in [("0", None), ("off", None), ("10-10", Some("10")), ("0-0", None)] {
        let (config, _, _) = parse_conf(&awg_conf().replace("PersistentKeepalive = 25", &format!("PersistentKeepalive = {value}"))).unwrap();
        assert_eq!(config.peers[0].keepalive.as_deref(), kept, "{value}");
    }
    for value in ["35-25", "abc", "1-99999"] {
        assert!(parse_conf(&awg_conf().replace("PersistentKeepalive = 25", &format!("PersistentKeepalive = {value}"))).is_err(), "{value}");
    }
}

#[test]
fn servers_saved_with_a_numeric_keepalive_still_load() {
    let (config, _, _) = parse_conf(&awg_conf()).unwrap();
    let mut saved = serde_json::to_value(&config).unwrap();
    saved["peers"][0]["keepalive"] = json!(25);
    let loaded: WgConfig = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(loaded.peers[0].keepalive.as_deref(), Some("25"));
    saved["peers"][0]["keepalive"] = serde_json::Value::Null;
    let loaded: WgConfig = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.peers[0].keepalive, None);
}
