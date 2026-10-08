//! WireGuard and AmneziaWG servers.
//!
//! They come as awg-quick `.conf` files, AmneziaVPN `vpn://` keys or
//! `wireguard://` links, and run on their own engine (amneziawg-go), not on
//! Xray: the engine takes the [`WgConfig::to_uapi`] text. AmneziaWG is
//! WireGuard plus obfuscation - junk packets, paddings, custom message
//! types - set by the [`OBFUSCATION_KEYS`] of the interface; without them it
//! is plain WireGuard.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::links::decode_base64;
use crate::model::{new_id, Server, ServerSource};

/// The AmneziaWG interface options, as written in a `.conf` file (the UAPI
/// takes them lowercased). Jc/Jmin/Jmax: junk packets before a handshake;
/// S1-S4: paddings of handshake, cookie and transport messages; H1-H4:
/// message types (a number, or a range in AWG 2.0); I1-I5: signature packets
/// (AWG 1.5+); J1-J3 and Itime: AWG 1.5 extras.
pub const OBFUSCATION_KEYS: [&str; 20] = [
    "Jc", "Jmin", "Jmax", "S1", "S2", "S3", "S4", "H1", "H2", "H3", "H4", "I1", "I2", "I3", "I4", "I5", "J1", "J2",
    "J3", "Itime",
];

/// AWG 1.5 options the engine's amneziawg-go (AWG 2.0) no longer has: kept
/// in the server (and its `.conf` export) but left out of the UAPI, which
/// would refuse them. See [`WgConfig::unsupported_options`].
pub const ENGINE_UNSUPPORTED_KEYS: [&str; 4] = ["J1", "J2", "J3", "Itime"];

