//! The corporate network: an OpenVPN or Check Point tunnel next to the VPN.
//!
//! The engine runs it (`uniray-engine ovpn`, which runs `uniray-ovpn`, the
//! OpenVPN 3 core; `uniray-engine cp`, which runs `uniray-cp`, snx-rs) and
//! serves it to Xray as SOCKS5, as for a WireGuard server; Xray sends the corporate networks and domains there, through the
//! `corp` outbound, and everything else by its usual rules. The tunnel can
//! reach its server through another server in front of it (when only that one
//! gets through, e.g. on allowlisting networks): the engine then connects to a
//! loopback port of Xray that forwards to the corporate server through it.
//!
//! What the server pushes - its networks, DNS servers and domains - is known
//! only once the tunnel is up; the app keeps the last of it
//! ([`CorpNetwork::learned`]) and builds the routing from that.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{self, Bypass};
use crate::engine::{resolve, secret};
use crate::error::{Error, Result};
use crate::routes::parse_cidr;

/// What runs the corporate network.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CorpKind {
    #[default]
    OpenVpn,
    /// A Check Point gateway (Remote Access VPN, IPsec): [`CorpNetwork::gateway`],
    /// with [`CorpNetwork::cert`] and the user name and password.
    CheckPoint,
}

/// The corporate network the user imported, see the module docs.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CorpNetwork {
    /// Stable within the list; the engine's reports carry it ([`report`]).
    pub id: String,
    pub kind: CorpKind,
    pub enabled: bool,
    pub name: String,
    /// The OpenVPN profile, its key files inlined (see [`assemble`]).
    pub profile: String,
    /// The Check Point gateway: `host` or `host:port` (its HTTPS port).
    pub gateway: String,
    /// The user's certificate for Check Point: PKCS#12 (`.p12`, `.pfx`) in
    /// base64; empty for a login without one.
    pub cert: String,
    /// The file it came from, for the UI.
    pub cert_name: String,
    pub cert_password: String,
    pub username: String,
    pub password: String,
    /// Route what the server pushes (networks, DNS, domains) into the tunnel,
    /// besides [`Self::routes`] and [`Self::domains`].
    #[serde(default = "yes")]
    pub use_pushed: bool,
    /// Networks (CIDR) into the tunnel, the user's own.
    pub routes: Vec<String>,
    /// Domains into the tunnel, the user's own (`corp.example`, or with a
    /// matcher prefix such as `full:`).
    pub domains: Vec<String>,
    /// DNS servers inside the tunnel for those domains, the user's own.
    pub dns: Vec<String>,
    pub via: Via,
    /// What the server pushed last time.
    pub learned: Pushed,
}

/// The way to the corporate server.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum Via {
    /// Straight out, past the VPN.
    #[default]
    Direct,
    /// Through the server the VPN is connected to.
    Main,
    /// Through this server of the list (one Xray runs).
    Server(String),
}

/// The networks, DNS servers and domains the server pushed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Pushed {
    pub routes: Vec<String>,
    pub dns: Vec<String>,
    pub domains: Vec<String>,
}

/// An engine's report on a corporate tunnel in a log line (engine/ovpn.go):
/// `engine: corp[<id>]: up {json}`, or `engine: corp: ...` from an engine
/// run without an id. Gives the id (empty without one) and the report.
pub fn report(line: &str) -> Option<(&str, &str)> {
    let (_, rest) = line.split_once("engine: corp")?;
    if let Some(report) = rest.strip_prefix(": ") {
        return Some(("", report));
    }
    let (id, report) = rest.strip_prefix('[')?.split_once("]: ")?;
    Some((id, report))
}

