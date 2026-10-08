//! A run of `uniray-engine` (`engine/` in the repository) for the servers
//! Xray cannot run: what the engine is told and what Xray's config needs.
//!
//! The engine gets a JSON file it deletes once read: its SOCKS credentials,
//! the port and the tunnel. Xray gets [`config::Engine`]: the same
//! credentials and the server endpoints to send around the tunnel.

use std::net::IpAddr;

use serde_json::{json, Value};

use crate::config::{self, Bypass};
use crate::error::Result;
use crate::model::{Server, ServerSource};

/// How the engine tells Xray's connections from everyone else's.
#[derive(Clone, Debug, PartialEq)]
pub enum Peers {
    /// Xray's process ids: given now, or later over the engine's stdin
    /// (`peer <pid>`) when Xray starts after the engine.
    Pids(Vec<u32>),
    /// Android: the connections of `uid` - the app's own in VPN mode, where
    /// Xray runs inside the app, root's in stealth mode. Their owner comes
    /// from `socket`, where the VPN service answers (Android 10+ hides
    /// /proc/net from apps), or from /proc/net.
    Uid { uid: u32, socket: Option<String> },
}

#[derive(Clone, Debug)]
pub struct EngineRun {
    /// The engine's mode, its first argument.
    pub mode: &'static str,
    /// The engine's config, without [`Peers`]; see [`Self::config_for`].
    config: Value,
    /// What Xray's config needs, for [`config::Runtime::engine`].
    pub runtime: config::Engine,
    /// For the log: what of the server the engine cannot honour.
    pub warnings: Vec<String>,
}

/// The engine run for `server`, or `None` when Xray runs it itself. Resolves
/// the server's name, so it blocks.
pub fn prepare(server: &Server, port: u16, ipv6: bool) -> Result<Option<EngineRun>> {
    let ServerSource::WireGuard(wireguard) = &server.source else {
        return Ok(None);
    };
    let endpoints = wireguard.resolve_endpoints(ipv6)?;
    let (user, pass) = ("uniray".to_owned(), secret());
    let engine = json!({
        "user": user,
        "pass": pass,
        "port": port,
        "awg": {
            "uapi": wireguard.to_uapi(&endpoints)?,
            "addresses": wireguard.addresses,
            "dns": wireguard.dns.iter().filter(|dns| dns.parse::<IpAddr>().is_ok()).collect::<Vec<_>>(),
            "mtu": wireguard.mtu.unwrap_or(1420),
        },
    });
    let warnings = wireguard
        .unsupported_options()
        .into_iter()
        .map(|key| format!("[Warning] engine: the AmneziaWG option {key} is from AWG 1.5 and is not used"))
        .collect();
    Ok(Some(EngineRun {
        mode: "awg",
        config: engine,
        runtime: config::Engine {
            port,
            user,
            pass,
            bypass: endpoints.iter().map(|addr| Bypass { addr: *addr, network: "udp" }).collect(),
            block_udp: false,
        },
        warnings,
    }))
}

impl EngineRun {
    /// The engine's config file, telling it who may connect. A `detached`
    /// engine keeps running when its stdin closes (it is stopped by a kill).
    pub fn config_for(&self, peers: &Peers, detached: bool) -> Vec<u8> {
        let mut engine = self.config.clone();
        match peers {
            Peers::Pids(pids) => engine["peers"] = json!(pids),
            Peers::Uid { uid, socket } => {
                engine["ownerUid"] = (*uid).into();
                if let Some(socket) = socket {
                    engine["ownerSocket"] = socket.as_str().into();
                }
            }
        }
        if detached {
            engine["detached"] = true.into();
        }
        serde_json::to_vec(&engine).expect("a JSON value serializes")
    }
}

/// 244 random bits from the OS generator, as 64 hex digits: two version-4
/// UUIDs, which `uuid` draws from it.
pub fn secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::parse_link;
    use crate::wireguard::server_from_conf;

    const CONF: &str = "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nAddress = 10.8.0.2/32\n\
                        DNS = 1.1.1.1, corp.example\nJc = 4\nItime = 60\n\
                        [Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nEndpoint = 203.0.113.9:51820\n\
                        AllowedIPs = 0.0.0.0/0\n";

    #[test]
    fn xray_servers_need_no_engine() {
        let server = parse_link("trojan://pw@203.0.113.1:443#T").unwrap();
        assert!(prepare(&server, 1, false).unwrap().is_none());
    }

    #[test]
    fn wireguard_run_carries_credentials_tunnel_and_bypass() {
        let server = server_from_conf(CONF).unwrap();
        let run = prepare(&server, 40123, false).unwrap().unwrap();
        let engine: Value = serde_json::from_slice(&run.config_for(&Peers::Pids(vec![77]), false)).unwrap();
        assert_eq!(run.mode, "awg");
        assert_eq!(engine["port"], 40123);
        assert_eq!(engine["peers"], json!([77]));
        assert_eq!((engine["user"].as_str(), engine["pass"].as_str()), (Some(run.runtime.user.as_str()), Some(run.runtime.pass.as_str())));
        assert_eq!(run.runtime.pass.len(), 64);
        assert!(engine["awg"]["uapi"].as_str().unwrap().contains("endpoint=203.0.113.9:51820\n"));
        // Names cannot be used inside the tunnel's stack; only addresses go.
        assert_eq!(engine["awg"]["dns"], json!(["1.1.1.1"]));
        assert_eq!(run.runtime.bypass, [Bypass { addr: "203.0.113.9:51820".parse().unwrap(), network: "udp" }]);
        assert_eq!(run.warnings.len(), 1);

        let owner = Peers::Uid { uid: 10123, socket: Some("@uniray-owner".into()) };
        let engine: Value = serde_json::from_slice(&run.config_for(&owner, false)).unwrap();
        assert_eq!((engine["ownerSocket"].as_str(), engine["ownerUid"].as_u64()), (Some("@uniray-owner"), Some(10123)));
        assert!(engine.get("peers").is_none() && engine.get("detached").is_none());

        // Stealth: root's connections, told by /proc/net; outlives the script.
        let root = Peers::Uid { uid: 0, socket: None };
        let engine: Value = serde_json::from_slice(&run.config_for(&root, true)).unwrap();
        assert_eq!((engine["ownerUid"].as_u64(), engine.get("ownerSocket")), (Some(0), None));
        assert_eq!(engine["detached"], true);
    }

    #[test]
    fn secrets_differ() {
        assert_ne!(secret(), secret());
    }
}
