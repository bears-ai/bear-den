use super::*;

#[test]
fn preferred_names_trim_edges_and_validate_utf8_byte_boundaries() {
    for (input, expected) in [
        (" A ", "A"),
        ("\u{2003}Zoë\u{00a0}", "Zoë"),
        ("李 小龍", "李 小龍"),
        ("Mary  Jane", "Mary  Jane"),
    ] {
        assert_eq!(PreferredName::new(input).unwrap().as_str(), expected);
    }
    for valid in [
        "a".repeat(128),
        "é".repeat(64),
        format!("{}a", "李".repeat(42)),
    ] {
        assert!(PreferredName::new(&valid).is_ok());
    }
    for invalid in [
        "a".repeat(129),
        format!("{}a", "é".repeat(64)),
        "李".repeat(43),
    ] {
        assert!(matches!(
            PreferredName::new(&invalid),
            Err(DenError::ValidationError(_))
        ));
    }
    assert_eq!(
        PreferredName::new(&format!("  {}  ", "a".repeat(128)))
            .unwrap()
            .as_str()
            .len(),
        128
    );
}

#[test]
fn empty_and_control_containing_names_are_rejected_before_trimming() {
    for invalid in [
        "",
        "   ",
        "\u{2003}\u{00a0}",
        "\nAlice",
        "Alice\t",
        "A\0B",
        "A\rB",
        "A\u{7f}B",
        "A\u{85}B",
    ] {
        let error = PreferredName::new(invalid).unwrap_err();
        assert!(matches!(error, DenError::ValidationError(_)));
        assert!(!error.to_string().contains("Alice"));
    }
}

#[test]
fn protected_values_have_redacted_debug_and_no_serde_implementations() {
    let name = PreferredName::new("Private sentinel name").unwrap();
    assert_eq!(format!("{name:?}"), "PreferredName([redacted])");
    assert_eq!(format!("{name:#?}"), "PreferredName([redacted])");
    assert_eq!(name.clone(), name);

    // Inference is unambiguous only while the protected types do NOT implement these
    // traits. A future derive makes this test fail to compile without new dependencies.
    trait AmbiguousSerialize<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousSerialize<()> for T {}
    enum ImplementsSerialize {}
    impl<T: ?Sized + serde::Serialize> AmbiguousSerialize<ImplementsSerialize> for T {}
    let _ = <PreferredName as AmbiguousSerialize<_>>::check;
    let _ = <PersonalCandidate as AmbiguousSerialize<_>>::check;

    trait AmbiguousDeserialize<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousDeserialize<()> for T {}
    enum ImplementsDeserialize {}
    impl<T: serde::Deserialize<'static>> AmbiguousDeserialize<ImplementsDeserialize> for T {}
    let _ = <PreferredName as AmbiguousDeserialize<_>>::check;
    let _ = <PersonalCandidate as AmbiguousDeserialize<_>>::check;
}
