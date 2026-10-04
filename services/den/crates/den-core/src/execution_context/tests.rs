use super::RuntimeContextLabel;

#[test]
fn historical_schema_spellings_round_trip_without_migration() {
    let labels = [
        (RuntimeContextLabel::ChannelConversation, "chat"),
        (RuntimeContextLabel::ArmatureConversation, "pair"),
        (RuntimeContextLabel::Curation, "curate"),
        (RuntimeContextLabel::JobRun, "work"),
        (RuntimeContextLabel::Observation, "watch"),
    ];
    assert_eq!(RuntimeContextLabel::ALL, labels.map(|(label, _)| label));
    for (label, persisted) in labels {
        assert_eq!(label.as_str(), persisted);
        assert_eq!(label.to_string(), persisted);
        assert_eq!(persisted.parse::<RuntimeContextLabel>().unwrap(), label);
        assert_eq!(
            serde_json::to_value(label).unwrap(),
            serde_json::json!(persisted)
        );
        assert_eq!(
            serde_json::from_value::<RuntimeContextLabel>(serde_json::json!(persisted)).unwrap(),
            label
        );
    }
}

#[test]
fn source_variant_names_are_not_new_persisted_spellings() {
    for invalid in [
        "ChannelConversation",
        "channel_conversation",
        "ArmatureConversation",
        "armature_conversation",
        "JobRun",
        "job_run",
        "Curation",
        "curation_operation",
        "Observation",
        "observation_operation",
        "talk",
        "unknown",
    ] {
        assert!(invalid.parse::<RuntimeContextLabel>().is_err());
        assert!(serde_json::from_value::<RuntimeContextLabel>(serde_json::json!(invalid)).is_err());
    }
}