impl Pushed {
    /// The engine's `corp: up {json}` log line (engine/ovpn.go), when `line`
    /// is one.
    pub fn from_log(line: &str) -> Option<Self> {
        let (_, report) = report(line)?;
        let json = report.strip_prefix("up ")?;
        #[derive(Deserialize)]
        struct Settings {
            #[serde(default)]
            routes: Option<Vec<String>>,
            #[serde(default)]
            dns: Option<Vec<String>>,
            #[serde(default)]
            domains: Option<Vec<String>>,
        }
        let settings: Settings = serde_json::from_str(json.trim()).ok()?;
        Some(Self {
            routes: settings.routes.unwrap_or_default(),
            dns: settings.dns.unwrap_or_default(),
            domains: settings.domains.unwrap_or_default(),
        })
    }
}

fn yes() -> bool {
    true
}

/// Where the profile connects: its first `remote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub host: String,
    pub port: u16,
    /// `"udp"` or `"tcp"`.
    pub proto: &'static str,
}

/// What the UI shows of a profile.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    pub host: String,
    pub port: u16,
    pub proto: &'static str,
    /// It asks for a user name and password (`auth-user-pass`).
    pub needs_credentials: bool,
}

/// The profile's lines outside its inline blocks (`<ca>...</ca>`), trimmed,
/// without comments.
fn directives(profile: &str) -> impl Iterator<Item = &str> {
    let mut block: Option<String> = None;
    profile.lines().filter_map(move |line| {
        let line = line.trim();
        if let Some(name) = &block {
            if line.eq_ignore_ascii_case(&format!("</{name}>")) {
                block = None;
            }
            return None;
        }
        if let Some(name) = line.strip_prefix('<').and_then(|rest| rest.strip_suffix('>')) {
            if !name.starts_with('/') {
                block = Some(name.to_owned());
            }
            return None;
        }
        (!line.is_empty() && !line.starts_with('#') && !line.starts_with(';')).then_some(line)
    })
}

fn proto_of(value: &str) -> &'static str {
    if value.to_ascii_lowercase().starts_with("tcp") { "tcp" } else { "udp" }
}

/// Reads where the profile connects and whether it needs credentials.
pub fn profile_info(profile: &str) -> Result<ProfileInfo> {
    let mut remote: Option<(String, Option<u16>, Option<&'static str>)> = None;
    let (mut port, mut proto, mut needs_credentials, mut client) = (1194, "udp", false, false);
    for line in directives(profile) {
        let mut words = line.split_whitespace();
        match words.next().unwrap_or_default() {
            "remote" if remote.is_none() => {
                let host = words.next().unwrap_or_default().to_owned();
                let port = words.next().and_then(|port| port.parse().ok());
                remote = Some((host, port, words.next().map(proto_of)));
            }
            "port" | "rport" => port = words.next().and_then(|port| port.parse().ok()).unwrap_or(port),
            "proto" => proto = words.next().map(proto_of).unwrap_or(proto),
            "auth-user-pass" => needs_credentials = true,
            "client" | "tls-client" => client = true,
            _ => {}
        }
    }
    let Some((host, remote_port, remote_proto)) = remote.filter(|(host, _, _)| !host.is_empty()) else {
        return Err(Error::InvalidConfig("the OpenVPN profile names no server (remote)".into()));
    };
    if !client {
        return Err(Error::InvalidConfig("this is not an OpenVPN client profile".into()));
    }
    Ok(ProfileInfo {
        host,
        port: remote_port.unwrap_or(port),
        proto: remote_proto.unwrap_or(proto),
        needs_credentials,
    })
}

/// The directives that name a file the profile needs, and the inline block
/// that takes its place.
const FILE_DIRECTIVES: [(&str, &str); 8] = [
    ("ca", "ca"),
    ("cert", "cert"),
    ("key", "key"),
    ("tls-auth", "tls-auth"),
    ("tls-crypt", "tls-crypt"),
    ("tls-crypt-v2", "tls-crypt-v2"),
    ("extra-certs", "extra-certs"),
    ("pkcs12", "pkcs12"),
];

/// Where a Check Point gateway is: `host` or `host:port`, the port its HTTPS
/// (443 unless given).
pub fn gateway_info(gateway: &str) -> Result<ProfileInfo> {
    let gateway = gateway.trim();
    let gateway = gateway.strip_prefix("https://").unwrap_or(gateway).trim_end_matches('/');
    let (host, port) = match gateway.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse().map_err(|_| Error::InvalidConfig(format!("{port} is not a port")))?;
            (host, port)
        }
        _ => (gateway, 443),
    };
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
        return Err(Error::InvalidConfig(format!("{gateway} is not a gateway address (vpn.example.com)")));
    }
    Ok(ProfileInfo { host: host.to_owned(), port, proto: "udp", needs_credentials: true })
}

