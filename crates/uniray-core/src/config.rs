//! Server + settings -> Xray JSON config.
//!
//! The emitted format targets Xray-core 26.9: `streamSettings.method`, flat
//! outbound `settings`, the built-in `tun` inbound and `metrics.listen`.

use std::net::IpAddr;

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::model::{Server, ServerSource, Settings};
use crate::routing;

pub const TAG_PROXY: &str = "proxy";
pub const TAG_DIRECT: &str = "direct";
pub const TAG_BLOCK: &str = "block";
pub const TAG_DNS_OUT: &str = "dns-out";
pub const TAG_MIXED: &str = "mixed";
pub const TAG_TUN: &str = "tun";
/// Tag of the traffic produced by the built-in DNS client.
pub const TAG_DNS: &str = "dns";
pub const TAG_METRICS: &str = "metrics";
/// Inbound feeding the metrics service from `Runtime::metrics_socket`; its
/// connections show up in the access log under this tag.
pub const TAG_METRICS_IN: &str = "uniray-metrics";

pub const TUN_MTU: u32 = 1500;
/// Default TUN interface name. A neutral, non-VPN-looking name so that, on the
/// desktop where Xray creates the interface, it does not stand out among the
/// system interfaces. Overridable in [`Settings::interface_name`].
pub const DEFAULT_INTERFACE_NAME: &str = "rmnet3";
pub const TUN_ADDRESS_V4: &str = "172.19.0.1/30";
pub const TUN_ADDRESS_V6: &str = "fdfe:dcba:9876::1/126";

/// How system-wide traffic reaches the core's capture inbound.
#[derive(Clone, Debug, PartialEq)]
pub enum Tun {
    /// Xray creates the interface, assigns addresses and installs routes
    /// itself (desktop, needs elevated privileges).
    Managed { name: String },
    /// The OS VPN service owns the interface and hands its file descriptor to
    /// the core (Android, iOS). `name` is only a cosmetic hint in the config:
    /// the kernel names the interface itself and apps cannot rename it.
    External { name: String },
    /// Like `Managed`, but for a router-side gateway (e.g. a MikroTik
    /// container): only `routes` go into the tunnel, so that replies to the
    /// LAN clients whose traffic is forwarded through it leave by the normal
    /// interface instead of looping back in. IPv4 only.
    Gateway { name: String, routes: Vec<String> },
    /// No interface at all: a transparent-proxy (TPROXY) inbound on `port`,
    /// fed by the iptables rules the caller installs. Capturing traffic this
    /// way needs CAP_NET_ADMIN, so the core runs as a privileged standalone
    /// process (Android stealth mode). With no TUN device and no VpnService,
    /// the structural signals an on-device VPN scanner keys on are gone.
    Transparent { port: u16 },
}

/// Where the core writes its logs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum LogSink {
    /// Standard output, read through a pipe (desktop, where the core is a
    /// child process).
    #[default]
    Stdout,
    /// Files tailed by the app (mobile, where the core runs in-process).
    Files { error: String, access: String },
}

/// Everything about a run that is not a user setting.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    /// `None` runs the local mixed inbound only.
    pub tun: Option<Tun>,
    /// Port of the mixed inbound; `None` omits it.
    pub mixed_port: Option<u16>,
    /// Loopback port serving `/debug/vars` with traffic counters.
    pub metrics_port: Option<u16>,
    /// Serve the counters on this Unix socket instead of a port: nothing
    /// listens on the network, and the socket's directory decides who may
    /// connect. Takes precedence over `metrics_port`.
    pub metrics_socket: Option<String>,
    pub log: LogSink,
}

#[derive(Clone, Debug)]
pub struct Built {
    pub config: Value,
    /// Outbounds whose counters make up the proxied traffic.
    pub proxy_tags: Vec<String>,
}

pub fn build(server: &Server, settings: &Settings, runtime: &Runtime) -> Result<Built> {
    match &server.source {
        ServerSource::Outbound(outbound) => Ok(from_outbound(outbound, settings, runtime)),
        ServerSource::Config(config) => from_config(config, settings, runtime),
    }
}

