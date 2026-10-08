//! The real network beneath the VPN.
//!
//! By default the core resolves the proxy servers over encrypted DNS (DoH)
//! sent straight out the physical interface (see
//! [`uniray_core::model::Settings::direct_dns`] and `config.rs`), so that an
//! operator's resolver cannot hijack the server's address. The user may
//! choose the network's own DNS (the provider's) instead
//! ([`uniray_core::model::Settings::direct_dns_from_network`]); this module
//! finds it. Kept platform-independent so it is tested everywhere.

use std::net::IpAddr;

use serde::Deserialize;

/// The real network under the VPN, as Android reports it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[cfg_attr(desktop, allow(dead_code))]
pub struct NetworkInfo {
    /// `wifi`, `ethernet`, `cellular`, `other` or `none`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Its DNS servers, IPv4 first.
    #[serde(default)]
    pub dns: Vec<String>,
}

/// The tunnel's own addresses (see `uniray_core::config`): a DNS server there
/// is ours, not the network's.
const TUNNEL_ADDRESSES: [&str; 2] = ["172.19.0.1", "fdfe:dcba:9876::1"];

/// The DNS server to use among the network's: the first IPv4 one, else the
/// first at all. Loopback ones (a local stub such as systemd-resolved's
/// 127.0.0.53) and the tunnel's own do not count.
pub fn provider_dns(servers: &[String]) -> Option<IpAddr> {
    let usable: Vec<IpAddr> = servers
        .iter()
        .filter_map(|server| server.trim().split('%').next()?.parse::<IpAddr>().ok())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .filter(|ip| !TUNNEL_ADDRESSES.iter().any(|ours| ours.parse::<IpAddr>().ok() == Some(*ip)))
        .collect();
    usable.iter().find(|ip| ip.is_ipv4()).or_else(|| usable.first()).copied()
}

/// The `nameserver` lines of a resolv.conf.
#[cfg_attr(not(all(desktop, unix)), allow(dead_code))]
pub fn resolv_conf_servers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some("nameserver")).then(|| words.next()).flatten().map(str::to_owned)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers(list: &[&str]) -> Vec<String> {
        list.iter().map(|server| (*server).to_owned()).collect()
    }

    #[test]
    fn the_providers_dns_is_picked() {
        // IPv4 first, wherever it is listed.
        let picked = provider_dns(&servers(&["2a10:50c0::ad1:ff", "192.168.1.1", "8.8.8.8"]));
        assert_eq!(picked, Some("192.168.1.1".parse().unwrap()));
        // A local stub and the tunnel's own resolver are not the network's.
        let picked = provider_dns(&servers(&["127.0.0.53", "172.19.0.1", "fe80::1%wlan0", " 10.0.0.1 "]));
        assert_eq!(picked, Some("10.0.0.1".parse().unwrap()));
        assert_eq!(provider_dns(&servers(&["fe80::1%wlan0"])), Some("fe80::1".parse().unwrap()));
        assert_eq!(provider_dns(&servers(&["127.0.0.53", "not an address"])), None);
        assert_eq!(provider_dns(&[]), None);
    }

    #[test]
    fn resolv_conf_is_read() {
        let text = "# generated\nnameserver 192.168.0.1\nsearch lan\nnameserver  2001:db8::53 \noptions edns0\n";
        assert_eq!(resolv_conf_servers(text), ["192.168.0.1", "2001:db8::53"]);
    }

    #[test]
    fn android_network_info_carries_dns() {
        let info: NetworkInfo = serde_json::from_str(r#"{"type":"cellular","dns":["10.0.0.1"]}"#).unwrap();
        assert_eq!(info.dns, ["10.0.0.1"]);
        let older: NetworkInfo = serde_json::from_str(r#"{"type":"wifi"}"#).unwrap();
        assert!(older.dns.is_empty());
    }
}
