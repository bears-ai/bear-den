use super::*;
use std::net::{Ipv4Addr, SocketAddr};

#[test]
fn public_http_target_retains_only_vetted_connection_addresses() {
    let target = resolve_public_http_target("https://1.1.1.1/docs").unwrap();
    assert_eq!(target.url.as_str(), "https://1.1.1.1/docs");
    assert_eq!(
        target.resolved_addrs,
        vec![SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 443))],
    );
    for denied in [
        "http://127.0.0.1/",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://localhost/",
    ] {
        assert!(resolve_public_http_target(denied).is_err(), "{denied}");
    }
    assert!(!is_public_ip(IpAddr::V6(
        "::ffff:10.0.0.1".parse().unwrap()
    )));
    assert!(is_public_ip(IpAddr::V6("::ffff:1.1.1.1".parse().unwrap())));
}

#[test]
fn egress_rejects_nonpublic_special_address_ranges() {
    for denied in [
        "100.64.0.1",
        "100.127.255.254",
        "192.0.0.9",
        "198.18.0.1",
        "198.19.255.254",
        "240.0.0.1",
        "::ffff:100.64.0.1",
        "2001:db8::1",
    ] {
        assert!(!is_public_ip(denied.parse().unwrap()), "{denied}");
    }
    for allowed in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
        assert!(is_public_ip(allowed.parse().unwrap()), "{allowed}");
    }
}