fn from_outbound(outbound: &Value, settings: &Settings, runtime: &Runtime) -> Built {
    let ip_strategy = if settings.ipv6 { "UseIP" } else { "UseIPv4" };

    let mut proxy = outbound.clone();
    proxy["tag"] = TAG_PROXY.into();
    // Resolve the server through the built-in DNS (see `dns` below) instead of
    // the system resolver, whose queries may be routed back into the tunnel.
    let server_domain = proxy["settings"]["address"]
        .as_str()
        .filter(|address| address.parse::<IpAddr>().is_err())
        .map(str::to_owned);
    if server_domain.is_some() {
        proxy["streamSettings"]["sockopt"]["domainStrategy"] = ip_strategy.into();
    }

    let mut outbounds = vec![
        proxy,
        json!({
            "tag": TAG_DIRECT,
            "protocol": "freedom",
            "streamSettings": { "sockopt": { "domainStrategy": ip_strategy } },
        }),
        json!({ "tag": TAG_BLOCK, "protocol": "blackhole" }),
    ];

    let mut rules = Vec::new();
    if runtime.tun.is_some() {
        outbounds.push(json!({ "tag": TAG_DNS_OUT, "protocol": "dns" }));
        rules.push(dns_hijack_rule(TAG_DNS_OUT));
    }
    rules.push(dns_server_rule(&settings.direct_dns, TAG_DIRECT));
    rules.push(dns_server_rule(&settings.remote_dns, TAG_PROXY));
    rules.extend(routing::rules(settings));

    let mut direct_domains = routing::direct_domains(settings);
    direct_domains.extend(server_domain.map(|domain| format!("full:{domain}")));
    let mut dns_servers = vec![json!(settings.remote_dns)];
    if !direct_domains.is_empty() {
        dns_servers.insert(
            0,
            json!({ "address": settings.direct_dns, "domains": direct_domains, "skipFallback": true }),
        );
    }

    let mut config = json!({
        "dns": { "tag": TAG_DNS, "servers": dns_servers, "queryStrategy": ip_strategy },
        "inbounds": inbounds(settings, runtime),
        "outbounds": outbounds,
        "routing": { "domainStrategy": "IPIfNonMatch", "rules": rules },
    });
    apply_observability(&mut config, settings, runtime);
    Built {
        config,
        proxy_tags: vec![TAG_PROXY.into()],
    }
}