/// Keys a `.conf` may carry that mean nothing to us; dropped without a word.
const IGNORED_KEYS: [&str; 3] = ["ListenPort", "Table", "SaveConfig"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WgConfig {
    /// Base64, as in the `.conf`.
    pub private_key: String,
    /// The tunnel addresses with prefix, e.g. `10.8.1.2/32`.
    pub addresses: Vec<String>,
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub mtu: Option<u16>,
    pub peers: Vec<WgPeer>,
    /// AmneziaWG options by their `.conf` name ([`OBFUSCATION_KEYS`]);
    /// empty for plain WireGuard.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub obfuscation: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WgPeer {
    pub public_key: String,
    #[serde(default)]
    pub preshared_key: Option<String>,
    /// `host:port`, `[v6]:port`; the host may be a name.
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub keepalive: Option<u16>,
}

impl WgConfig {
    pub fn is_amnezia(&self) -> bool {
        !self.obfuscation.is_empty()
    }

    /// The first peer's endpoint as host and port.
    pub fn endpoint(&self) -> Result<(String, u16)> {
        let peer = self.peers.first().ok_or_else(|| Error::InvalidConfig("no [Peer]".into()))?;
        split_endpoint(&peer.endpoint)
    }

    /// The options of this server the engine ignores, for a warning.
    pub fn unsupported_options(&self) -> Vec<&'static str> {
        ENGINE_UNSUPPORTED_KEYS.into_iter().filter(|key| self.obfuscation.contains_key(*key)).collect()
    }

    /// Resolves every peer's endpoint to the address the engine dials, in
    /// the order of `peers` (what [`Self::to_uapi`] takes). Blocking. Names
    /// prefer IPv4 unless `ipv6` allows either.
    pub fn resolve_endpoints(&self, ipv6: bool) -> Result<Vec<SocketAddr>> {
        if self.peers.is_empty() {
            return Err(Error::InvalidConfig("no [Peer]".into()));
        }
        self.peers
            .iter()
            .map(|peer| {
                let (host, port) = split_endpoint(&peer.endpoint)?;
                if let Ok(ip) = host.parse::<IpAddr>() {
                    return Ok(SocketAddr::new(ip, port));
                }
                let found: Vec<SocketAddr> = (host.as_str(), port)
                    .to_socket_addrs()
                    .map_err(|err| Error::InvalidConfig(format!("cannot resolve {host}: {err}")))?
                    .collect();
                found
                    .iter()
                    .find(|addr| addr.is_ipv4())
                    .or_else(|| found.iter().find(|_| ipv6))
                    .copied()
                    .ok_or_else(|| Error::InvalidConfig(format!("{host} has no usable address")))
            })
            .collect()
    }

    /// The configuration in the UAPI format the engine takes, with the peers'
    /// endpoints already resolved (`resolved[i]` belongs to `peers[i]`): UAPI
    /// accepts only addresses. Keys go hex-encoded; the device options,
    /// AmneziaWG's included, come before the peers.
    pub fn to_uapi(&self, resolved: &[SocketAddr]) -> Result<String> {
        if resolved.len() != self.peers.len() {
            return Err(Error::InvalidConfig("an endpoint was not resolved".into()));
        }
        let mut uapi = format!("private_key={}\n", key_hex(&self.private_key)?);
        for key in OBFUSCATION_KEYS.into_iter().filter(|key| !ENGINE_UNSUPPORTED_KEYS.contains(key)) {
            if let Some(value) = self.obfuscation.get(key) {
                uapi.push_str(&format!("{}={value}\n", key.to_ascii_lowercase()));
            }
        }
        uapi.push_str("replace_peers=true\n");
        for (peer, endpoint) in self.peers.iter().zip(resolved) {
            uapi.push_str(&format!("public_key={}\n", key_hex(&peer.public_key)?));
            if let Some(psk) = &peer.preshared_key {
                uapi.push_str(&format!("preshared_key={}\n", key_hex(psk)?));
            }
            uapi.push_str(&format!("endpoint={endpoint}\n"));
            if let Some(seconds) = peer.keepalive {
                uapi.push_str(&format!("persistent_keepalive_interval={seconds}\n"));
            }
            uapi.push_str("replace_allowed_ips=true\n");
            for ip in &peer.allowed_ips {
                uapi.push_str(&format!("allowed_ip={ip}\n"));
            }
        }
        Ok(uapi)
    }

    /// Back to an awg-quick `.conf`, for sharing.
    pub fn to_conf(&self) -> String {
        let mut conf = String::from("[Interface]\n");
        conf.push_str(&format!("PrivateKey = {}\n", self.private_key));
        if !self.addresses.is_empty() {
            conf.push_str(&format!("Address = {}\n", self.addresses.join(", ")));
        }
        if !self.dns.is_empty() {
            conf.push_str(&format!("DNS = {}\n", self.dns.join(", ")));
        }
        if let Some(mtu) = self.mtu {
            conf.push_str(&format!("MTU = {mtu}\n"));
        }
        for key in OBFUSCATION_KEYS {
            if let Some(value) = self.obfuscation.get(key) {
                conf.push_str(&format!("{key} = {value}\n"));
            }
        }
        for peer in &self.peers {
            conf.push_str("\n[Peer]\n");
            conf.push_str(&format!("PublicKey = {}\n", peer.public_key));
            if let Some(psk) = &peer.preshared_key {
                conf.push_str(&format!("PresharedKey = {psk}\n"));
            }
            conf.push_str(&format!("Endpoint = {}\n", peer.endpoint));
            conf.push_str(&format!("AllowedIPs = {}\n", peer.allowed_ips.join(", ")));
            if let Some(seconds) = peer.keepalive {
                conf.push_str(&format!("PersistentKeepalive = {seconds}\n"));
            }
        }
        conf
    }

    fn validate(&self) -> Result<()> {
        key_hex(&self.private_key)?;
        if self.peers.is_empty() {
            return Err(Error::InvalidConfig("no [Peer]".into()));
        }
        for peer in &self.peers {
            key_hex(&peer.public_key)?;
            if let Some(psk) = &peer.preshared_key {
                key_hex(psk)?;
            }
            split_endpoint(&peer.endpoint)?;
        }
        for (key, value) in &self.obfuscation {
            validate_obfuscation(key, value)?;
        }
        Ok(())
    }
}

/// A WireGuard key (base64 of 32 bytes) as the hex the UAPI wants.
fn key_hex(key: &str) -> Result<String> {
    match decode_base64(key.trim()) {
        Some(bytes) if bytes.len() == 32 => Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
        _ => Err(Error::InvalidConfig("a key is not a valid WireGuard key".into())),
    }
}

