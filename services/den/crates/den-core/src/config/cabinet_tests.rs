use super::*;

#[test]
fn cabinet_origins_are_strict_and_optional() {
    for raw in ["https://cabinet.example", "http://localhost:3008/"] {
        assert!(parse_optional_origin(Some(raw)).unwrap().is_some());
    }
    for raw in [
        "/cabinet",
        "cabinet.example",
        "ftp://cabinet.example",
        "https://user:secret@cabinet.example",
        "https://@cabinet.example",
        "https://cabinet.example/cabinet",
        "https://cabinet.example/..",
        "https://cabinet.example/%2e",
        "https://cabinet.example?x=1",
        "https://cabinet.example#x",
        "https://cabinet.example\\path",
    ] {
        assert!(parse_optional_origin(Some(raw)).is_err(), "{raw}");
    }
    assert_eq!(parse_optional_origin(None).unwrap(), None);
    assert_eq!(parse_optional_origin(Some("  ")).unwrap(), None);
}

#[test]
fn cabinet_destinations_preserve_local_rollout_and_queries() {
    let mut config = Config::test_stub();
    assert_eq!(config.cabinet_home_url(WebSurface::Den), "/cabinet");
    assert_eq!(
        config
            .cabinet_destination()
            .resolve_path("/cabinet/page?version=v1")
            .unwrap(),
        "/cabinet/page?version=v1"
    );
    config.cabinet_web_origin = parse_optional_origin(Some("https://cabinet.example")).unwrap();
    assert_eq!(
        config.cabinet_home_url(WebSurface::Den),
        "https://cabinet.example/cabinet"
    );
    assert_eq!(config.cabinet_home_url(WebSurface::Cabinet), "/cabinet");
    assert_eq!(
        config.surface_public_origin(WebSurface::Cabinet),
        "https://cabinet.example"
    );
    assert_eq!(
        config
            .cabinet_destination()
            .resolve_path("/cabinet/page?version=v1")
            .unwrap(),
        "https://cabinet.example/cabinet/page?version=v1"
    );
    for path in [
        "//evil.example",
        "https://evil.example/cabinet",
        "/cabinet/../admin",
        "/cabinet/%2e%2e/admin",
        "/cabinetish",
        "/cabinet\\evil",
        "/cabinet#fragment",
    ] {
        assert!(
            config.cabinet_destination().resolve_path(path).is_err(),
            "{path}"
        );
    }
}

#[test]
fn cabinet_url_normalizes_an_optional_trailing_slash_for_links_and_origin_checks() {
    for base in [
        "https://cabinet.example",
        "https://cabinet.example:8443",
        "http://localhost:3008",
        "http://[::1]:3008",
    ] {
        let without_slash = parse_optional_origin(Some(base)).unwrap();
        let with_slash = parse_optional_origin(Some(&format!("{base}/"))).unwrap();
        assert_eq!(without_slash, with_slash);
        for origin in [without_slash, with_slash] {
            let mut config = Config::test_stub();
            config.cabinet_web_origin = origin;
            assert_eq!(config.surface_public_origin(WebSurface::Cabinet), base);
            assert_eq!(
                config.cabinet_home_url(WebSurface::Den),
                format!("{base}/cabinet")
            );
            assert_eq!(
                config
                    .cabinet_destination()
                    .resolve_path("/cabinet/page/history?before=v1")
                    .unwrap(),
                format!("{base}/cabinet/page/history?before=v1")
            );
        }
    }
    assert!(parse_optional_origin(Some("https://cabinet.example/path"))
        .unwrap_err()
        .to_string()
        .starts_with("CABINET_URL"));
}

#[test]
fn exactly_one_surface_owns_cabinet_recovery() {
    let mut config = Config::test_stub();
    assert!(config.owns_cabinet_recovery(WebSurface::Den));
    assert!(!config.owns_cabinet_recovery(WebSurface::Cabinet));
    config.cabinet_web_origin = parse_optional_origin(Some("https://cabinet.example")).unwrap();
    assert!(!config.owns_cabinet_recovery(WebSurface::Den));
    assert!(config.owns_cabinet_recovery(WebSurface::Cabinet));
}