/// A complete config from a JSON subscription: its outbounds, routing and DNS
/// are the provider's business, we only plug our inbounds and observability in.
fn from_config(source: &Value, settings: &Settings, runtime: &Runtime) -> Result<Built> {
    let mut config = source.clone();
    let outbounds = config
        .get_mut("outbounds")
        .and_then(Value::as_array_mut)
        .filter(|outbounds| !outbounds.is_empty())
        .ok_or_else(|| Error::InvalidConfig("no outbounds".into()))?;

    // Stats are keyed by tag, so every proxying outbound needs one.
    let mut proxy_tags = Vec::new();
    let mut dns_tag = None;
    for (index, outbound) in outbounds.iter_mut().enumerate() {
        let protocol = outbound["protocol"].as_str().unwrap_or_default().to_owned();
        if outbound["tag"].as_str().is_none_or(str::is_empty) {
            outbound["tag"] = format!("out-{index}").into();
        }
        let tag = outbound["tag"].as_str().unwrap_or_default().to_owned();
        match protocol.as_str() {
            "freedom" | "direct" | "blackhole" | "block" | "loopback" => {}
            "dns" => dns_tag = Some(tag),
            _ => proxy_tags.push(tag),
        }
    }

    let server_domains = bootstrap_server_domains(outbounds, &proxy_tags, settings);
    let bootstrap_outbound = if server_domains.is_empty() {
        None
    } else {
        // Resolving the servers needs a way out that is not a server.
        let freedom = outbounds
            .iter()
            .find(|outbound| outbound["protocol"] == "freedom")
            .and_then(|outbound| outbound["tag"].as_str().map(str::to_owned));
        Some(freedom.unwrap_or_else(|| {
            outbounds.push(json!({ "tag": TAG_DIRECT, "protocol": "freedom" }));
            TAG_DIRECT.to_owned()
        }))
    };

    let dns_out = if runtime.tun.is_some() {
        Some(dns_tag.unwrap_or_else(|| {
            outbounds.push(json!({ "tag": TAG_DNS_OUT, "protocol": "dns" }));
            TAG_DNS_OUT.into()
        }))
    } else {
        None
    };

    let mut leading_rules = Vec::new();
    if let Some(dns_out) = &dns_out {
        leading_rules.push(dns_hijack_rule(dns_out));
    }
    if let Some(freedom) = &bootstrap_outbound {
        let dns = object_entry(&mut config, "dns");
        let dns_tag = dns
            .entry("tag")
            .or_insert_with(|| TAG_DNS.into())
            .as_str()
            .unwrap_or(TAG_DNS)
            .to_owned();
        let servers = dns.entry("servers").or_insert_with(|| json!([]));
        if let Some(servers) = servers.as_array_mut() {
            servers.insert(
                0,
                json!({ "address": settings.direct_dns, "domains": server_domains, "skipFallback": true }),
            );
        }
        let mut rule = dns_server_rule(&settings.direct_dns, freedom);
        rule["inboundTag"] = json!([dns_tag]);
        leading_rules.push(rule);
    }
    if !leading_rules.is_empty() {
        let routing = object_entry(&mut config, "routing");
        let rules = routing.entry("rules").or_insert_with(|| json!([]));
        if let Some(rules) = rules.as_array_mut() {
            rules.splice(0..0, leading_rules);
        }
    }

    // Our inbounds replace the provider's. Rules that select traffic by the
    // provider's inbound tags (e.g. "socks" -> balancer) are pointed at ours,
    // otherwise that traffic would miss them and fall to the first outbound.
    let ours = inbounds(settings, runtime);
    let our_tags: Vec<Value> = ours.iter().map(|inbound| inbound["tag"].clone()).collect();
    let provider_tags: Vec<String> = config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|inbound| inbound["tag"].as_str().map(str::to_owned))
        .collect();
    if let Some(rules) = config["routing"]["rules"].as_array_mut() {
        for rule in rules {
            retarget_inbound_tags(rule, &provider_tags, &our_tags);
        }
    }
    config["inbounds"] = ours.into();

    // `observatory` and `burstObservatory` stay: balancers with the leastPing
    // and leastLoad strategies choose servers from their measurements. Only the
    // API goes, its inbound is gone with the provider's inbounds.
    if let Some(object) = config.as_object_mut() {
        object.remove("api");
    }
    apply_observability(&mut config, settings, runtime);
    Ok(Built { config, proxy_tags })
}

/// Makes the core resolve the proxy servers' domains itself, through
/// `settings.direct_dns`, instead of the system resolver: on a phone that
/// resolver may have to be the mobile operator's, behind a router it may be
/// hijacked (fake-IP), and inside a tunnel it may depend on the tunnel it is
/// supposed to bring up. Returns the domains as DNS matchers.
fn bootstrap_server_domains(outbounds: &mut [Value], proxy_tags: &[String], settings: &Settings) -> Vec<String> {
    let strategy = if settings.ipv6 { "UseIP" } else { "UseIPv4" };
    let mut domains = Vec::new();
    for outbound in outbounds.iter_mut() {
        let is_proxy = outbound["tag"].as_str().is_some_and(|tag| proxy_tags.iter().any(|proxy| proxy == tag));
        if !is_proxy {
            continue;
        }
        let settings_of = &outbound["settings"];
        let address = [&settings_of["address"], &settings_of["vnext"][0]["address"], &settings_of["servers"][0]["address"]]
            .into_iter()
            .find_map(Value::as_str)
            .filter(|address| address.parse::<IpAddr>().is_err())
            .map(str::to_owned);
        let Some(address) = address else { continue };
        let matcher = format!("full:{address}");
        if !domains.contains(&matcher) {
            domains.push(matcher);
        }
        let sockopt = &mut outbound["streamSettings"]["sockopt"];
        if sockopt.get("domainStrategy").is_none() {
            sockopt["domainStrategy"] = strategy.into();
        }
    }
    domains
}

/// Replaces the provider's inbound tags in a routing rule with ours.
fn retarget_inbound_tags(rule: &mut Value, provider_tags: &[String], our_tags: &[Value]) {
    let Some(tags) = rule["inboundTag"].as_array() else { return };
    if !tags.iter().any(|tag| tag.as_str().is_some_and(|tag| provider_tags.iter().any(|p| p == tag))) {
        return;
    }
    let mut retargeted: Vec<Value> = Vec::new();
    for tag in tags {
        let replacement: Vec<Value> = match tag.as_str() {
            Some(tag) if provider_tags.iter().any(|p| p == tag) => our_tags.to_vec(),
            _ => vec![tag.clone()],
        };
        for tag in replacement {
            if !retargeted.contains(&tag) {
                retargeted.push(tag);
            }
        }
    }
    rule["inboundTag"] = retargeted.into();
}

