//! NaiveProxy servers.
//!
//! NaiveProxy is HTTP/2 (or HTTP/3) CONNECT through Chromium's own network
//! stack, so that the connection carries a browser's TLS fingerprint. Only
//! `naive` itself can make it; it runs behind the engine (`uniray-engine
//! naive`), not on Xray. Links follow the de-facto format of NekoBox and
//! others:
//!
//! ```text
//! naive+https://user:pass@host:port?extra-headers=...&insecure-concurrency=2#name
//! naive+quic://user:pass@host:port#name
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::links::parse_url;
use crate::model::{new_id, Server, ServerSource};

pub const SCHEMES: [&str; 2] = ["naive+https", "naive+quic"];

/// naive's `--insecure-concurrency` above this only makes the connection
/// stand out.
const MAX_CONCURRENCY: u32 = 8;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NaiveConfig {
    /// `https` (HTTP/2 over TLS) or `quic` (HTTP/3).
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    /// Headers sent with every CONNECT, `Name: value` lines joined by CRLF.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra_headers: String,
    /// Tunnels naive keeps open in parallel; `None` is naive's default (1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_concurrency: Option<u32>,
}

impl NaiveConfig {
    /// naive's `config.json`, connecting to `resolved` for the server's
    /// name: the address the bypass rule lets out past a TUN; naive would
    /// otherwise resolve the name itself, possibly to another address. The
    /// engine adds `listen`, a SOCKS5 server with credentials of its own.
    pub fn naive_config(&self, resolved: std::net::IpAddr) -> Value {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let proxy = format!(
            "{}://{}:{}@{host}:{}",
            self.scheme,
            encode(&self.user),
            encode(&self.pass),
            self.port
        );
        let mut config = json!({
            "proxy": proxy,
            "host-resolver-rules": format!("MAP {} {resolved}", self.host),
        });
        if !self.extra_headers.is_empty() {
            config["extra-headers"] = self.extra_headers.as_str().into();
        }
        if let Some(concurrency) = self.insecure_concurrency {
            config["insecure-concurrency"] = concurrency.into();
        }
        config
    }

    pub fn to_link(&self, name: &str) -> String {
        let mut query = Vec::new();
        if !self.extra_headers.is_empty() {
            query.push(format!("extra-headers={}", encode(&self.extra_headers)));
        }
        if let Some(concurrency) = self.insecure_concurrency {
            query.push(format!("insecure-concurrency={concurrency}"));
        }
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
        format!(
            "naive+{}://{}:{}@{host}:{}{query}#{}",
            self.scheme,
            encode(&self.user),
            encode(&self.pass),
            self.port,
            encode(name)
        )
    }
}

/// Percent-encodes everything but unreserved characters.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

pub fn server_from_link(link: &str) -> Result<Server> {
    let (scheme, _) = link.split_once("://").ok_or_else(|| Error::InvalidLink("missing scheme".into()))?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "naive+https" => "https",
        "naive+quic" => "quic",
        other => return Err(Error::UnsupportedScheme(other.into())),
    };
    let url = parse_url(link)?;
    let port = if url.port.is_empty() { 443 } else { url.port()? };
    let (user, pass) = url
        .userinfo
        .as_deref()
        .and_then(|userinfo| userinfo.split_once(':'))
        .filter(|(user, pass)| !user.is_empty() && !pass.is_empty())
        .ok_or_else(|| Error::InvalidLink("missing user or password".into()))?;
    let mut warnings = Vec::new();
    let extra_headers = url.params.get("extra-headers").unwrap_or_default().to_owned();
    if extra_headers.split("\r\n").filter(|line| !line.is_empty()).any(|line| !line.contains(':')) {
        return Err(Error::InvalidLink("extra-headers must be \"Name: value\" lines".into()));
    }
    let insecure_concurrency = match url.params.get("insecure-concurrency") {
        None | Some("") => None,
        Some(value) => match value.parse::<u32>() {
            Ok(0) | Err(_) => return Err(Error::InvalidLink(format!("bad insecure-concurrency {value:?}"))),
            Ok(value) if value > MAX_CONCURRENCY => {
                warnings.push(format!("insecure-concurrency {value} lowered to {MAX_CONCURRENCY}"));
                Some(MAX_CONCURRENCY)
            }
            Ok(value) => Some(value),
        },
    };
    let config = NaiveConfig {
        scheme: scheme.into(),
        host: url.host.clone(),
        port,
        user: user.to_owned(),
        pass: pass.to_owned(),
        extra_headers,
        insecure_concurrency,
    };
    let name = if url.fragment.trim().is_empty() { format!("{}:{port}", url.host) } else { url.fragment.clone() };
    Ok(Server {
        id: new_id(),
        name,
        protocol: "naive".into(),
        address: url.host,
        port,
        transport: if scheme == "quic" { "quic" } else { "h2" }.into(),
        security: "tls".into(),
        source: ServerSource::Naive(config),
        link: Some(link.to_owned()),
        subscription_id: None,
        delay_ms: None,
        warnings,
    })
}

#[cfg(test)]
mod tests;
