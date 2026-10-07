//! A forgiving splitter for proxy share links.
//!
//! Share links are URL-shaped but routinely break URL rules (base64 with `/`
//! in the userinfo, several ports, a stray `/` before the query), so they are
//! taken apart by hand instead of with a strict URL parser.

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;
use percent_encoding::percent_decode_str;

use crate::error::{Error, Result};

#[derive(Debug, Default)]
pub(crate) struct Params(Vec<(String, String)>);

impl Params {
    pub fn parse(query: &str) -> Self {
        Self(
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (decode(key), decode(value))
                })
                .collect(),
        )
    }

    /// The value of `key`, treating an empty value as absent.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.as_str())
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1" | "true"))
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RawUrl {
    /// Percent-decoded userinfo.
    pub userinfo: Option<String>,
    /// Host without IPv6 brackets.
    pub host: String,
    /// Port as written; may hold several ports for Hysteria port hopping.
    pub port: String,
    pub params: Params,
    /// Percent-decoded fragment, conventionally the server name.
    pub fragment: String,
}

impl RawUrl {
    pub fn port(&self) -> Result<u16> {
        self.port
            .parse()
            .map_err(|_| Error::InvalidLink(format!("bad port {:?}", self.port)))
    }
}

/// Splits `scheme://body?query#fragment` without interpreting the body.
pub(crate) fn split(link: &str) -> Result<(&str, &str, &str, String)> {
    let (scheme, rest) = link
        .split_once("://")
        .ok_or_else(|| Error::InvalidLink("missing scheme".into()))?;
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (body, query) = rest.split_once('?').unwrap_or((rest, ""));
    Ok((scheme, body, query, decode(fragment)))
}

pub(crate) fn parse(link: &str) -> Result<RawUrl> {
    let (_, body, query, fragment) = split(link)?;
    let mut url = parse_authority(body)?;
    url.params = Params::parse(query);
    url.fragment = fragment;
    Ok(url)
}

/// Parses `[userinfo@]host:port[/...]`.
pub(crate) fn parse_authority(body: &str) -> Result<RawUrl> {
    let (userinfo, hostport) = match body.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(decode(userinfo)), hostport),
        None => (None, body),
    };
    let hostport = hostport.split('/').next().unwrap_or_default();
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (host, port) = rest
            .split_once(']')
            .ok_or_else(|| Error::InvalidLink("unterminated IPv6 address".into()))?;
        (host, port.strip_prefix(':').unwrap_or_default())
    } else {
        hostport.rsplit_once(':').unwrap_or((hostport, ""))
    };
    if host.is_empty() {
        return Err(Error::InvalidLink("missing host".into()));
    }
    Ok(RawUrl {
        userinfo,
        host: host.to_owned(),
        port: port.to_owned(),
        params: Params::default(),
        fragment: String::new(),
    })
}

pub(crate) fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// Decodes base64 in any of the flavours found in the wild: standard or
/// URL-safe alphabet, with or without padding, possibly wrapped.
pub(crate) fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let cleaned: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    STANDARD_NO_PAD
        .decode(&cleaned)
        .or_else(|_| URL_SAFE_NO_PAD.decode(&cleaned))
        .ok()
}

pub(crate) fn decode_base64_str(value: &str) -> Option<String> {
    String::from_utf8(decode_base64(value)?).ok()
}
