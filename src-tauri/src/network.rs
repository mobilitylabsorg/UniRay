//! Which DNS server resolves the VPN servers, depending on the network the
//! device is on. Used by the mobile core; kept platform-independent so that
//! it is tested everywhere.
#![cfg_attr(desktop, allow(dead_code))]

use std::net::IpAddr;

use serde::Deserialize;
use uniray_core::model::Settings;

/// The real network under the VPN, as Android reports it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NetworkInfo {
    /// `wifi`, `ethernet`, `cellular`, `other` or `none`.
    #[serde(rename = "type")]
    pub kind: String,
    pub dns: Vec<String>,
}

impl NetworkInfo {
    pub fn is_cellular(&self) -> bool {
        self.kind == "cellular"
    }

    /// The operator's DNS server, preferring IPv4.
    pub fn operator_dns(&self) -> Option<&str> {
        let ips = || self.dns.iter().filter(|dns| dns.parse::<IpAddr>().is_ok());
        ips()
            .find(|dns| dns.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv4()))
            .or_else(|| ips().next())
            .map(String::as_str)
    }
}

/// The DNS server the core resolves the VPN servers (and direct sites) with.
/// On mobile data it must be the operator's: other resolvers may be
/// unreachable there. Elsewhere any works, so the configured one is used.
pub fn direct_dns(settings: &Settings, network: Option<&NetworkInfo>) -> String {
    network
        .filter(|network| network.is_cellular())
        .and_then(NetworkInfo::operator_dns)
        .map_or_else(|| settings.direct_dns.clone(), str::to_owned)
}

/// Whether a change of network calls for another DNS server, and with it a
/// reconnect: switching between mobile data and anything else, or getting
/// different operator DNS on mobile data.
pub fn needs_reconnect(before: &NetworkInfo, now: &NetworkInfo) -> bool {
    if now.kind == "none" {
        return false;
    }
    before.is_cellular() != now.is_cellular() || (now.is_cellular() && before.operator_dns() != now.operator_dns())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(kind: &str, dns: &[&str]) -> NetworkInfo {
        NetworkInfo { kind: kind.into(), dns: dns.iter().map(|dns| dns.to_string()).collect() }
    }

    #[test]
    fn mobile_data_uses_the_operator_dns() {
        let settings = Settings::default();
        let cellular = network("cellular", &["2a00:1fa0::1", "10.20.30.40", "10.20.30.41"]);
        assert_eq!(direct_dns(&settings, Some(&cellular)), "10.20.30.40");
        assert_eq!(direct_dns(&settings, Some(&network("cellular", &["2a00:1fa0::1"]))), "2a00:1fa0::1");
        // Without operator DNS, the configured server is the only option.
        assert_eq!(direct_dns(&settings, Some(&network("cellular", &[]))), settings.direct_dns);
    }

    #[test]
    fn other_networks_use_the_configured_dns() {
        let settings = Settings { direct_dns: "https://8.8.8.8/dns-query".into(), ..Settings::default() };
        assert_eq!(direct_dns(&settings, Some(&network("wifi", &["192.168.10.1"]))), "https://8.8.8.8/dns-query");
        assert_eq!(direct_dns(&settings, None), "https://8.8.8.8/dns-query");
    }

    #[test]
    fn reconnects_only_when_the_dns_choice_changes() {
        let wifi = network("wifi", &["192.168.10.1"]);
        let other_wifi = network("wifi", &["192.168.1.1"]);
        let cellular = network("cellular", &["10.20.30.40"]);
        let other_operator = network("cellular", &["10.99.0.1"]);
        assert!(needs_reconnect(&wifi, &cellular));
        assert!(needs_reconnect(&cellular, &wifi));
        assert!(needs_reconnect(&cellular, &other_operator));
        assert!(!needs_reconnect(&wifi, &other_wifi));
        assert!(!needs_reconnect(&cellular, &cellular.clone()));
        // A moment without any network is not a reason to reconnect.
        assert!(!needs_reconnect(&cellular, &network("none", &[])));
    }
}
