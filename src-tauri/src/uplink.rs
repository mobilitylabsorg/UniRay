//! The physical network interface on desktop: the one the default route
//! leaves by, never our TUN. Android reports its network itself
//! (`core::network_info`); here the diagnostics read what kind of network it
//! is, and the speed test's direct run binds to it to measure past the tunnel.

use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uplink {
    pub name: String,
    /// `wifi`, `ethernet` or `other`, as `network::NetworkInfo::kind`.
    pub kind: &'static str,
    pub address: Option<Ipv4Addr>,
}

pub fn detect() -> Option<Uplink> {
    imp::detect()
}

/// The interface of the best default route in `/proc/net/route` text,
/// skipping those `skip` names (the TUN).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn default_route_interface(route_table: &str, skip: impl Fn(&str) -> bool) -> Option<String> {
    route_table
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.get(1) == Some(&"00000000") && fields.get(7) == Some(&"00000000"))
        .filter(|fields| !skip(fields[0]))
        .min_by_key(|fields| fields.get(6).and_then(|metric| metric.parse::<u32>().ok()).unwrap_or(u32::MAX))
        .map(|fields| fields[0].to_owned())
}

/// The first IPv4 address of an interface.
#[cfg(unix)]
fn interface_address(name: &str) -> Option<Ipv4Addr> {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list that freeifaddrs releases below.
    if unsafe { libc::getifaddrs(&mut addrs) } != 0 {
        return None;
    }
    let mut found = None;
    let mut node = addrs;
    while !node.is_null() {
        // SAFETY: a node of the list getifaddrs returned.
        let entry = unsafe { &*node };
        node = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string of the list.
        let entry_name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) };
        // SAFETY: ifa_addr points to a sockaddr; its family says which.
        if entry_name.to_bytes() == name.as_bytes() && i32::from(unsafe { (*entry.ifa_addr).sa_family }) == libc::AF_INET {
            // SAFETY: an AF_INET address is a sockaddr_in.
            let addr = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
            found = Some(Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)));
            break;
        }
    }
    // SAFETY: the list getifaddrs allocated, released once.
    unsafe { libc::freeifaddrs(addrs) };
    found
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn detect() -> Option<Uplink> {
        let routes = std::fs::read_to_string("/proc/net/route").ok()?;
        // A TUN (ours, or another VPN's) has tun_flags.
        let name = default_route_interface(&routes, |name| std::path::Path::new(&format!("/sys/class/net/{name}/tun_flags")).exists())?;
        let sys = format!("/sys/class/net/{name}");
        let kind = if std::path::Path::new(&format!("{sys}/wireless")).exists() || std::path::Path::new(&format!("{sys}/phy80211")).exists() {
            "wifi"
        } else if std::fs::read_to_string(format!("{sys}/type")).is_ok_and(|kind| kind.trim() == "1") {
            "ethernet"
        } else {
            "other"
        };
        Some(Uplink { address: interface_address(&name), name, kind })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::process::Command;

    pub fn detect() -> Option<Uplink> {
        // The default route stays on the physical interface: the TUN takes
        // the more specific halves of the address space.
        let route = Command::new("/sbin/route").args(["-n", "get", "default"]).output().ok()?;
        let route = String::from_utf8_lossy(&route.stdout);
        let name = route.lines().find_map(|line| line.trim().strip_prefix("interface:")).map(|name| name.trim().to_owned())?;
        if name.starts_with("utun") {
            return None;
        }
        let ports = Command::new("/usr/sbin/networksetup").arg("-listallhardwareports").output().ok();
        let ports = ports.map(|out| String::from_utf8_lossy(&out.stdout).into_owned()).unwrap_or_default();
        let kind = hardware_port_kind(&ports, &name);
        Some(Uplink { address: interface_address(&name), name, kind })
    }

    /// `networksetup -listallhardwareports` names each device's port:
    /// "Hardware Port: Wi-Fi" above "Device: en0".
    fn hardware_port_kind(ports: &str, device: &str) -> &'static str {
        let mut port = "";
        for line in ports.lines() {
            if let Some(name) = line.strip_prefix("Hardware Port:") {
                port = name.trim();
            } else if line.strip_prefix("Device:").map(str::trim) == Some(device) {
                return if port.eq_ignore_ascii_case("Wi-Fi") || port.eq_ignore_ascii_case("AirPort") {
                    "wifi"
                } else if port.contains("Ethernet") || port.contains("LAN") || port.contains("Thunderbolt") {
                    "ethernet"
                } else {
                    "other"
                };
            }
        }
        "other"
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_ETHERNET_CSMACD,
        IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    pub fn detect() -> Option<Uplink> {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        let mut size = 16 * 1024u32;
        let mut buffer;
        loop {
            // u64s, so that the structures (they hold pointers) are aligned.
            buffer = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: the buffer holds `size` bytes.
            let result = unsafe {
                GetAdaptersAddresses(u32::from(AF_INET), flags, std::ptr::null(), buffer.as_mut_ptr().cast(), &mut size)
            };
            match result {
                0 => break,
                // ERROR_BUFFER_OVERFLOW: `size` now says how much is needed.
                111 => continue,
                _ => return None,
            }
        }
        let mut best: Option<(u32, Uplink)> = None;
        let mut node = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !node.is_null() {
            // SAFETY: a node of the list GetAdaptersAddresses wrote.
            let adapter = unsafe { &*node };
            node = adapter.Next;
            // Up, with a default gateway, and a real NIC: Wintun and other
            // tunnels are neither Ethernet nor Wi-Fi.
            let kind = match adapter.IfType {
                IF_TYPE_IEEE80211 => "wifi",
                IF_TYPE_ETHERNET_CSMACD => "ethernet",
                _ => continue,
            };
            if adapter.OperStatus != IfOperStatusUp || adapter.FirstGatewayAddress.is_null() {
                continue;
            }
            // SAFETY: Description is a NUL-terminated wide string.
            let description = unsafe { wide(adapter.Description) };
            if description.contains("Wintun") || description.contains("WireGuard") {
                continue;
            }
            let address = unicast_ipv4(adapter);
            let metric = adapter.Ipv4Metric;
            if best.as_ref().is_none_or(|(best_metric, _)| metric < *best_metric) {
                // SAFETY: FriendlyName is a NUL-terminated wide string.
                let name = unsafe { wide(adapter.FriendlyName) };
                best = Some((metric, Uplink { name, kind, address }));
            }
        }
        best.map(|(_, uplink)| uplink)
    }

    fn unicast_ipv4(adapter: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
        let mut unicast = adapter.FirstUnicastAddress;
        while !unicast.is_null() {
            // SAFETY: a node of the adapter's unicast list.
            let entry = unsafe { &*unicast };
            unicast = entry.Next;
            let sockaddr = entry.Address.lpSockaddr;
            // SAFETY: lpSockaddr points to a SOCKADDR; its family says which.
            if !sockaddr.is_null() && unsafe { (*sockaddr).sa_family } == AF_INET {
                // SAFETY: an AF_INET address is a SOCKADDR_IN.
                let addr = unsafe { &*sockaddr.cast::<SOCKADDR_IN>() };
                // SAFETY: S_addr is the union's plain u32 view.
                return Some(Ipv4Addr::from(u32::from_be(unsafe { addr.sin_addr.S_un.S_addr })));
            }
        }
        None
    }

    /// # Safety
    /// `text` is null or a NUL-terminated UTF-16 string.
    unsafe fn wide(text: *const u16) -> String {
        if text.is_null() {
            return String::new();
        }
        let mut len = 0;
        // SAFETY: the caller's contract: NUL-terminated.
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` units before the NUL.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_route_skips_the_tunnel_and_takes_the_best_metric() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     uniray0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\n\
                     wlan0\t00000000\t010AA8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                     enp3s0\t00000000\t010AA8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                     enp3s0\t000AA8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(default_route_interface(table, |name| name == "uniray0").as_deref(), Some("enp3s0"));
        assert_eq!(default_route_interface(table, |name| name != "wlan0").as_deref(), Some("wlan0"));
        assert_eq!(default_route_interface(table, |_| true), None);
    }

    #[cfg(unix)]
    #[test]
    fn loopback_has_its_address() {
        let name = if cfg!(target_os = "macos") { "lo0" } else { "lo" };
        assert_eq!(interface_address(name), Some(Ipv4Addr::LOCALHOST));
    }
}
