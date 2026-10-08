//! Share link parsing: `vless://`, `vmess://`, `trojan://`, `ss://`,
//! `hysteria2://` -> [`Server`] holding a ready Xray outbound; and the
//! WireGuard/AmneziaWG forms - `vpn://` keys, `wireguard://` links and
//! pasted `.conf` blocks - see [`crate::wireguard`]; and `naive+https://`,
//! `naive+quic://`, see [`crate::naive`].

mod stream;
mod url;

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::model::{new_id, Server, ServerSource};
use url::{Params, RawUrl};

pub(crate) use url::{decode_base64, decode_base64_str, parse as parse_url};

/// Schemes [`parse_link`] understands.
pub const SCHEMES: [&str; 11] = [
    "vless", "vmess", "trojan", "ss", "hysteria2", "hy2", "vpn", "wireguard", "wg", "naive+https", "naive+quic",
];

pub fn is_link(line: &str) -> bool {
    line.split_once("://")
        .is_some_and(|(scheme, _)| SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()))
}

pub fn parse_link(link: &str) -> Result<Server> {
    let link = link.trim();
    let scheme = link
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .ok_or_else(|| Error::InvalidLink("missing scheme".into()))?;
    let parsed = match scheme.as_str() {
        "vpn" => return crate::wireguard::server_from_amnezia_key(link),
        "wireguard" | "wg" => return crate::wireguard::server_from_link(link),
        "naive+https" | "naive+quic" => return crate::naive::server_from_link(link),
        "vless" => vless(link),
        "vmess" => vmess(link),
        "trojan" => trojan(link),
        "ss" => shadowsocks(link),
        "hysteria2" | "hy2" => hysteria2(link),
        _ => Err(Error::UnsupportedScheme(scheme)),
    }?;
    Ok(parsed.into_server(link))
}

/// Outcome of importing a block of text: one link per line, and any
/// WireGuard/AmneziaWG `.conf` pasted whole.
#[derive(Debug, Default)]
pub struct Imported {
    pub servers: Vec<Server>,
    /// Lines that looked like links but could not be parsed, with the reason.
    pub errors: Vec<String>,
}

pub fn parse_many(text: &str) -> Imported {
    let mut imported = Imported::default();
    let (blocks, rest) = crate::wireguard::split_conf_blocks(text);
    for block in blocks {
        match crate::wireguard::server_from_conf(&block) {
            Ok(server) => imported.servers.push(server),
            Err(err) => imported.errors.push(err.to_string()),
        }
    }
    for line in rest.lines().map(str::trim).filter(|line| is_link(line)) {
        match parse_link(line) {
            Ok(server) => imported.servers.push(server),
            Err(err) => imported.errors.push(err.to_string()),
        }
    }
    imported
}

struct Parsed {
    name: String,
    protocol: &'static str,
    address: String,
    port: u16,
    transport: &'static str,
    security: &'static str,
    outbound: Value,
    warnings: Vec<String>,
}

impl Parsed {
    fn into_server(self, link: &str) -> Server {
        let name = if self.name.trim().is_empty() {
            format!("{}:{}", self.address, self.port)
        } else {
            self.name.trim().to_owned()
        };
        Server {
            id: new_id(),
            name,
            protocol: self.protocol.into(),
            address: self.address,
            port: self.port,
            transport: self.transport.into(),
            security: self.security.into(),
            source: ServerSource::Outbound(self.outbound),
            link: Some(link.to_owned()),
            subscription_id: None,
            delay_ms: None,
            warnings: self.warnings,
        }
    }
}

fn userinfo(url: &RawUrl, what: &str) -> Result<String> {
    url.userinfo
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidLink(format!("missing {what}")))
}

