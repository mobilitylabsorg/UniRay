//! Subscription responses -> servers and metadata.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::links::{self, decode_base64_str};
use crate::model::{new_id, Server, ServerSource, Subscription, UserInfo};

/// What the panel says about the subscription in response headers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: Option<String>,
    pub update_interval_hours: Option<u32>,
    pub user_info: Option<UserInfo>,
    pub hwid_active: bool,
    pub support_url: Option<String>,
    pub web_page_url: Option<String>,
    pub announce: Option<String>,
}

impl Meta {
    /// Stores the metadata of a successful update in `subscription`.
    pub fn apply(self, subscription: &mut Subscription, now: i64) {
        if let Some(title) = self.title {
            subscription.name = title;
        }
        subscription.update_interval_hours = self.update_interval_hours;
        subscription.user_info = self.user_info;
        subscription.hwid_active = self.hwid_active;
        subscription.support_url = self.support_url;
        subscription.web_page_url = self.web_page_url;
        subscription.announce = self.announce;
        subscription.updated_at = Some(now);
        subscription.last_error = None;
    }
}

#[derive(Debug)]
pub struct Parsed {
    pub servers: Vec<Server>,
    /// Entries that could not be imported, with the reason.
    pub errors: Vec<String>,
    pub meta: Meta,
}

/// Interprets a subscription response. `headers` must have lowercase names;
/// `hwid_sent` tells whether the request carried `x-hwid`.
pub fn parse_response(
    status: u16,
    headers: &HashMap<String, String>,
    body: &str,
    hwid_sent: bool,
) -> Result<Parsed> {
    let flag = |name: &str| headers.get(name).is_some_and(|value| value.trim() == "true");

    // Remnawave's HWID verdicts come first: with them the body, if any, holds
    // placeholder servers rather than the subscription.
    if flag("x-hwid-max-devices-reached") || flag("x-hwid-limit") {
        return Err(Error::HwidLimitReached);
    }
    if flag("x-hwid-not-supported") {
        return Err(Error::HwidRequired);
    }
    if status == 404 && !hwid_sent {
        return Err(Error::NotFoundWithoutHwid);
    }
    if !(200..300).contains(&status) {
        return Err(Error::Http(status));
    }

    let (servers, errors) = parse_body(body);
    if servers.is_empty() {
        return Err(Error::EmptySubscription);
    }
    Ok(Parsed {
        servers,
        errors,
        meta: parse_meta(headers),
    })
}

fn parse_meta(headers: &HashMap<String, String>) -> Meta {
    let text = |name: &str| {
        headers
            .get(name)
            .map(|value| decode_header(value))
            .filter(|value| !value.is_empty())
    };
    Meta {
        title: text("profile-title"),
        update_interval_hours: headers
            .get("profile-update-interval")
            .and_then(|value| value.trim().parse().ok())
            .filter(|hours| *hours > 0),
        user_info: headers.get("subscription-userinfo").map(|value| parse_user_info(value)),
        hwid_active: headers.get("x-hwid-active").is_some_and(|value| value.trim() == "true"),
        support_url: text("support-url"),
        web_page_url: text("profile-web-page-url"),
        announce: text("announce"),
    }
}

/// Header values are either plain text or `base64:<utf-8 text>`.
fn decode_header(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix("base64:")
        .and_then(decode_base64_str)
        .unwrap_or_else(|| value.to_owned())
        .trim()
        .to_owned()
}

/// `upload=1; download=2; total=3; expire=1700000000`
fn parse_user_info(value: &str) -> UserInfo {
    let mut info = UserInfo::default();
    for (key, value) in value.split(';').filter_map(|pair| pair.trim().split_once('=')) {
        let value = value.trim();
        match key.trim() {
            "upload" => info.upload = value.parse().unwrap_or(0),
            "download" => info.download = value.parse().unwrap_or(0),
            "total" => info.total = value.parse().unwrap_or(0),
            "expire" => info.expire = value.parse().ok().filter(|expire| *expire > 0),
            _ => {}
        }
    }
    info
}

