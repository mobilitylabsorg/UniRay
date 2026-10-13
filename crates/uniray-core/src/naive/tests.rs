use super::*;
use crate::links::{parse_link, parse_many};

#[test]
fn naive_https_link_is_parsed() {
    let link = "naive+https://user:p%40ss%3Aword@proxy.example.com:8443?extra-headers=X-Id%3A%2042%0D%0AX-B%3A%20c&insecure-concurrency=2#My%20Naive";
    let server = parse_link(link).unwrap();
    assert_eq!((server.name.as_str(), server.protocol.as_str()), ("My Naive", "naive"));
    assert_eq!((server.address.as_str(), server.port), ("proxy.example.com", 8443));
    assert_eq!((server.transport.as_str(), server.security.as_str()), ("h2", "tls"));
    assert!(!server.source.is_xray());
    let ServerSource::Naive(config) = &server.source else { panic!() };
    assert_eq!(config.scheme, "https");
    // The password keeps its ':' - only the first one separates the user.
    assert_eq!((config.user.as_str(), config.pass.as_str()), ("user", "p@ss:word"));
    assert_eq!(config.extra_headers, "X-Id: 42\r\nX-B: c");
    assert_eq!(config.insecure_concurrency, Some(2));

    // The link it exports reads back the same.
    let again = parse_link(&config.to_link(&server.name)).unwrap();
    assert_eq!(again.source, server.source);
    assert_eq!(again.name, "My Naive");
}

#[test]
fn naive_quic_defaults() {
    let server = parse_link("naive+quic://a:b@[2001:db8::1]").unwrap();
    let ServerSource::Naive(config) = &server.source else { panic!() };
    assert_eq!((config.scheme.as_str(), config.port), ("quic", 443));
    assert_eq!(server.transport, "quic");
    assert_eq!(server.name, "2001:db8::1:443");
    assert_eq!(
        config.naive_config("2001:db8::1".parse().unwrap())["proxy"],
        "quic://a:b@[2001:db8::1]:443"
    );
}

#[test]
fn broken_naive_links_are_refused() {
    for link in [
        "naive+https://host.example:443",
        "naive+https://user@host.example:443",
        "naive+https://a:b@host.example:443?insecure-concurrency=0",
        "naive+https://a:b@host.example:443?extra-headers=no-colon",
        "naive+http://a:b@host.example:443",
    ] {
        assert!(parse_link(link).is_err(), "{link}");
    }
    let server = parse_link("naive+https://a:b@h.example?insecure-concurrency=99").unwrap();
    assert_eq!(server.warnings.len(), 1);
}

#[test]
fn naive_links_import_among_others() {
    let imported = parse_many("naive+https://a:b@h.example#One\ntrojan://pw@203.0.113.1:443#T\n");
    assert_eq!(imported.servers.len(), 2);
    assert!(imported.errors.is_empty());
}