fn inbounds(settings: &Settings, runtime: &Runtime) -> Vec<Value> {
    let sniffing = json!({ "enabled": true, "destOverride": ["http", "tls", "quic"] });
    let mut inbounds = Vec::new();
    if let Some(port) = runtime.mixed_port {
        inbounds.push(json!({
            "tag": TAG_MIXED,
            "listen": if settings.allow_lan { "0.0.0.0" } else { "127.0.0.1" },
            "port": port,
            "protocol": "mixed",
            "settings": mixed_settings(settings),
            "sniffing": sniffing,
        }));
    }
    match &runtime.tun {
        Some(Tun::Managed { name }) => inbounds.push(json!({
            "tag": TAG_TUN,
            "protocol": "tun",
            "settings": {
                "name": name,
                "mtu": TUN_MTU,
                "gateway": [TUN_ADDRESS_V4, TUN_ADDRESS_V6],
                // Windows only: DNS servers of the interface. Any routed
                // address works, queries are answered by the built-in DNS.
                "dns": ["1.1.1.1"],
                // Both families are captured even with IPv6 lookups off, so
                // that IPv6 traffic cannot bypass the tunnel.
                "autoSystemRoutingTable": ["0.0.0.0/0", "::/0"],
                "autoOutboundsInterface": "auto",
            },
            "sniffing": sniffing,
        })),
        Some(Tun::Gateway { name, routes }) => inbounds.push(json!({
            "tag": TAG_TUN,
            "protocol": "tun",
            "settings": {
                "name": name,
                "mtu": TUN_MTU,
                "gateway": [TUN_ADDRESS_V4],
                "autoSystemRoutingTable": routes,
                "autoOutboundsInterface": "auto",
            },
            "sniffing": sniffing,
        })),
        Some(Tun::External { name }) => inbounds.push(json!({
            "tag": TAG_TUN,
            "protocol": "tun",
            // A name is required so Xray does not list the system interfaces to
            // pick a free one, which Android forbids to apps (netlink:
            // permission denied). It stays cosmetic: the fd is already open.
            "settings": { "name": name, "mtu": TUN_MTU },
            "sniffing": sniffing,
        })),
        Some(Tun::Transparent { port }) => inbounds.push(json!({
            // Tagged like the TUN so the shared routing - the DNS hijack and
            // the retargeted provider rules - applies to it unchanged.
            "tag": TAG_TUN,
            "listen": "0.0.0.0",
            "port": port,
            "protocol": "dokodemo-door",
            // `followRedirect` recovers each packet's original destination from
            // the TPROXY mark; tproxy captures both TCP and UDP, DNS included.
            "settings": { "network": "tcp,udp", "followRedirect": true },
            "streamSettings": { "sockopt": { "tproxy": "tproxy" } },
            "sniffing": sniffing,
        })),
        None => {}
    }
    inbounds
}

/// SOCKS5/HTTP inbound settings, with a password when the user asked for one.
fn mixed_settings(settings: &Settings) -> Value {
    if settings.proxy_auth {
        json!({
            "auth": "password",
            "accounts": [{ "user": settings.proxy_user, "pass": settings.proxy_password }],
            "udp": true,
        })
    } else {
        json!({ "udp": true })
    }
}

fn apply_observability(config: &mut Value, settings: &Settings, runtime: &Runtime) {
    config["log"] = match &runtime.log {
        LogSink::Stdout => json!({ "loglevel": settings.log_level }),
        LogSink::Files { error, access } => {
            json!({ "loglevel": settings.log_level, "error": error, "access": access })
        }
    };

    if let Some(path) = &runtime.metrics_socket {
        // Xray's metrics listener only does TCP; a tunnel inbound on the
        // socket hands connections to the metrics service by its tag.
        config["metrics"] = json!({ "tag": TAG_METRICS });
        if let Some(inbounds) = config["inbounds"].as_array_mut() {
            inbounds.push(json!({
                "tag": TAG_METRICS_IN,
                "listen": path,
                "protocol": "tunnel",
                "settings": { "address": "127.0.0.1", "port": 80, "network": "tcp" },
            }));
        }
        let routing = object_entry(config, "routing");
        let rules = routing.entry("rules").or_insert_with(|| json!([]));
        if let Some(rules) = rules.as_array_mut() {
            rules.insert(0, json!({ "inboundTag": [TAG_METRICS_IN], "outboundTag": TAG_METRICS }));
        }
    } else if let Some(port) = runtime.metrics_port {
        config["metrics"] = json!({ "tag": TAG_METRICS, "listen": format!("127.0.0.1:{port}") });
    } else {
        return;
    }
    config["stats"] = json!({});
    let system = object_entry(object_entry_value(config, "policy"), "system");
    system.insert("statsOutboundUplink".into(), true.into());
    system.insert("statsOutboundDownlink".into(), true.into());
}

