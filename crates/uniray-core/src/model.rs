use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a server is built from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ServerSource {
    /// A single Xray outbound object (without `tag`), produced from a share link.
    Outbound(Value),
    /// A complete Xray config from a JSON subscription. Its outbounds, routing
    /// and DNS are used as is; inbounds are replaced with ours.
    Config(Value),
    /// A WireGuard or AmneziaWG server. Not Xray: it runs on the engine, see
    /// [`crate::wireguard`] and [`crate::engine`].
    WireGuard(crate::wireguard::WgConfig),
    /// A NaiveProxy server. Not Xray: `naive` runs it behind the engine, see
    /// [`crate::naive`].
    Naive(crate::naive::NaiveConfig),
}

impl ServerSource {
    /// Whether this server runs on Xray (else on the engine).
    pub fn is_xray(&self) -> bool {
        matches!(self, ServerSource::Outbound(_) | ServerSource::Config(_))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub name: String,
    /// `vless`, `vmess`, `trojan`, `shadowsocks`, `hysteria`, `amneziawg`,
    /// `wireguard`, `naive` or `custom`.
    pub protocol: String,
    pub address: String,
    pub port: u16,
    /// Transport method, for display (`raw`, `websocket`, `xhttp`, ...).
    pub transport: String,
    /// Transport security, for display (`none`, `tls`, `reality`).
    pub security: String,
    pub source: ServerSource,
    /// The share link the server was imported from, if any.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub subscription_id: Option<String>,
    /// Last measured delay in milliseconds, `-1` when the test failed.
    #[serde(default)]
    pub delay_ms: Option<i64>,
    /// Problems found while importing that do not prevent using the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Traffic quota reported by the panel in `subscription-userinfo`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub upload: u64,
    pub download: u64,
    /// Total quota in bytes, `0` means unlimited.
    pub total: u64,
    /// Expiration as unix seconds, `None` means never.
    pub expire: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    /// Unix seconds of the last successful update.
    #[serde(default)]
    pub updated_at: Option<i64>,
    /// Update interval advertised by the panel, in hours.
    #[serde(default)]
    pub update_interval_hours: Option<u32>,
    #[serde(default)]
    pub user_info: Option<UserInfo>,
    /// The panel reported that HWID device limit is enabled.
    #[serde(default)]
    pub hwid_active: bool,
    #[serde(default)]
    pub support_url: Option<String>,
    #[serde(default)]
    pub web_page_url: Option<String>,
    #[serde(default)]
    pub announce: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Local mixed (SOCKS/HTTP) inbound + system proxy.
    #[default]
    Proxy,
    /// TUN interface capturing all traffic.
    Tun,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RoutingPreset {
    /// Everything through the proxy.
    Global,
    /// Private networks directly.
    #[default]
    BypassLan,
    /// Private networks and Russian sites directly.
    BypassRu,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuleKind {
    Domain,
    Ip,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuleAction {
    Proxy,
    Direct,
    Block,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub kind: RuleKind,
    /// A domain (`example.com`, `full:`, `keyword:`, `regexp:`, `geosite:`) or
    /// an IP/CIDR (`10.0.0.0/8`, `geoip:`).
    pub value: String,
    pub action: RuleAction,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PerAppMode {
    #[default]
    Off,
    /// Only the listed apps go through the VPN.
    Allow,
    /// Everything except the listed apps goes through the VPN.
    Disallow,
}

/// Android per-app proxy.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerApp {
    #[serde(default)]
    pub mode: PerAppMode,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub mode: Mode,
    /// Port of the local SOCKS/HTTP inbound.
    pub mixed_port: u16,
    /// Listen on all interfaces instead of loopback.
    pub allow_lan: bool,
    /// Require a user name and password on the local SOCKS/HTTP proxy.
    pub proxy_auth: bool,
    pub proxy_user: String,
    pub proxy_password: String,
    pub routing_preset: RoutingPreset,
    pub custom_rules: Vec<Rule>,
    /// DNS server reached through the proxy.
    pub remote_dns: String,
    /// DNS server reached directly, out the physical interface. It resolves the
    /// proxy server's own address and the directly-routed domains. The default
    /// is encrypted (DoH) so that a resolver on the path - a mobile operator's,
    /// or RKN's DNS spoofing - cannot substitute a fake address for the server
    /// and break (or misdirect) the connection before the tunnel is even up.
    pub direct_dns: String,
    /// Use the current network's own DNS server (the provider's) as the
    /// direct DNS, found when connecting; `direct_dns` is kept for when there
    /// is none to be found.
    pub direct_dns_from_network: bool,
    pub ipv6: bool,
    /// Send `x-hwid` and device headers when fetching subscriptions.
    pub send_hwid: bool,
    /// Xray log level: `debug`, `info`, `warning`, `error`, `none`.
    pub log_level: String,
    /// Name of the TUN interface. On the desktop Xray creates the interface
    /// with this name, so a neutral name blends in among the system
    /// interfaces. On Android it is only the VPN session label (shown in
    /// system VPN settings) and a cosmetic config hint: the kernel names the
    /// actual interface `tunN` and apps cannot change that.
    pub interface_name: String,
    pub per_app: PerApp,
    /// Run the tunnel without a VpnService, as a privileged root process, so
    /// that on-device VPN scanners find no VpnService/`tun0`/`TRANSPORT_VPN`.
    /// Needs root (Magisk or a root-mode Shizuku); ignored, with a fall back to
    /// the VPN service, when the device has none. Android only.
    pub stealth: bool,
    /// In stealth, capture with a root-owned TUN interface instead of
    /// tproxy+iptables. The fallback for kernels/ROMs without TPROXY; the
    /// interface is then visible, under a neutral name.
    pub stealth_root_tun: bool,
    /// Keep the tunnel working with the screen off: the tunnel service holds
    /// a CPU wake lock (and a Wi-Fi lock) while connected, so the phone's
    /// sleep does not stall it and drop the connections. Costs battery, so
    /// off by default. Android only.
    pub keep_awake: bool,
    pub selected_server: Option<String>,
    /// UI language, `None` follows the system.
    pub locale: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Proxy,
            mixed_port: 10808,
            allow_lan: false,
            proxy_auth: false,
            proxy_user: "uniray".into(),
            // Random per install; only used once the user turns auth on.
            proxy_password: new_id().replace('-', "")[..16].to_owned(),
            routing_preset: RoutingPreset::default(),
            custom_rules: Vec::new(),
            remote_dns: "https://1.1.1.1/dns-query".into(),
            // A different host than remote_dns, so one routing rule can send
            // this resolver's queries out directly while remote_dns's go
            // through the proxy (both are the 53-less DoH port 443).
            direct_dns: "https://8.8.8.8/dns-query".into(),
            direct_dns_from_network: false,
            ipv6: false,
            send_hwid: true,
            log_level: "warning".into(),
            interface_name: crate::config::DEFAULT_INTERFACE_NAME.into(),
            per_app: PerApp::default(),
            stealth: false,
            stealth_root_tun: false,
            keep_awake: false,
            selected_server: None,
            locale: None,
        }
    }
}

fn yes() -> bool {
    true
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