fn split_endpoint(endpoint: &str) -> Result<(String, u16)> {
    let bad = || Error::InvalidConfig(format!("bad endpoint {endpoint:?}"));
    let (host, port) = endpoint.trim().rsplit_once(':').ok_or_else(bad)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = port.parse().map_err(|_| bad())?;
    if host.is_empty() {
        return Err(bad());
    }
    Ok((host.to_owned(), port))
}

fn validate_obfuscation(key: &str, value: &str) -> Result<()> {
    let bad = || Error::InvalidConfig(format!("bad AmneziaWG option {key} = {value:?}"));
    match key {
        "Jc" | "Jmin" | "Jmax" | "S1" | "S2" | "S3" | "S4" | "Itime" => {
            value.parse::<u32>().map_err(|_| bad())?;
        }
        // A message type, or a range of them (AWG 2.0).
        "H1" | "H2" | "H3" | "H4" => {
            let ok = match value.split_once('-') {
                Some((low, high)) => matches!((low.trim().parse::<u32>(), high.trim().parse::<u32>()), (Ok(l), Ok(h)) if l <= h),
                None => value.parse::<u32>().is_ok(),
            };
            if !ok {
                return Err(bad());
            }
        }
        // Signature packets in the CPS notation; one line each.
        _ => {
            if value.is_empty() || value.contains(['\n', '\r']) {
                return Err(bad());
            }
        }
    }
    Ok(())
}

/// Parses one awg-quick `.conf` (an `[Interface]` and its `[Peer]`s). Returns
/// the config, a name from a `# Name = ...` comment if there is one, and
/// warnings about options that were dropped.
pub fn parse_conf(text: &str) -> Result<(WgConfig, Option<String>, Vec<String>)> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Interface,
        Peer,
    }
    let mut section = Section::None;
    let mut name = None;
    let mut warnings = Vec::new();
    let mut private_key = None;
    let mut addresses = Vec::new();
    let mut dns = Vec::new();
    let mut mtu = None;
    let mut obfuscation = BTreeMap::new();
    let mut peers: Vec<WgPeer> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if let Some(comment) = line.strip_prefix('#') {
            if let Some((key, value)) = comment.split_once('=') {
                if key.trim().eq_ignore_ascii_case("name") && !value.trim().is_empty() {
                    name.get_or_insert_with(|| value.trim().to_owned());
                }
            }
            continue;
        }
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.eq_ignore_ascii_case("[interface]") {
            section = Section::Interface;
            continue;
        }
        if line.eq_ignore_ascii_case("[peer]") {
            section = Section::Peer;
            peers.push(WgPeer {
                public_key: String::new(),
                preshared_key: None,
                endpoint: String::new(),
                allowed_ips: Vec::new(),
                keepalive: None,
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(Error::InvalidConfig(format!("unexpected line {line:?}")));
        };
        let (key, value) = (key.trim(), value.trim());
        // Base64 keys end in `=`; split_once took the first one only.
        let list = || value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned);
        match section {
            Section::None => return Err(Error::InvalidConfig("an option before [Interface]".into())),
            Section::Interface => {
                if let Some(known) = OBFUSCATION_KEYS.iter().find(|known| known.eq_ignore_ascii_case(key)) {
                    obfuscation.insert((*known).to_owned(), value.to_owned());
                } else if key.eq_ignore_ascii_case("PrivateKey") {
                    private_key = Some(value.to_owned());
                } else if key.eq_ignore_ascii_case("Address") {
                    addresses.extend(list());
                } else if key.eq_ignore_ascii_case("DNS") {
                    dns.extend(list());
                } else if key.eq_ignore_ascii_case("MTU") {
                    mtu = Some(value.parse().map_err(|_| Error::InvalidConfig(format!("bad MTU {value:?}")))?);
                } else if !IGNORED_KEYS.iter().any(|ignored| ignored.eq_ignore_ascii_case(key)) {
                    warnings.push(format!("{key} is not supported and was ignored"));
                }
            }
            Section::Peer => {
                let peer = peers.last_mut().expect("a [Peer] was opened");
                if key.eq_ignore_ascii_case("PublicKey") {
                    peer.public_key = value.to_owned();
                } else if key.eq_ignore_ascii_case("PresharedKey") {
                    peer.preshared_key = Some(value.to_owned());
                } else if key.eq_ignore_ascii_case("Endpoint") {
                    peer.endpoint = value.to_owned();
                } else if key.eq_ignore_ascii_case("AllowedIPs") {
                    peer.allowed_ips.extend(list());
                } else if key.eq_ignore_ascii_case("PersistentKeepalive") {
                    peer.keepalive = match value.parse::<u16>() {
                        Ok(0) => None,
                        Ok(seconds) => Some(seconds),
                        Err(_) if value.eq_ignore_ascii_case("off") => None,
                        Err(_) => return Err(Error::InvalidConfig(format!("bad PersistentKeepalive {value:?}"))),
                    };
                } else {
                    warnings.push(format!("{key} is not supported and was ignored"));
                }
            }
        }
    }

    for peer in &mut peers {
        if peer.allowed_ips.is_empty() {
            peer.allowed_ips = vec!["0.0.0.0/0".into(), "::/0".into()];
        }
    }
    let config = WgConfig {
        private_key: private_key.ok_or_else(|| Error::InvalidConfig("no PrivateKey".into()))?,
        addresses,
        dns,
        mtu,
        peers,
        obfuscation,
    };
    if config.addresses.is_empty() {
        return Err(Error::InvalidConfig("no Address".into()));
    }
    config.validate()?;
    Ok((config, name, warnings))
}