/// `config[key]` as an object, created or replaced when it is not one.
fn object_entry<'a>(config: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    object_entry_value(config, key)
        .as_object_mut()
        .expect("just ensured to be an object")
}

fn object_entry_value<'a>(config: &'a mut Value, key: &str) -> &'a mut Value {
    if !config[key].is_object() {
        config[key] = json!({});
    }
    &mut config[key]
}

/// Answers DNS queries arriving through the TUN with the built-in DNS.
fn dns_hijack_rule(dns_outbound: &str) -> Value {
    json!({ "inboundTag": [TAG_TUN], "port": "53", "outboundTag": dns_outbound })
}

/// Sends the built-in DNS client's own queries to `server` through `outbound`.
fn dns_server_rule(server: &str, outbound: &str) -> Value {
    let host = dns_host(server);
    let matcher = if host.parse::<IpAddr>().is_ok() { "ip" } else { "domain" };
    let value = if matcher == "ip" { host } else { format!("full:{host}") };
    json!({ "inboundTag": [TAG_DNS], matcher: [value], "outboundTag": outbound })
}

/// Host of a DNS server address such as `1.1.1.1`, `tcp://8.8.8.8:53` or
/// `https://dns.example/dns-query`.
fn dns_host(address: &str) -> String {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or_default().to_owned();
    }
    if authority.parse::<IpAddr>().is_ok() {
        return authority.to_owned();
    }
    authority.rsplit_once(':').map_or(authority, |(host, _)| host).to_owned()
}