/// Prepares freshly fetched servers to replace the subscription's `old` ones.
/// A server that is still there keeps its id and measured delay, so that the
/// selection and the live connection survive an update.
pub fn reconcile(old: &[Server], mut new: Vec<Server>, subscription_id: &str) -> Vec<Server> {
    let same = |a: &Server, b: &Server| {
        (a.link.is_some() && a.link == b.link)
            || (a.name == b.name && a.protocol == b.protocol && a.address == b.address && a.port == b.port)
    };
    let mut claimed = HashSet::new();
    for server in &mut new {
        server.subscription_id = Some(subscription_id.to_owned());
        let previous = old
            .iter()
            .find(|candidate| !claimed.contains(candidate.id.as_str()) && same(candidate, server));
        if let Some(previous) = previous {
            claimed.insert(previous.id.as_str());
            server.id = previous.id.clone();
            server.delay_ms = previous.delay_ms;
        }
    }
    new
}

/// Parses a subscription body or any pasted text: a list of share links, a
/// JSON Xray config (or an array of them), or base64 of either. Returns the
/// servers and the entries that could not be imported.
pub fn parse_body(body: &str) -> (Vec<Server>, Vec<String>) {
    let body = body.trim().trim_start_matches('\u{feff}');
    if let Some(parsed) = parse_plain(body) {
        return parsed;
    }
    decode_base64_str(body)
        .and_then(|decoded| parse_plain(decoded.trim()))
        .unwrap_or_default()
}

fn parse_plain(text: &str) -> Option<(Vec<Server>, Vec<String>)> {
    if text.starts_with('[') || text.starts_with('{') {
        let doc: Value = serde_json::from_str(text).ok()?;
        let configs = match doc {
            Value::Array(configs) => configs,
            config => vec![config],
        };
        let mut servers = Vec::new();
        let mut errors = Vec::new();
        for config in configs {
            match config_server(config) {
                Ok(server) => servers.push(server),
                Err(err) => errors.push(err.to_string()),
            }
        }
        return Some((servers, errors));
    }
    if text.lines().any(|line| links::is_link(line.trim())) {
        let imported = links::parse_many(text);
        return Some((imported.servers, imported.errors));
    }
    None
}

/// Wraps a complete Xray config from a JSON subscription into a server.
fn config_server(mut config: Value) -> Result<Server> {
    let mut warnings = Vec::new();
    if strip_allow_insecure(&mut config) {
        warnings.push(
            "the config asks to skip TLS certificate verification (allowInsecure), which Xray no longer supports; the certificate will be verified"
                .to_owned(),
        );
    }
    let outbound = config["outbounds"]
        .as_array()
        .and_then(|outbounds| outbounds.first())
        .ok_or_else(|| Error::InvalidConfig("no outbounds".into()))?;

    // Both the flat settings and the legacy `vnext`/`servers` wrappers occur.
    let settings = &outbound["settings"];
    let endpoint = [settings, &settings["vnext"][0], &settings["servers"][0]]
        .into_iter()
        .find(|candidate| candidate["address"].is_string());
    let address = endpoint
        .and_then(|endpoint| endpoint["address"].as_str())
        .unwrap_or_default()
        .to_owned();
    let port = endpoint
        .and_then(|endpoint| endpoint["port"].as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or(0);

    let stream = &outbound["streamSettings"];
    let transport = stream["method"]
        .as_str()
        .or_else(|| stream["network"].as_str())
        .unwrap_or("raw")
        .to_owned();
    let security = stream["security"].as_str().unwrap_or("none").to_owned();
    // A config that routes through a balancer is a group of servers, not the
    // first outbound it happens to list.
    let protocol = if config["routing"]["balancers"].as_array().is_some_and(|balancers| !balancers.is_empty()) {
        "balancer".to_owned()
    } else {
        outbound["protocol"].as_str().unwrap_or("custom").to_owned()
    };
    let name = config["remarks"]
        .as_str()
        .map(str::trim)
        .filter(|remarks| !remarks.is_empty())
        .map_or_else(|| format!("{address}:{port}"), str::to_owned);

    Ok(Server {
        id: new_id(),
        name,
        protocol,
        address,
        port,
        transport,
        security,
        source: ServerSource::Config(config),
        link: None,
        subscription_id: None,
        delay_ms: None,
        warnings,
    })
}

/// Removes `allowInsecure` wherever it occurs; Xray refuses to start with it.
/// Returns whether any occurrence was actually enabling it.
fn strip_allow_insecure(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => {
            let mut enabled = object.remove("allowInsecure") == Some(Value::Bool(true));
            for child in object.values_mut() {
                enabled |= strip_allow_insecure(child);
            }
            enabled
        }
        Value::Array(items) => items.iter_mut().fold(false, |enabled, item| strip_allow_insecure(item) | enabled),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
