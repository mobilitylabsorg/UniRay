//! Routing presets and user rules -> Xray routing rules.

use serde_json::{json, Value};

use crate::config::{TAG_BLOCK, TAG_DIRECT, TAG_PROXY};
use crate::model::{Rule, RuleAction, RuleKind, RoutingPreset, Settings};

const RU_DOMAINS: [&str; 4] = ["geosite:category-ru", "domain:ru", "domain:su", "domain:xn--p1ai"];

/// User rules first (they win), then the preset.
pub fn rules(settings: &Settings) -> Vec<Value> {
    let mut rules: Vec<Value> = settings
        .custom_rules
        .iter()
        .filter(|rule| rule.enabled && !rule.value.trim().is_empty())
        .map(custom_rule)
        .collect();

    if settings.routing_preset != RoutingPreset::Global {
        rules.push(json!({ "domain": ["geosite:private"], "outboundTag": TAG_DIRECT }));
        rules.push(json!({ "ip": ["geoip:private"], "outboundTag": TAG_DIRECT }));
    }
    if settings.routing_preset == RoutingPreset::BypassRu {
        rules.push(json!({ "domain": RU_DOMAINS, "outboundTag": TAG_DIRECT }));
        rules.push(json!({ "ip": ["geoip:ru"], "outboundTag": TAG_DIRECT }));
    }
    rules
}

/// Domains that are routed directly and therefore must be resolved by the
/// direct DNS server, so that they get answers suitable for a direct connection.
pub fn direct_domains(settings: &Settings) -> Vec<String> {
    let mut domains: Vec<String> = settings
        .custom_rules
        .iter()
        .filter(|rule| {
            rule.enabled
                && rule.kind == RuleKind::Domain
                && rule.action == RuleAction::Direct
                && !rule.value.trim().is_empty()
        })
        .map(|rule| domain_matcher(&rule.value))
        .collect();
    if settings.routing_preset == RoutingPreset::BypassRu {
        domains.extend(RU_DOMAINS.iter().map(|domain| domain.to_string()));
    }
    domains
}

fn custom_rule(rule: &Rule) -> Value {
    let outbound = match rule.action {
        RuleAction::Proxy => TAG_PROXY,
        RuleAction::Direct => TAG_DIRECT,
        RuleAction::Block => TAG_BLOCK,
    };
    match rule.kind {
        RuleKind::Domain => json!({ "domain": [domain_matcher(&rule.value)], "outboundTag": outbound }),
        RuleKind::Ip => json!({ "ip": [rule.value.trim()], "outboundTag": outbound }),
    }
}

/// A bare `example.com` means the domain and its subdomains; Xray would read
/// it as a substring match, which is rarely what the user wants.
fn domain_matcher(value: &str) -> String {
    let value = value.trim();
    const PREFIXES: [&str; 7] = ["domain:", "full:", "keyword:", "regexp:", "geosite:", "ext:", "dotless:"];
    if PREFIXES.iter().any(|prefix| value.starts_with(prefix)) {
        value.to_owned()
    } else {
        format!("domain:{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::new_id;

    fn rule(kind: RuleKind, value: &str, action: RuleAction) -> Rule {
        Rule { id: new_id(), kind, value: value.into(), action, enabled: true }
    }

    #[test]
    fn global_preset_has_no_rules() {
        let settings = Settings { routing_preset: RoutingPreset::Global, ..Default::default() };
        assert!(rules(&settings).is_empty());
    }

    #[test]
    fn custom_rules_precede_preset() {
        let mut blocked = rule(RuleKind::Domain, "ads.example.com", RuleAction::Block);
        let mut settings = Settings {
            routing_preset: RoutingPreset::BypassRu,
            custom_rules: vec![blocked.clone(), rule(RuleKind::Ip, "10.1.0.0/16", RuleAction::Proxy)],
            ..Default::default()
        };
        let built = rules(&settings);
        assert_eq!(built[0], json!({ "domain": ["domain:ads.example.com"], "outboundTag": "block" }));
        assert_eq!(built[1], json!({ "ip": ["10.1.0.0/16"], "outboundTag": "proxy" }));
        assert_eq!(built.len(), 6);
        assert_eq!(built[5], json!({ "ip": ["geoip:ru"], "outboundTag": "direct" }));

        blocked.enabled = false;
        settings.custom_rules[0] = blocked;
        assert_eq!(rules(&settings).len(), 5);
    }

    #[test]
    fn prefixed_matchers_are_kept() {
        assert_eq!(domain_matcher("geosite:google"), "geosite:google");
        assert_eq!(domain_matcher(" full:a.b "), "full:a.b");
        assert_eq!(domain_matcher("a.b"), "domain:a.b");
    }

    #[test]
    fn direct_domains_feed_direct_dns() {
        let settings = Settings {
            routing_preset: RoutingPreset::BypassLan,
            custom_rules: vec![
                rule(RuleKind::Domain, "bank.example", RuleAction::Direct),
                rule(RuleKind::Domain, "x.example", RuleAction::Proxy),
            ],
            ..Default::default()
        };
        assert_eq!(direct_domains(&settings), ["domain:bank.example"]);
    }
}