/// Turns a provider's config into "everything through the proxy", for the
/// router gateway where split routing is not wanted. This also removes every
/// `geoip:`/`geosite:` reference, so the core runs without the geo databases.
///
/// Kept routing rules: catch-alls that send traffic to a balancer or a proxy
/// outbound (they pick which server carries everything), and the provider's
/// blocks (e.g. no BitTorrent through the VPN) unless they need geo data.
/// Dropped: everything that sends traffic `direct`, and rules that pick a
/// proxy by domain or IP. DNS servers limited to certain domains are dropped
/// too; when no server is left, `fallback_dns` is used.
pub fn proxy_everything(config: &mut Value, fallback_dns: &str) {
    let tags_of = |protocols: &[&str]| -> Vec<String> {
        config["outbounds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|outbound| outbound["protocol"].as_str().is_some_and(|protocol| protocols.contains(&protocol)))
            .filter_map(|outbound| outbound["tag"].as_str().map(str::to_owned))
            .collect()
    };
    let block_tags = tags_of(&["blackhole", "block"]);
    let local_tags = tags_of(&["freedom", "direct", "blackhole", "block", "dns", "loopback"]);
    let has_geo = |rule: &Value| {
        ["domain", "ip", "source"].iter().any(|key| {
            rule.get(*key).and_then(Value::as_array).is_some_and(|items| {
                items.iter().filter_map(Value::as_str).any(|item| {
                    item.starts_with("geoip:") || item.starts_with("geosite:") || item.starts_with("ext:")
                })
            })
        })
    };

    if let Some(rules) = config.get_mut("routing").and_then(|routing| routing.get_mut("rules")).and_then(Value::as_array_mut) {
        rules.retain(|rule| {
            let target = rule.get("outboundTag").and_then(Value::as_str);
            if target.is_some_and(|tag| block_tags.iter().any(|block| block == tag)) {
                return !has_geo(rule);
            }
            let narrows = ["domain", "ip", "source", "sourcePort", "port", "protocol", "inboundTag"]
                .iter()
                .any(|key| rule.get(*key).is_some());
            let to_balancer = rule.get("balancerTag").is_some();
            let to_proxy = target.is_some_and(|tag| !local_tags.iter().any(|local| local == tag));
            !narrows && (to_balancer || to_proxy)
        });
    }

    // A balancer with no measured server yet, or none alive, sends traffic to
    // its fallback; when that is `direct`, the traffic leaves outside the VPN.
    let first_proxy = config["outbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|outbound| outbound["tag"].as_str())
        .find(|tag| !local_tags.iter().any(|local| local == tag))
        .map(str::to_owned);
    if let Some(balancers) = config.get_mut("routing").and_then(|routing| routing.get_mut("balancers")).and_then(Value::as_array_mut) {
        for balancer in balancers {
            let fallback = balancer.get("fallbackTag").and_then(Value::as_str);
            if fallback.is_some_and(|tag| local_tags.iter().any(|local| local == tag)) {
                match &first_proxy {
                    Some(proxy) => balancer["fallbackTag"] = proxy.clone().into(),
                    None => drop(balancer.as_object_mut().map(|object| object.remove("fallbackTag"))),
                }
            }
        }
    }

    if let Some(dns) = config.get_mut("dns").and_then(Value::as_object_mut) {
        if let Some(servers) = dns.get_mut("servers").and_then(Value::as_array_mut) {
            servers.retain(|server| server.get("domains").is_none());
            for server in servers.iter_mut() {
                if let Some(server) = server.as_object_mut() {
                    for key in ["expectIPs", "expectedIPs", "unexpectedIPs"] {
                        server.remove(key);
                    }
                }
            }
            if servers.is_empty() {
                servers.push(fallback_dns.into());
            }
        }
    }
}

/// Address and port an outbound connects to, in any of the layouts Xray
/// accepts (flat settings, `vnext`, `servers`).
pub fn outbound_endpoint(outbound: &Value) -> Option<(String, u16)> {
    let settings = &outbound["settings"];
    [settings, &settings["vnext"][0], &settings["servers"][0]]
        .into_iter()
        .find_map(|candidate| {
            let address = candidate["address"].as_str()?;
            let port = u16::try_from(candidate["port"].as_u64()?).ok()?;
            Some((address.to_owned(), port))
        })
}

/// The server that actually carries traffic when `connected` (typically a
/// provider's balancer group) sent it out through its outbound `tag`: another
/// server of the list with the same endpoint, which has a real name.
pub fn exit_server<'a>(servers: &'a [Server], connected: &Server, tag: &str) -> Option<&'a Server> {
    let ServerSource::Config(config) = &connected.source else {
        return None;
    };
    let outbound = config["outbounds"].as_array()?.iter().find(|outbound| outbound["tag"] == tag)?;
    let (address, port) = outbound_endpoint(outbound)?;
    servers
        .iter()
        .find(|server| server.id != connected.id && server.address == address && server.port == port)
}

/// The outbound a delay probe should go through, tagged `tag`.
pub fn probe_outbound(server: &Server, tag: &str) -> Result<Value> {
    let mut outbound = match &server.source {
        ServerSource::Outbound(outbound) => outbound.clone(),
        ServerSource::Config(config) => config["outbounds"]
            .as_array()
            .and_then(|outbounds| outbounds.first())
            .cloned()
            .ok_or_else(|| Error::InvalidConfig("no outbounds".into()))?,
    };
    outbound["tag"] = tag.into();
    Ok(outbound)
}

/// A config that only measures delays: one loopback SOCKS inbound per server,
/// each wired to that server's outbound. `ports[i]` belongs to `servers[i]`.
pub fn build_probe(servers: &[&Server], ports: &[u16]) -> Result<Value> {
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();
    for (index, (server, port)) in servers.iter().zip(ports).enumerate() {
        let (inbound, outbound) = (format!("in-{index}"), format!("out-{index}"));
        inbounds.push(json!({
            "tag": inbound,
            "listen": "127.0.0.1",
            "port": port,
            "protocol": "mixed",
        }));
        outbounds.push(probe_outbound(server, &outbound)?);
        rules.push(json!({ "inboundTag": [inbound], "outboundTag": outbound }));
    }
    Ok(json!({
        "log": { "loglevel": "none" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "routing": { "rules": rules },
    }))
}

#[cfg(test)]
mod tests;
