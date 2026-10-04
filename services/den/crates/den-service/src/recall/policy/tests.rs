use super::*;

#[test]
fn shared_normal_records_are_indexable() {
    assert!(is_indexable("shared", "overview", "normal"));
    assert!(is_indexable("shared", "note", "normal"));
}

#[test]
fn raw_legacy_and_source_scopes_are_never_indexable() {
    for scope in ["profile_local", "role_local", "source_local"] {
        for kind in ["note", "decision", "summary", "overview"] {
            assert!(!is_indexable(scope, kind, "normal"));
        }
    }
}

#[test]
fn non_normal_visibility_and_ephemeral_kinds_excluded() {
    assert!(!is_indexable("shared", "overview", "hidden"));
    assert!(!is_indexable("shared", "scratch", "normal"));
    assert!(!is_indexable("shared", "log", "normal"));
    assert!(!is_indexable("unknown_scope", "note", "normal"));
    assert!(is_indexable("hat", "note", "normal"));
    assert!(!is_indexable("source_local", "note", "normal"));
}

#[test]
fn hat_index_requires_canonical_hat_id_and_never_indexes_raw_sources() {
    let hat = HatId::new(Uuid::new_v4());
    let req = IndexRequest {
        bear_id: Uuid::new_v4(),
        memory_id: Uuid::new_v4().to_string(),
        sequence_no: 1,
        logical_path: Some(format!("hat_memory/{hat}/note.md")),
        scope_type: "hat".into(),
        scope_profile: None,
        scope_hat_id: Some(hat),
        work_surface_ref: None,
        kind: "note".into(),
        visibility: "normal".into(),
        content_text: "reviewed knowledge".into(),
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        entity_ids: vec![],
    };
    assert!(req.is_indexable());
    let payload = build_payload(
        &req,
        &Chunk {
            index: 0,
            text: "reviewed knowledge".into(),
            content_hash: "x".into(),
        },
        "test-standard",
    );
    assert_eq!(payload["scope_hat_id"], hat.to_string());
    assert!(!IndexRequest {
        scope_hat_id: None,
        ..req.clone()
    }
    .is_indexable());
    assert!(!IndexRequest {
        scope_type: "source_local".into(),
        ..req
    }
    .is_indexable());
}

#[test]
fn point_id_is_deterministic_and_varies_by_chunk() {
    let bear = Uuid::nil();
    let a = point_id(bear, "mem-1", 0, "bears-embed-v1");
    let a2 = point_id(bear, "mem-1", 0, "bears-embed-v1");
    let b = point_id(bear, "mem-1", 1, "bears-embed-v1");
    let c = point_id(bear, "mem-2", 0, "bears-embed-v1");
    assert_eq!(a, a2);
    assert_ne!(a, b);
    assert_ne!(a, c);
    // Valid UUID format.
    assert!(Uuid::parse_str(&a).is_ok());
}

#[test]
fn archived_lifecycle_records_are_not_indexable() {
    let req = IndexRequest {
        bear_id: Uuid::nil(),
        memory_id: "mem-archived".into(),
        sequence_no: 1,
        logical_path: Some("core/old.md".into()),
        scope_type: "shared".into(),
        scope_profile: None,
        scope_hat_id: None,
        work_surface_ref: None,
        kind: "note".into(),
        visibility: "normal".into(),
        content_text: "old body".into(),
        salience: "normal".into(),
        lifecycle_status: "archived".into(),
        freshness_trend: "stale".into(),
        entity_ids: Vec::new(),
    };
    assert!(!req.is_indexable());
}

#[test]
fn payload_carries_required_fields() {
    let req = IndexRequest {
        bear_id: Uuid::nil(),
        memory_id: "mem-1".into(),
        sequence_no: 1,
        logical_path: Some("core/work_surfaces/x/overview.md".into()),
        scope_type: "shared".into(),
        scope_profile: None,
        scope_hat_id: None,
        work_surface_ref: Some("x".into()),
        kind: "overview".into(),
        visibility: "normal".into(),
        content_text: "body".into(),
        salience: "high".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        entity_ids: vec!["ent-1".into(), "ent-2".into()],
    };
    let chunk = Chunk {
        index: 0,
        text: "body".into(),
        content_hash: "abc".into(),
    };
    let payload = build_payload(&req, &chunk, "bears-embed-v1");
    assert_eq!(payload["source_class"], SOURCE_CLASS_BEAR_MEMORY);
    assert_eq!(payload["embedding_standard"], "bears-embed-v1");
    assert_eq!(payload["memory_id"], "mem-1");
    assert_eq!(payload["chunk_index"], 0);
    assert_eq!(payload["content_hash"], "abc");
    assert_eq!(payload["work_surface_ref"], "x");
    assert!(payload["scope_hat_id"].is_null());
    assert_eq!(payload["kind"], "overview");
    assert_eq!(payload["salience"], "high");
    assert_eq!(payload["lifecycle_status"], "active");
    assert_eq!(payload["freshness_trend"], "stable");
    assert_eq!(payload["text"], "body");
    assert_eq!(payload["entity_ids"], serde_json::json!(["ent-1", "ent-2"]));
}