impl CorpNetwork {
    /// Where it connects, as the UI shows it.
    pub fn info(&self) -> Result<ProfileInfo> {
        match self.kind {
            CorpKind::OpenVpn => profile_info(&self.profile),
            CorpKind::CheckPoint => gateway_info(&self.gateway),
        }
    }
}

/// Puts a profile together from what the user picked: the profile (`.ovpn`,
/// `.conf`, as OpenVPN Connect, Tunnelblick's `.tblk` and Viscosity's
/// `.visc` hold it) and the files it names (`ca ca.crt`, `tls-auth ta.key 1`),
/// which go inline, as one file is what the core takes. Returns the name
/// (the profile's file name without extension) and the profile.
pub fn assemble(files: &[(String, String)]) -> Result<(String, String)> {
    let is_profile = |(name, text): &&(String, String)| {
        let lower = name.to_ascii_lowercase();
        (lower.ends_with(".ovpn") || lower.ends_with(".conf")) && profile_info(text).is_ok()
    };
    let (name, profile) = files
        .iter()
        .find(is_profile)
        .or_else(|| files.iter().find(|(_, text)| profile_info(text).is_ok()))
        .ok_or_else(|| Error::InvalidConfig("no OpenVPN client profile among the files".into()))?;
    let by_name: HashMap<&str, &str> = files
        .iter()
        .map(|(name, text)| (Path::new(name).file_name().and_then(|name| name.to_str()).unwrap_or(name), text.as_str()))
        .collect();

    let mut out = String::new();
    let mut inline = String::new();
    let mut block: Option<String> = None;
    for raw in profile.lines() {
        let line = raw.trim();
        if let Some(name) = &block {
            if line.eq_ignore_ascii_case(&format!("</{name}>")) {
                block = None;
            }
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if let Some(name) = line.strip_prefix('<').and_then(|rest| rest.strip_suffix('>')).filter(|name| !name.starts_with('/')) {
            block = Some(name.to_owned());
        }
        let mut words = line.split_whitespace();
        let directive = words.next().unwrap_or_default();
        let file = words.next();
        let found = FILE_DIRECTIVES.iter().find(|(name, _)| *name == directive);
        match (found, file) {
            (Some(&(_, tag)), Some(file)) if file != "[inline]" => {
                let file = file.trim_matches('"');
                let base = Path::new(file).file_name().and_then(|name| name.to_str()).unwrap_or(file);
                let text = by_name.get(base).ok_or_else(|| {
                    Error::InvalidConfig(format!("the profile needs {base}: pick it together with the profile"))
                })?;
                inline.push_str(&format!("<{tag}>\n{}\n</{tag}>\n", text.trim()));
                // tls-auth's key direction stays a directive of its own.
                if let Some(direction) = words.next().filter(|_| directive == "tls-auth") {
                    out.push_str(&format!("key-direction {direction}\n"));
                }
            }
            _ => {
                out.push_str(raw);
                out.push('\n');
            }
        }
    }
    out.push_str(&inline);
    let stem = Path::new(name).file_stem().and_then(|stem| stem.to_str()).unwrap_or(name);
    Ok((stem.to_owned(), out))
}

/// One run of the corporate network: the engine's config and what Xray's
/// config needs ([`config::Corp`]).
#[derive(Clone, Debug)]
pub struct CorpRun {
    /// The engine in `ovpn` mode.
    pub engine: crate::engine::EngineRun,
    pub runtime: config::Corp,
}

/// The way to the corporate server for [`prepare`]: straight, or through
/// Xray's `front_port` and then `outbound` (`None`: the main server's).
pub enum Way {
    Direct,
    Front { port: u16, outbound: Option<Value> },
}

/// The engine run and Xray's part for `corp`: the engine on `port`, the
/// tunnel's server resolved (it blocks) to an address that a rule can send
/// past the VPN, or reached through `way`'s front. `binary` is uniray-ovpn,
/// or uniray-cp for Check Point (which goes straight only: it needs several
/// ports of the gateway, its HTTPS and IKE/ESP over UDP).
pub fn prepare(corp: &CorpNetwork, port: u16, ipv6: bool, binary: &Path, way: Way) -> Result<CorpRun> {
    let (user, pass) = ("uniray".to_owned(), secret());
    let (mode, engine, bypass, front) = match corp.kind {
        CorpKind::OpenVpn => {
            let (engine, bypass, front) = prepare_ovpn(corp, ipv6, binary, way)?;
            ("ovpn", engine, bypass, front)
        }
        CorpKind::CheckPoint => {
            let (engine, bypass) = prepare_cp(corp, binary)?;
            ("cp", engine, bypass, None)
        }
    };
    // Reached by name straight, not through a front (resolved at its far end).
    let host = corp.info()?.host;
    let host = (front.is_none() && host.parse::<IpAddr>().is_err()).then_some(host);
    let mut engine = engine;
    engine["corp"] = json!(corp.id);
    engine["user"] = json!(user);
    engine["pass"] = json!(pass);
    engine["port"] = json!(port);
    routing(corp, mode, engine, user, pass, port, bypass, front, host)
}

/// The engine's `ovpn` section, the bypass rule and the front.
fn prepare_ovpn(corp: &CorpNetwork, ipv6: bool, binary: &Path, way: Way) -> Result<(Value, Vec<Bypass>, Option<config::CorpFront>)> {
    let remote = profile_info(&corp.profile)?;
    let (server, front, bypass) = match way {
        Way::Direct => {
            let addr = resolve(&remote.host, remote.port, ipv6)?;
            (addr, None, vec![Bypass { addr, network: remote.proto }])
        }
        Way::Front { port: front_port, outbound } => {
            let front = config::CorpFront {
                port: front_port,
                host: remote.host.clone(),
                remote_port: remote.port,
                network: remote.proto,
                outbound,
            };
            (SocketAddr::from(([127, 0, 0, 1], front_port)), Some(front), Vec::new())
        }
    };
    let engine = json!({
        "ovpn": {
            "binary": binary.to_string_lossy(),
            "profile": corp.profile,
            "username": corp.username,
            "password": corp.password,
            "server": server.ip().to_string(),
            "port": server.port(),
            "proto": remote.proto,
        },
    });
    Ok((engine, bypass, front))
}

/// The engine's `cp` section and the bypass rules: the gateway, resolved to
/// its IPv4 address (snx-rs speaks IPv4 to it), on its HTTPS port and on IKE
/// and NAT-T (UDP 500, 4500).
fn prepare_cp(corp: &CorpNetwork, binary: &Path) -> Result<(Value, Vec<Bypass>)> {
    let gateway = gateway_info(&corp.gateway)?;
    let addr = resolve(&gateway.host, gateway.port, false)?;
    let ip = addr.ip();
    if !ip.is_ipv4() {
        return Err(Error::InvalidConfig(format!("{} has no IPv4 address", gateway.host)));
    }
    let bypass = vec![
        Bypass { addr, network: "tcp" },
        Bypass { addr: SocketAddr::new(ip, 500), network: "udp" },
        Bypass { addr: SocketAddr::new(ip, 4500), network: "udp" },
    ];
    let server_name = if gateway.port == 443 { gateway.host.clone() } else { format!("{}:{}", gateway.host, gateway.port) };
    let engine = json!({
        "cp": {
            "binary": binary.to_string_lossy(),
            "gateway": server_name,
            "address": ip.to_string(),
            "cert": corp.cert,
            "certPassword": corp.cert_password,
            "username": corp.username,
            "password": corp.password,
        },
    });
    Ok((engine, bypass))
}

/// The engine run and Xray's routing into the tunnel: the user's networks,
/// DNS servers and domains, and (when used) what the server pushed.
#[allow(clippy::too_many_arguments)]
fn routing(
    corp: &CorpNetwork,
    mode: &'static str,
    engine: Value,
    user: String,
    pass: String,
    port: u16,
    bypass: Vec<Bypass>,
    front: Option<config::CorpFront>,
    host: Option<String>,
) -> Result<CorpRun> {
    let pushed = if corp.use_pushed { corp.learned.clone() } else { Pushed::default() };
    let mut routes: Vec<String> = Vec::new();
    for route in corp.routes.iter().chain(&pushed.routes) {
        let route = route.trim();
        let Some((ip, len)) = parse_cidr(route).or_else(|| route.parse::<IpAddr>().ok().map(|ip| (ip, if ip.is_ipv4() { 32 } else { 128 }))) else {
            continue;
        };
        let cidr = format!("{ip}/{len}");
        if !routes.contains(&cidr) {
            routes.push(cidr);
        }
    }
    let mut dns: Vec<IpAddr> = Vec::new();
    for server in corp.dns.iter().chain(&pushed.dns) {
        if let Ok(ip) = server.trim().parse::<IpAddr>() {
            if !dns.contains(&ip) {
                dns.push(ip);
            }
        }
    }
    let mut domains: Vec<String> = Vec::new();
    for domain in corp.domains.iter().chain(&pushed.domains) {
        let domain = domain.trim().trim_start_matches('.');
        if domain.is_empty() {
            continue;
        }
        let matcher = crate::routing::domain_matcher(domain);
        if !domains.contains(&matcher) {
            domains.push(matcher);
        }
    }

    let runtime = config::Engine { port, user, pass, bypass, block_udp: false, sniff_excluded: Vec::new() };
    Ok(CorpRun {
        engine: crate::engine::EngineRun { mode, config: engine, runtime: runtime.clone(), warnings: Vec::new() },
        runtime: config::Corp { engine: runtime, routes, domains, dns, front, host },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = "client\ndev tun\nproto udp\nremote vpn.corp.example 1195\nauth-user-pass\n\
                           <ca>\nremote not.this 1\n</ca>\nkey-direction 1\n";

    #[test]
    fn profile_info_reads_the_first_remote() {
        let info = profile_info(PROFILE).unwrap();
        assert_eq!(info, ProfileInfo { host: "vpn.corp.example".into(), port: 1195, proto: "udp", needs_credentials: true });
        let tcp = profile_info("client\nport 443\nremote 203.0.113.1\nremote 203.0.113.2 1194 udp\nproto tcp-client\n").unwrap();
        assert_eq!((tcp.host.as_str(), tcp.port, tcp.proto, tcp.needs_credentials), ("203.0.113.1", 443, "tcp", false));
        assert_eq!(profile_info("client\nremote 203.0.113.1 1194 tcp\n").unwrap().proto, "tcp");
        assert!(profile_info("client\n").is_err());
        assert!(profile_info("remote 203.0.113.1\n").is_err(), "a server config");
    }

    #[test]
    fn assemble_inlines_the_named_files() {
        let files = vec![
            ("corp/config.conf".to_owned(), "client\nremote 203.0.113.1 1194\nca \"ca.crt\"\ntls-auth ta.key 1\ncert [inline]\n<cert>\nC\n</cert>\n".to_owned()),
            ("corp/ca.crt".to_owned(), "-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n".to_owned()),
            ("ta.key".to_owned(), "TA\n".to_owned()),
        ];
        let (name, profile) = assemble(&files).unwrap();
        assert_eq!(name, "config");
        assert!(profile.contains("<ca>\n-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n</ca>\n"));
        assert!(profile.contains("<tls-auth>\nTA\n</tls-auth>\n") && profile.contains("key-direction 1\n"));
        assert!(!profile.contains("ca \"ca.crt\"") && !profile.contains("tls-auth ta.key"));
        assert!(profile.contains("cert [inline]\n<cert>\nC\n</cert>\n"));

        let missing = vec![("a.ovpn".to_owned(), "client\nremote h 1\nca ca.crt\n".to_owned())];
        assert!(assemble(&missing).unwrap_err().to_string().contains("ca.crt"));
    }

    #[test]
    fn pushed_settings_come_from_the_log() {
        let line = r#"[Info] engine: corp: up {"mtu":1500,"addresses":["10.8.0.2/24"],"routes":["10.99.0.0/24"],"dns":["10.8.0.1"],"domains":["corp.test"]}"#;
        assert_eq!(
            Pushed::from_log(line),
            Some(Pushed { routes: vec!["10.99.0.0/24".into()], dns: vec!["10.8.0.1".into()], domains: vec!["corp.test".into()] })
        );
        let empty = r#"[Info] engine: corp: up {"mtu":1500,"addresses":["10.8.0.2/24"],"routes":null,"dns":null,"domains":null}"#;
        assert_eq!(Pushed::from_log(empty), Some(Pushed::default()));
        assert_eq!(Pushed::from_log("[Error] engine: corp: down AUTH_FAILED"), None);
        let labelled = r#"[Info] engine: corp[a1]: up {"routes":["10.1.0.0/16"],"dns":null,"domains":null}"#;
        assert_eq!(Pushed::from_log(labelled).unwrap().routes, ["10.1.0.0/16"]);
        assert_eq!(report("[Error] engine: corp[a1]: down AUTH_FAILED"), Some(("a1", "down AUTH_FAILED")));
        assert_eq!(report("[Warning] engine: corp: reconnecting"), Some(("", "reconnecting")));
        assert_eq!(report("[Info] engine: ready"), None);
    }

    #[test]
    fn prepare_merges_own_and_pushed() {
        let corp = CorpNetwork {
            enabled: true,
            profile: "client\nremote 203.0.113.7 1195\n".into(),
            username: "u".into(),
            password: "p".into(),
            use_pushed: true,
            routes: vec!["192.168.254.0/24".into(), "10.1.2.3".into(), "junk".into()],
            domains: vec![".corp.example".into(), "full:git.example".into()],
            dns: vec!["10.8.0.53".into()],
            learned: Pushed { routes: vec!["10.99.0.0/24".into(), "192.168.254.0/24".into()], dns: vec!["10.8.0.1".into()], domains: vec!["corp.test".into()] },
            ..Default::default()
        };
        let run = prepare(&corp, 40200, false, Path::new("/opt/uniray-ovpn"), Way::Direct).unwrap();
        let corp_rt = run.runtime.clone();
        assert_eq!(corp_rt.routes, ["192.168.254.0/24", "10.1.2.3/32", "10.99.0.0/24"]);
        assert_eq!(corp_rt.domains, ["domain:corp.example", "full:git.example", "domain:corp.test"]);
        assert_eq!(corp_rt.dns, ["10.8.0.53".parse::<IpAddr>().unwrap(), "10.8.0.1".parse().unwrap()]);
        assert_eq!(corp_rt.engine.bypass, [Bypass { addr: "203.0.113.7:1195".parse().unwrap(), network: "udp" }]);
        let engine: Value = serde_json::from_slice(&run.engine.config_for(&crate::engine::Peers::Pids(vec![9]), false)).unwrap();
        assert_eq!(engine["ovpn"]["server"], "203.0.113.7");
        assert_eq!(engine["ovpn"]["port"], 1195);
        assert_eq!(engine["ovpn"]["binary"], "/opt/uniray-ovpn");
        assert_eq!(run.engine.mode, "ovpn");

        let own_only = CorpNetwork { use_pushed: false, ..corp.clone() };
        let run = prepare(&own_only, 40200, false, Path::new("x"), Way::Front { port: 40300, outbound: None }).unwrap();
        let corp_rt = run.runtime.clone();
        assert_eq!(corp_rt.routes, ["192.168.254.0/24", "10.1.2.3/32"]);
        assert!(corp_rt.engine.bypass.is_empty());
        let front = corp_rt.front.unwrap();
        assert_eq!((front.port, front.host.as_str(), front.remote_port, front.network), (40300, "203.0.113.7", 1195, "udp"));
        let engine: Value = serde_json::from_slice(&run.engine.config_for(&crate::engine::Peers::Pids(vec![]), false)).unwrap();
        assert_eq!((engine["ovpn"]["server"].as_str(), engine["ovpn"]["port"].as_u64()), (Some("127.0.0.1"), Some(40300)));
    }

    #[test]
    fn gateway_info_reads_host_and_port() {
        assert_eq!(gateway_info("vpn.corp.example").unwrap().host, "vpn.corp.example");
        assert_eq!(gateway_info("https://vpn.corp.example/").unwrap().port, 443);
        let custom = gateway_info(" vpn.corp.example:8443 ").unwrap();
        assert_eq!((custom.host.as_str(), custom.port), ("vpn.corp.example", 8443));
        assert!(gateway_info("").is_err());
        assert!(gateway_info("vpn.corp.example:x").is_err());
        assert!(gateway_info("vpn corp").is_err());
    }

    #[test]
    fn prepare_check_point() {
        let corp = CorpNetwork {
            kind: CorpKind::CheckPoint,
            enabled: true,
            gateway: "203.0.113.9".into(),
            cert: "UDEy".into(),
            cert_password: "cp".into(),
            username: "u".into(),
            password: "p".into(),
            routes: vec!["10.11.0.0/16".into()],
            learned: Pushed { routes: vec![], dns: vec!["10.11.33.18".into()], domains: vec!["corp.test".into()] },
            use_pushed: true,
            via: Via::Main,
            ..Default::default()
        };
        let run = prepare(&corp, 40200, false, Path::new("/opt/uniray-cp"), Way::Direct).unwrap();
        assert_eq!(run.engine.mode, "cp");
        assert_eq!(
            run.runtime.engine.bypass,
            [
                Bypass { addr: "203.0.113.9:443".parse().unwrap(), network: "tcp" },
                Bypass { addr: "203.0.113.9:500".parse().unwrap(), network: "udp" },
                Bypass { addr: "203.0.113.9:4500".parse().unwrap(), network: "udp" },
            ]
        );
        assert_eq!(run.runtime.routes, ["10.11.0.0/16"]);
        assert_eq!(run.runtime.domains, ["domain:corp.test"]);
        assert!(run.runtime.front.is_none());
        let mut engine_run = run.engine.clone();
        engine_run.set_cp_binary(Path::new("/staged/uniray-cp"));
        let engine: Value = serde_json::from_slice(&engine_run.config_for(&crate::engine::Peers::Pids(vec![]), false)).unwrap();
        assert_eq!(engine["cp"]["gateway"], "203.0.113.9");
        assert_eq!(engine["cp"]["address"], "203.0.113.9");
        assert_eq!(engine["cp"]["cert"], "UDEy");
        assert_eq!(engine["cp"]["binary"], "/staged/uniray-cp");
        assert_eq!(engine["port"], 40200);
        assert!(engine["ovpn"].is_null());

        let custom = CorpNetwork { gateway: "203.0.113.9:8443".into(), ..corp };
        let run = prepare(&custom, 40200, false, Path::new("x"), Way::Direct).unwrap();
        let engine: Value = serde_json::from_slice(&run.engine.config_for(&crate::engine::Peers::Pids(vec![]), false)).unwrap();
        assert_eq!(engine["cp"]["gateway"], "203.0.113.9:8443");
        assert_eq!(run.runtime.engine.bypass[0].addr.port(), 8443);
    }
}
