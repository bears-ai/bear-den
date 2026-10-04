use super::*;

#[test]
fn profile_parse_and_display_round_trip() {
    for profile in RuntimeContextLabel::ALL {
        let parsed: RuntimeContextLabel = profile.as_str().parse().expect("profile parses");
        assert_eq!(parsed, profile);
        assert_eq!(profile.to_string(), profile.as_str());
    }
    assert!("unknown".parse::<RuntimeContextLabel>().is_err());
    assert!("talk".parse::<RuntimeContextLabel>().is_err());
}