fn vless(link: &str) -> Result<Parsed> {
    let url = url::parse(link)?;
    let port = url.port()?;
    let mut warnings = Vec::new();
    let stream = stream::build(&url.params, "none", &mut warnings)?;

    let mut settings = Map::new();
    settings.insert("address".into(), url.host.clone().into());
    settings.insert("port".into(), port.into());
    settings.insert("id".into(), userinfo(&url, "user id")?.into());
    settings.insert(
        "encryption".into(),
        url.params.get("encryption").unwrap_or("none").into(),
    );
    if let Some(flow) = url.params.get("flow") {
        settings.insert("flow".into(), flow.into());
    }

    Ok(Parsed {
        name: url.fragment,
        protocol: "vless",
        address: url.host,
        port,
        transport: stream.transport,
        security: stream.security,
        outbound: json!({ "protocol": "vless", "settings": settings, "streamSettings": stream.settings }),
        warnings,
    })
}

/// `vmess://` is base64 of a JSON object (the v2rayN format); the URL-shaped
/// variant some panels emit is accepted too.
fn vmess(link: &str) -> Result<Parsed> {
    let (_, body, _, _) = url::split(link)?;
    let Some(decoded) = decode_base64_str(body).filter(|text| text.trim_start().starts_with('{')) else {
        return vmess_url(link);
    };
    let doc: Value =
        serde_json::from_str(&decoded).map_err(|err| Error::InvalidLink(format!("vmess JSON: {err}")))?;
    let field = |key: &str| match doc.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    };

    let address = field("add").ok_or_else(|| Error::InvalidLink("vmess without address".into()))?;
    let port: u16 = field("port")
        .and_then(|port| port.parse().ok())
        .ok_or_else(|| Error::InvalidLink("vmess without port".into()))?;
    let id = field("id").ok_or_else(|| Error::InvalidLink("vmess without id".into()))?;

    // Map the v2rayN field names onto the share link parameter names.
    let mut params = Params::default();
    let net = field("net").unwrap_or_else(|| "tcp".into());
    let path_key = match net.as_str() {
        "grpc" | "gun" => "serviceName",
        "kcp" | "mkcp" => "seed",
        _ => "path",
    };
    for (key, value) in [
        ("headerType", field("type")),
        ("host", field("host")),
        (path_key, field("path")),
        ("security", field("tls")),
        ("sni", field("sni")),
        ("alpn", field("alpn")),
        ("fp", field("fp")),
        ("pbk", field("pbk")),
        ("sid", field("sid")),
        ("allowInsecure", field("allowInsecure").or_else(|| field("skip-cert-verify"))),
    ] {
        if let Some(value) = value {
            params.set(key, value);
        }
    }
    if net == "grpc" && params.get("headerType") == Some("multi") {
        params.set("mode", "multi");
    }
    params.set("type", net);

    let mut warnings = Vec::new();
    let stream = stream::build(&params, "none", &mut warnings)?;
    Ok(Parsed {
        name: field("ps").unwrap_or_default(),
        protocol: "vmess",
        outbound: vmess_outbound(&address, port, &id, field("scy").as_deref(), stream.settings),
        address,
        port,
        transport: stream.transport,
        security: stream.security,
        warnings,
    })
}

fn vmess_url(link: &str) -> Result<Parsed> {
    let url = url::parse(link)?;
    let port = url.port()?;
    let id = userinfo(&url, "user id")?;
    let mut warnings = Vec::new();
    let stream = stream::build(&url.params, "none", &mut warnings)?;
    Ok(Parsed {
        name: url.fragment,
        protocol: "vmess",
        outbound: vmess_outbound(&url.host, port, &id, url.params.get("encryption"), stream.settings),
        address: url.host,
        port,
        transport: stream.transport,
        security: stream.security,
        warnings,
    })
}

fn vmess_outbound(address: &str, port: u16, id: &str, cipher: Option<&str>, stream: Value) -> Value {
    json!({
        "protocol": "vmess",
        "settings": { "address": address, "port": port, "id": id, "security": cipher.unwrap_or("auto") },
        "streamSettings": stream,
    })
}

fn trojan(link: &str) -> Result<Parsed> {
    let url = url::parse(link)?;
    let port = url.port()?;
    let password = userinfo(&url, "password")?;
    let mut warnings = Vec::new();
    let stream = stream::build(&url.params, "tls", &mut warnings)?;
    Ok(Parsed {
        name: url.fragment,
        protocol: "trojan",
        outbound: json!({
            "protocol": "trojan",
            "settings": { "address": url.host, "port": port, "password": password },
            "streamSettings": stream.settings,
        }),
        address: url.host,
        port,
        transport: stream.transport,
        security: stream.security,
        warnings,
    })
}

