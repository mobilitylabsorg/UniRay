//! Routes into a tunnel that leave some addresses out: the rest of an address
//! family, as the fewest CIDR prefixes that cover it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A prefix: its first address and its length. IPv4 is in the low 32 bits.
type Prefix = (u128, u8);

/// `cidr` (`"10.0.0.0/8"`, `"2001:db8::/32"`) as an address and a length.
pub fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let (ip, len) = cidr.split_once('/')?;
    let ip: IpAddr = ip.parse().ok()?;
    let len: u8 = len.parse().ok().filter(|len| *len <= bits(ip.is_ipv6()))?;
    Some((ip, len))
}

/// Whether the prefix `cidr` covers `ip`.
pub fn covers(cidr: &str, ip: IpAddr) -> bool {
    parse_cidr(cidr).is_some_and(|(net, len)| {
        net.is_ipv6() == ip.is_ipv6() && contains(prefix(net, len), prefix(ip, bits(ip.is_ipv6())), ip.is_ipv6())
    })
}

/// All of IPv4 (or IPv6 with `v6`) but `holes`, which may be of either
/// family: the other family's are ignored. Without holes, `0.0.0.0/0` or
/// `::/0`.
pub fn all_but(v6: bool, holes: &[(IpAddr, u8)]) -> Vec<String> {
    let mut blocks = vec![(0, 0)];
    for &(ip, len) in holes.iter().filter(|(ip, _)| ip.is_ipv6() == v6) {
        let hole = prefix(ip, len);
        blocks = blocks.into_iter().flat_map(|block| subtract(block, hole, v6)).collect();
    }
    blocks.sort();
    blocks.into_iter().map(|(net, len)| format!("{}/{len}", address(net, v6))).collect()
}

fn bits(v6: bool) -> u8 {
    if v6 { 128 } else { 32 }
}

fn mask(len: u8, v6: bool) -> u128 {
    let full = if v6 { u128::MAX } else { u128::from(u32::MAX) };
    if len == 0 { 0 } else { (u128::MAX << (bits(v6) - len)) & full }
}

fn prefix(ip: IpAddr, len: u8) -> Prefix {
    let net = match ip {
        IpAddr::V4(ip) => u128::from(u32::from(ip)),
        IpAddr::V6(ip) => u128::from(ip),
    };
    (net & mask(len, ip.is_ipv6()), len)
}

fn address(net: u128, v6: bool) -> IpAddr {
    if v6 { Ipv6Addr::from(net).into() } else { Ipv4Addr::from(net as u32).into() }
}

fn contains(outer: Prefix, inner: Prefix, v6: bool) -> bool {
    outer.1 <= inner.1 && inner.0 & mask(outer.1, v6) == outer.0
}

fn subtract(block: Prefix, hole: Prefix, v6: bool) -> Vec<Prefix> {
    if contains(hole, block, v6) {
        return Vec::new();
    }
    if !contains(block, hole, v6) {
        return vec![block];
    }
    let (net, len) = block;
    let halves = [(net, len + 1), (net | 1 << (bits(v6) - 1 - len), len + 1)];
    halves.into_iter().flat_map(|half| subtract(half, hole, v6)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(routes: &[String], ip: &str) -> bool {
        let ip: IpAddr = ip.parse().unwrap();
        routes.iter().any(|route| covers(route, ip))
    }

    #[test]
    fn without_holes_everything() {
        assert_eq!(all_but(false, &[]), ["0.0.0.0/0"]);
        assert_eq!(all_but(true, &[]), ["::/0"]);
    }

    #[test]
    fn holes_are_left_out_and_the_rest_kept() {
        let holes = [("203.0.113.9".parse().unwrap(), 32), ("2001:db8::9".parse().unwrap(), 128)];
        let v4 = all_but(false, &holes);
        assert!(!covered(&v4, "203.0.113.9"));
        for ip in ["203.0.113.8", "203.0.113.10", "1.1.1.1", "255.255.255.255", "0.0.0.0"] {
            assert!(covered(&v4, ip), "{ip}");
        }
        assert_eq!(v4.len(), 32);
        let v6 = all_but(true, &holes);
        assert!(!covered(&v6, "2001:db8::9"));
        for ip in ["2001:db8::8", "2001:db8::a", "2606:4700::1111", "::1"] {
            assert!(covered(&v6, ip), "{ip}");
        }
        assert_eq!(v6.len(), 128);
        // Neither family's routes cover the other's addresses.
        assert!(!covered(&v4, "2606:4700::1111") && !covered(&v6, "1.1.1.1"));
    }

    #[test]
    fn prefixes_do_not_overlap() {
        let holes: Vec<_> = ["10.0.0.0/8", "192.168.0.0/16", "1.1.1.1/32"].iter().map(|cidr| parse_cidr(cidr).unwrap()).collect();
        let routes = all_but(false, &holes);
        let prefixes: Vec<Prefix> = routes.iter().map(|route| parse_cidr(route).map(|(ip, len)| prefix(ip, len)).unwrap()).collect();
        for (i, a) in prefixes.iter().enumerate() {
            for b in &prefixes[i + 1..] {
                assert!(!contains(*a, *b, false) && !contains(*b, *a, false), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn cidrs_parse() {
        assert_eq!(parse_cidr("10.0.0.0/8"), Some(("10.0.0.0".parse().unwrap(), 8)));
        assert_eq!(parse_cidr("::/0"), Some(("::".parse().unwrap(), 0)));
        assert_eq!(parse_cidr("10.0.0.0/33"), None);
        assert_eq!(parse_cidr("10.0.0.0"), None);
    }
}
