//! Share link query parameters -> Xray `streamSettings`.

use serde_json::{json, Map, Value};

use super::url::Params;
use crate::error::{Error, Result};

pub(crate) struct Stream {
    pub settings: Value,
    pub transport: &'static str,
    pub security: &'static str,
}

/// Builds `streamSettings` from the conventional share link parameters
/// (`type`, `security`, `sni`, `fp`, `pbk`, `path`, `host`, ...).
pub(crate) fn build(params: &Params, default_security: &str, warnings: &mut Vec<String>) -> Result<Stream> {
    let transport = match params.get("type").unwrap_or("tcp") {
        "tcp" | "raw" => "raw",
        "ws" | "websocket" => "websocket",
        "grpc" | "gun" => "grpc",
        "httpupgrade" => "httpupgrade",
        "xhttp" | "splithttp" => "xhttp",
        "kcp" | "mkcp" => "mkcp",
        other => return Err(Error::Unsupported(format!("transport {other}"))),
    };
    let mut stream = Map::new();
    stream.insert("method".into(), transport.into());

    let host = params.get("host");
    let path = params.get("path");
    match transport {
        "raw" => {
            if params.get("headerType") == Some("http") {
                let mut request = Map::new();
                request.insert("path".into(), list(path.unwrap_or("/")));
                if let Some(host) = host {
                    request.insert("headers".into(), json!({ "Host": list(host) }));
                }
                stream.insert(
                    "rawSettings".into(),
                    json!({ "header": { "type": "http", "request": request } }),
                );
            }
        }
        "websocket" => {
            stream.insert("wsSettings".into(), path_host(path, host));
        }
        "httpupgrade" => {
            stream.insert("httpupgradeSettings".into(), path_host(path, host));
        }
        "grpc" => {
            let mut grpc = Map::new();
            put(&mut grpc, "serviceName", params.get("serviceName"));
            put(&mut grpc, "authority", params.get("authority"));
            if params.get("mode") == Some("multi") {
                grpc.insert("multiMode".into(), true.into());
            }
            stream.insert("grpcSettings".into(), grpc.into());
        }
        "xhttp" => {
            let mut xhttp = Map::new();
            put(&mut xhttp, "path", path);
            put(&mut xhttp, "host", host);
            put(&mut xhttp, "mode", params.get("mode"));
            if let Some(extra) = params.get("extra") {
                match serde_json::from_str::<Value>(extra) {
                    Ok(extra @ Value::Object(_)) => {
                        xhttp.insert("extra".into(), extra);
                    }
                    _ => warnings.push("xhttp `extra` is not a JSON object, ignored".into()),
                }
            }
            stream.insert("xhttpSettings".into(), xhttp.into());
        }
        _ => {
            if params.get("seed").is_some() || params.get("headerType").is_some_and(|t| t != "none") {
                warnings.push("mKCP seed/header obfuscation is not supported, ignored".into());
            }
        }
    }

    let security = match params.get("security").unwrap_or(default_security) {
        "none" => "none",
        "tls" | "xtls" => "tls",
        "reality" => "reality",
        other => return Err(Error::Unsupported(format!("security {other}"))),
    };
    stream.insert("security".into(), security.into());

    // Clients conventionally fall back to the Host header when no SNI is given.
    let sni = params
        .get("sni")
        .or_else(|| params.get("peer"))
        .or_else(|| host.filter(|h| !h.contains(',')));
    let fingerprint = params.get("fp");
    match security {
        "tls" => {
            let mut tls = Map::new();
            put(&mut tls, "serverName", sni);
            put(&mut tls, "fingerprint", fingerprint);
            if let Some(alpn) = params.get("alpn") {
                tls.insert("alpn".into(), list(alpn));
            }
            put(&mut tls, "pinnedPeerCertSha256", params.get("pcs"));
            put(&mut tls, "verifyPeerCertByName", params.get("vcn"));
            put(&mut tls, "echConfigList", params.get("ech"));
            if params.flag("allowInsecure") || params.flag("insecure") {
                warnings.push(INSECURE_WARNING.into());
            }
            stream.insert("tlsSettings".into(), tls.into());
        }
        "reality" => {
            let password = params
                .get("pbk")
                .ok_or_else(|| Error::InvalidLink("REALITY link without `pbk`".into()))?;
            let mut reality = Map::new();
            put(&mut reality, "serverName", sni);
            put(&mut reality, "fingerprint", fingerprint.or(Some("chrome")));
            put(&mut reality, "password", Some(password));
            put(&mut reality, "shortId", params.get("sid"));
            put(&mut reality, "spiderX", params.get("spx"));
            put(&mut reality, "mldsa65Verify", params.get("pqv"));
            stream.insert("realitySettings".into(), reality.into());
        }
        _ => {}
    }

    Ok(Stream {
        settings: stream.into(),
        transport,
        security,
    })
}

/// Xray removed `allowInsecure`; a config that still sets it does not start.
pub(crate) const INSECURE_WARNING: &str =
    "the link asks to skip TLS certificate verification (allowInsecure), which Xray no longer supports; the certificate will be verified";

fn path_host(path: Option<&str>, host: Option<&str>) -> Value {
    let mut settings = Map::new();
    put(&mut settings, "path", path);
    put(&mut settings, "host", host);
    settings.into()
}

fn put(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.into(), value.into());
    }
}

fn list(value: &str) -> Value {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .into()
}
