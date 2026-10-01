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