/// The server for a parsed config. `link` is what to share it as again.
pub fn into_server(config: WgConfig, name: Option<String>, link: String, warnings: Vec<String>) -> Result<Server> {
    let (address, port) = config.endpoint()?;
    let amnezia = config.is_amnezia();
    let name = name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| format!("{address}:{port}"));
    Ok(Server {
        id: new_id(),
        name,
        protocol: if amnezia { "amneziawg" } else { "wireguard" }.into(),
        address,
        port,
        transport: "udp".into(),
        security: if amnezia { "awg" } else { "none" }.into(),
        source: ServerSource::WireGuard(config),
        link: Some(link),
        subscription_id: None,
        delay_ms: None,
        warnings,
    })
}

/// A `.conf` as a server.
pub fn server_from_conf(text: &str) -> Result<Server> {
    let (config, name, warnings) = parse_conf(text)?;
    let link = config.to_conf();
    into_server(config, name, link, warnings)
}

/// An AmneziaVPN key: `vpn://` + base64url of Qt's `qCompress` (a 4-byte
/// big-endian length, then zlib) of a JSON document. Its AmneziaWG or
/// WireGuard container carries the client `.conf` in `last_config`.
pub fn server_from_amnezia_key(link: &str) -> Result<Server> {
    let body = link
        .trim()
        .split_once("://")
        .map(|(_, body)| body)
        .ok_or_else(|| Error::InvalidLink("missing scheme".into()))?;
    let packed = decode_base64(body).ok_or_else(|| Error::InvalidLink("the vpn:// key is not valid base64".into()))?;
    let json = unpack_qcompress(&packed).ok_or_else(|| Error::InvalidLink("the vpn:// key does not unpack".into()))?;
    let doc: Value =
        serde_json::from_slice(&json).map_err(|err| Error::InvalidLink(format!("vpn:// key JSON: {err}")))?;

    let containers = doc["containers"].as_array().cloned().unwrap_or_default();
    let default = doc["defaultContainer"].as_str().unwrap_or_default();
    let usable = |container: &Value| -> Option<(Value, bool)> {
        match container["container"].as_str()? {
            "amnezia-awg" => Some((container["awg"].clone(), true)),
            "amnezia-wireguard" => Some((container["wireguard"].clone(), false)),
            _ => None,
        }
    };
    let (settings, _) = containers
        .iter()
        .filter(|container| container["container"].as_str() == Some(default))
        .chain(containers.iter())
        .find_map(usable)
        .ok_or_else(|| {
            let kinds: Vec<&str> = containers.iter().filter_map(|c| c["container"].as_str()).collect();
            Error::Unsupported(format!("AmneziaVPN key without AmneziaWG or WireGuard (has: {})", kinds.join(", ")))
        })?;
    // `last_config` is itself JSON, as a string; its `config` is the .conf.
    let last: Value = match &settings["last_config"] {
        Value::String(text) => {
            serde_json::from_str(text).map_err(|err| Error::InvalidLink(format!("vpn:// last_config: {err}")))?
        }
        other => other.clone(),
    };
    let conf = last["config"]
        .as_str()
        .ok_or_else(|| Error::InvalidLink("the vpn:// key carries no client config".into()))?;
    let (config, _, warnings) = parse_conf(conf)?;
    let name = doc["description"].as_str().map(str::to_owned);
    into_server(config, name, link.trim().to_owned(), warnings)
}

