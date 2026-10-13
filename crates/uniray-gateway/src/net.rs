//! Network plumbing of the container: forwarding, the TUN device, which
//! prefixes go into the tunnel, and delay probes that bypass it.

use std::ffi::CString;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use uniray_core::routes;
use std::time::{Duration, Instant};

/// Never sent into the tunnel: local networks (the LAN whose traffic the
/// container forwards must get its replies the normal way), loopback, link
/// local, CGNAT, multicast and reserved space.
const LOCAL_V4: [&str; 9] = [
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "224.0.0.0/3",
    "198.18.0.0/15",
];

/// The IPv4 prefixes to route into the tunnel: everything except the local
/// ranges and `also_exclude` (e.g. the container's own DNS servers, so that
/// its system resolver never depends on the tunnel it serves).
pub fn tunnel_routes(also_exclude: &[Ipv4Addr]) -> Vec<String> {
    let mut holes: Vec<(IpAddr, u8)> = LOCAL_V4.iter().filter_map(|cidr| routes::parse_cidr(cidr)).collect();
    holes.extend(also_exclude.iter().map(|ip| (IpAddr::V4(*ip), 32)));
    routes::all_but(false, &holes)
}

/// Nameservers of the container (`dns=` of the RouterOS container).
pub fn system_nameservers() -> Vec<Ipv4Addr> {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|ip| ip.trim().parse().ok())
        .collect()
}

/// The interface of the default route, before the tunnel takes over.
pub fn default_interface() -> Option<String> {
    std::fs::read_to_string("/proc/net/route")
        .ok()?
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| fields.get(1) == Some(&"00000000") && fields.get(7) == Some(&"00000000"))
        .and_then(|fields| fields.first().map(|iface| iface.to_string()))
}

/// MAC address of an interface; stable for a RouterOS veth.
pub fn mac_address(iface: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))
        .ok()
        .map(|mac| mac.trim().to_owned())
        .filter(|mac| !mac.is_empty() && mac != "00:00:00:00:00:00")
}

/// Forwarding must be on for LAN traffic to pass through the container.
/// RouterOS usually has it on already; failing to set it is only logged.
pub fn enable_forwarding() -> Result<(), String> {
    let path = "/proc/sys/net/ipv4/ip_forward";
    if std::fs::read_to_string(path).is_ok_and(|value| value.trim() == "1") {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"1"))
        .map_err(|err| format!("cannot enable IPv4 forwarding ({err}); enable it for the container"))
}

/// Creates /dev/net/tun when the container runtime did not provide it.
pub fn ensure_tun_device() -> Result<(), String> {
    let path = "/dev/net/tun";
    if std::path::Path::new(path).exists() {
        return Ok(());
    }
    std::fs::create_dir_all("/dev/net").map_err(|err| err.to_string())?;
    let c_path = CString::new(path).expect("static path");
    // SAFETY: mknod with a valid NUL-terminated path; 10:200 is the TUN device.
    let result = unsafe { libc::mknod(c_path.as_ptr(), libc::S_IFCHR | 0o666, libc::makedev(10, 200)) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!("cannot create {path}: {}", std::io::Error::last_os_error()))
    }
}

/// Time of a TCP handshake with `host:port` in milliseconds, through `iface`
/// rather than the tunnel, or `None` on failure.
pub fn tcp_ping(host: &str, port: u16, iface: Option<&str>, timeout: Duration) -> Option<u64> {
    let addr = (host, port).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)?;
    let SocketAddr::V4(addr) = addr else { return None };
    // SAFETY: plain socket syscalls on a descriptor this function owns and
    // closes on every path.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return None;
        }
        let result = (|| {
            if let Some(iface) = iface {
                let name = CString::new(iface).ok()?;
                let bytes = name.as_bytes_with_nul();
                // Without it the probe would measure the tunnel, not the server.
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_BINDTODEVICE,
                    bytes.as_ptr().cast(),
                    bytes.len() as libc::socklen_t,
                );
            }
            let sockaddr = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: port.to_be(),
                sin_addr: libc::in_addr { s_addr: u32::from(*addr.ip()).to_be() },
                sin_zero: [0; 8],
            };
            let started = Instant::now();
            let connected = libc::connect(
                fd,
                (&sockaddr as *const libc::sockaddr_in).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );
            if connected != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
                return None;
            }
            let mut poll = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
            if libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) != 1 {
                return None;
            }
            let mut error: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, (&mut error as *mut libc::c_int).cast(), &mut len);
            (error == 0).then(|| started.elapsed().as_millis() as u64)
        })();
        libc::close(fd);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(routes: &[String], ip: &str) -> bool {
        routes.iter().any(|route| routes::covers(route, ip.parse().unwrap()))
    }

    #[test]
    fn routes_cover_the_internet_but_not_local_networks() {
        let routes = tunnel_routes(&[]);
        for public in ["1.1.1.1", "8.8.8.8", "77.88.8.8", "172.15.255.255", "172.32.0.1", "192.169.0.1", "223.255.255.255"] {
            assert!(covered(&routes, public), "{public} should go into the tunnel");
        }
        for local in ["10.1.2.3", "192.168.88.10", "172.16.0.1", "172.31.255.254", "127.0.0.1", "100.64.0.1", "169.254.1.1", "239.255.255.250", "255.255.255.255"] {
            assert!(!covered(&routes, local), "{local} must not go into the tunnel");
        }
        // Few enough prefixes for a routing table.
        assert!(routes.len() < 80, "{} routes", routes.len());
    }

    #[test]
    fn extra_addresses_are_carved_out() {
        let routes = tunnel_routes(&["1.1.1.1".parse().unwrap()]);
        assert!(!covered(&routes, "1.1.1.1"));
        assert!(covered(&routes, "1.1.1.2"));
        assert!(covered(&routes, "1.0.0.1"));
    }
}