/// SIP002 (`ss://base64(method:password)@host:port`), its plain-userinfo form
/// used with 2022 ciphers, and the legacy fully-encoded `ss://base64(...)`.
fn shadowsocks(link: &str) -> Result<Parsed> {
    let (_, body, query, fragment) = url::split(link)?;
    let decoded;
    let body = if body.contains('@') {
        body
    } else {
        decoded = decode_base64_str(body.trim_end_matches('/'))
            .ok_or_else(|| Error::InvalidLink("ss link is not valid base64".into()))?;
        decoded.as_str()
    };
    let url = url::parse_authority(body)?;
    let port = url.port()?;
    let params = Params::parse(query);
    if params.get("plugin").is_some() {
        return Err(Error::Unsupported("shadowsocks plugins".into()));
    }

    let credentials = userinfo(&url, "credentials")?;
    let credentials = match credentials.split_once(':') {
        Some(_) => credentials,
        None => decode_base64_str(&credentials)
            .ok_or_else(|| Error::InvalidLink("ss credentials are not valid base64".into()))?,
    };
    let (method, password) = credentials
        .split_once(':')
        .ok_or_else(|| Error::InvalidLink("ss credentials must be method:password".into()))?;

    let mut outbound = json!({
        "protocol": "shadowsocks",
        "settings": { "address": url.host, "port": port, "method": method, "password": password },
    });
    let mut warnings = Vec::new();
    let (transport, security) = if params.get("type").is_some() || params.get("security").is_some() {
        let stream = stream::build(&params, "none", &mut warnings)?;
        outbound["streamSettings"] = stream.settings;
        (stream.transport, stream.security)
    } else {
        ("raw", "none")
    };

    Ok(Parsed {
        name: fragment,
        protocol: "shadowsocks",
        address: url.host,
        port,
        transport,
        security,
        outbound,
        warnings,
    })
}

fn hysteria2(link: &str) -> Result<Parsed> {
    let url = url::parse(link)?;
    let mut warnings = Vec::new();
    // `443,5000-6000` is port hopping; Xray takes a single port here.
    let first_port = url.port.split([',', '-']).next().unwrap_or_default();
    let port: u16 = first_port
        .parse()
        .map_err(|_| Error::InvalidLink(format!("bad port {:?}", url.port)))?;
    if first_port != url.port {
        warnings.push(format!("port hopping is not supported, using port {port}"));
    }
    let auth = userinfo(&url, "password")?;
    let params = &url.params;

    let mut tls = Map::new();
    if let Some(sni) = params.get("sni") {
        tls.insert("serverName".into(), sni.into());
    }
    tls.insert("alpn".into(), json!(["h3"]));
    if let Some(pin) = params.get("pinSHA256") {
        tls.insert("pinnedPeerCertSha256".into(), pin.replace(':', "").into());
    }
    if params.flag("insecure") || params.flag("allowInsecure") {
        warnings.push(stream::INSECURE_WARNING.into());
    }

    let mut stream = json!({
        "method": "hysteria",
        "hysteriaSettings": { "version": 2, "auth": auth },
        "security": "tls",
        "tlsSettings": tls,
    });
    match params.get("obfs") {
        None | Some("none") => {}
        Some("salamander") => {
            let password = params
                .get("obfs-password")
                .ok_or_else(|| Error::InvalidLink("salamander obfs without `obfs-password`".into()))?;
            stream["finalmask"] = json!({ "udp": [{ "type": "salamander", "settings": { "password": password } }] });
        }
        Some(other) => return Err(Error::Unsupported(format!("obfs {other}"))),
    }

    Ok(Parsed {
        name: url.fragment,
        protocol: "hysteria",
        outbound: json!({
            "protocol": "hysteria",
            "settings": { "version": 2, "address": url.host, "port": port },
            "streamSettings": stream,
        }),
        address: url.host,
        port,
        transport: "hysteria",
        security: "tls",
        warnings,
    })
}

#[cfg(test)]
mod tests;
