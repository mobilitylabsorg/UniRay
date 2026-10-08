//! Settings from the environment, in the style of the MikroTik container
//! images: `/container envs add list=UNIRAY key=SUB value="https://..."`.

use std::env;
use std::path::PathBuf;

pub struct Options {
    /// Subscription URLs: `SUB`, `SUB1`, `SUB_WORK`, ... (each may hold several,
    /// separated by spaces or commas).
    pub subscriptions: Vec<String>,
    /// Share links: `SRV`, `SRV1`, `SRV_TEST`, ...
    pub links: Vec<String>,
    pub web_listen: String,
    /// Protects the web UI with HTTP basic auth (any user name).
    pub web_password: Option<String>,
    /// SOCKS/HTTP proxy port for clients that are pointed at the container.
    pub mixed_port: u16,
    /// Capture forwarded LAN traffic with a TUN interface.
    pub tun: bool,
    pub tun_name: String,
    pub send_hwid: bool,
    pub hwid: Option<String>,
    pub device_model: String,
    /// Overrides the subscription's own update interval.
    pub update_hours: Option<u32>,
    pub log_level: String,
    /// Server to pick on first start: a part of its name.
    pub preferred_server: Option<String>,
    /// Optional mount that keeps the selection and the last subscription
    /// across restarts.
    pub data_dir: PathBuf,
    pub xray: PathBuf,
    /// `uniray-engine`, for WireGuard, AmneziaWG and Naive servers; such
    /// servers are left out when it is missing.
    pub engine: PathBuf,
    /// NaiveProxy's `naive`, which the engine runs for a Naive server.
    pub naive: PathBuf,
    pub remote_dns: String,
    pub direct_dns: String,
}

impl Options {
    pub fn from_env() -> Self {
        let mut subscriptions = Vec::new();
        let mut links = Vec::new();
        let mut vars: Vec<(String, String)> = env::vars().collect();
        // SUB, SUB1, SUB2 ... in a predictable order.
        vars.sort();
        for (key, value) in vars {
            let list = if key == "SUB" || key.starts_with("SUB_") || key.strip_prefix("SUB").is_some_and(starts_with_digit) {
                &mut subscriptions
            } else if key == "SRV" || key.starts_with("SRV_") || key.strip_prefix("SRV").is_some_and(starts_with_digit) {
                &mut links
            } else {
                continue;
            };
            list.extend(
                value
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned),
            );
        }

        Self {
            subscriptions,
            links,
            web_listen: format!("0.0.0.0:{}", var("WEB_PORT").unwrap_or_else(|| "9090".into())),
            web_password: var("WEB_PASSWORD"),
            mixed_port: var("MIXED_PORT").and_then(|port| port.parse().ok()).unwrap_or(1080),
            tun: flag("TUN", true),
            tun_name: var("TUN_NAME").unwrap_or_else(|| "uniray0".into()),
            send_hwid: flag("SEND_HWID", true),
            hwid: var("HWID"),
            device_model: var("DEVICE_MODEL").unwrap_or_else(|| "MikroTik".into()),
            update_hours: var("UPDATE_INTERVAL").and_then(|hours| hours.parse().ok()).filter(|hours| *hours > 0),
            log_level: var("LOG_LEVEL").unwrap_or_else(|| "warning".into()),
            preferred_server: var("SERVER"),
            data_dir: PathBuf::from(var("DATA_DIR").unwrap_or_else(|| "/data".into())),
            xray: PathBuf::from(var("XRAY_BIN").unwrap_or_else(|| "/usr/bin/xray".into())),
            engine: PathBuf::from(var("ENGINE_BIN").unwrap_or_else(|| "/usr/bin/uniray-engine".into())),
            naive: PathBuf::from(var("NAIVE_BIN").unwrap_or_else(|| "/usr/bin/naive".into())),
            remote_dns: var("REMOTE_DNS").unwrap_or_else(|| "https://1.1.1.1/dns-query".into()),
            // The container's own resolvers (`dns=` of the RouterOS container):
            // on a mobile uplink those are the operator's, the only ones that
            // may answer. They are kept out of the tunnel (see net.rs).
            direct_dns: var("DIRECT_DNS").unwrap_or_else(|| "localhost".into()),
        }
    }
}

fn starts_with_digit(rest: &str) -> bool {
    rest.starts_with(|c: char| c.is_ascii_digit())
}

fn var(key: &str) -> Option<String> {
    env::var(key).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

fn flag(key: &str, default: bool) -> bool {
    match var(key).as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => default,
    }
}