fn unpack_qcompress(packed: &[u8]) -> Option<Vec<u8>> {
    let (length, stream) = packed.split_first_chunk::<4>()?;
    let expected = u32::from_be_bytes(*length) as usize;
    let mut out = Vec::with_capacity(expected.min(1 << 20));
    flate2::read::ZlibDecoder::new(stream).read_to_end(&mut out).ok()?;
    (out.len() == expected).then_some(out)
}

/// `wireguard://<private key>@host:port?publickey=...&address=...#name`, the
/// form v2rayN and NekoBox share. `wg://` is the same.
pub fn server_from_link(link: &str) -> Result<Server> {
    let url = crate::links::parse_url(link)?;
    let port = url.port()?;
    let params = &url.params;
    let private_key = url
        .userinfo
        .clone()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| Error::InvalidLink("missing private key".into()))?;
    let public_key = params
        .get("publickey")
        .or_else(|| params.get("publicKey"))
        .ok_or_else(|| Error::InvalidLink("missing publickey".into()))?;
    let list = |key: &str| -> Vec<String> {
        params.get(key).map(|value| value.split(',').map(|item| item.trim().to_owned()).collect()).unwrap_or_default()
    };
    let host = if url.host.contains(':') { format!("[{}]", url.host) } else { url.host.clone() };
    let mut allowed_ips = list("allowedips");
    if allowed_ips.is_empty() {
        allowed_ips = vec!["0.0.0.0/0".into(), "::/0".into()];
    }
    let mut addresses = list("address");
    if addresses.is_empty() {
        addresses = list("ip");
    }
    let config = WgConfig {
        private_key,
        addresses,
        dns: list("dns"),
        mtu: params.get("mtu").and_then(|mtu| mtu.parse().ok()),
        peers: vec![WgPeer {
            public_key: public_key.to_owned(),
            preshared_key: params.get("presharedkey").or_else(|| params.get("psk")).map(str::to_owned),
            endpoint: format!("{host}:{port}"),
            allowed_ips,
            keepalive: params.get("keepalive").and_then(|seconds| seconds.parse().ok()),
        }],
        obfuscation: BTreeMap::new(),
    };
    if config.addresses.is_empty() {
        return Err(Error::InvalidLink("missing address".into()));
    }
    config.validate()?;
    let mut warnings = Vec::new();
    if params.get("reserved").is_some() {
        warnings.push("reserved bytes (a WARP option) are not supported and were ignored".into());
    }
    let name = Some(url.fragment.clone());
    into_server(config, name, link.trim().to_owned(), warnings)
}

/// Splits pasted text into `.conf` blocks (each from an `[Interface]` line to
/// the next one) and the remaining lines.
pub fn split_conf_blocks(text: &str) -> (Vec<String>, String) {
    let mut blocks: Vec<String> = Vec::new();
    let mut rest = String::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.trim().eq_ignore_ascii_case("[interface]") {
            blocks.push(String::new());
            in_block = true;
        } else if crate::links::is_link(line.trim()) {
            // A share link ends the block it follows.
            in_block = false;
        }
        match blocks.last_mut() {
            Some(block) if in_block => {
                block.push_str(line);
                block.push('\n');
            }
            _ => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    (blocks, rest)
}

#[cfg(test)]
mod tests;
