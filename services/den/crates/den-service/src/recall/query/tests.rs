use super::*;

fn passage(memory_id: &str, path: &str, score: f32) -> RecalledPassage {
    RecalledPassage {
        memory_id: memory_id.into(),
        logical_path: Some(path.into()),
        kind: Some("note".into()),
        score,
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        text: "the quick brown fox jumps over the lazy dog".into(),
        conflicts_with: Vec::new(),
    }
}

#[test]
fn disabled_recall_reason_preserves_diagnostic_strings() {
    let cases = [
        (DisabledRecallReason::QdrantUnset, "qdrant_unset"),
        (DisabledRecallReason::EmbeddingsUnset, "embeddings_unset"),
        (DisabledRecallReason::NoEntities, "no_entities"),
    ];

    for (reason, expected) in cases {
        let projection = disabled_projection(reason);
        assert_eq!(projection.diagnostic["reason"], expected);
    }
}

#[test]
fn render_drops_paths_already_in_anchors() {
    let projection = RecallProjection {
        passages: vec![
            passage("m1", "core/a.md", 0.91),
            passage("m2", "core/b.md", 0.80),
        ],
        diagnostic: Value::Null,
    };
    let anchors = "# Projected memory\n- core/a.md: ...";
    let block = render_recall_block(&projection, anchors).expect("block");
    assert!(block.contains("core/b.md"));
    assert!(!block.contains("core/a.md"));
}

#[test]
fn render_none_when_all_deduped() {
    let projection = RecallProjection {
        passages: vec![passage("m1", "core/a.md", 0.91)],
        diagnostic: Value::Null,
    };
    assert!(render_recall_block(&projection, "core/a.md").is_none());
}

#[test]
fn truncate_collapses_whitespace_and_caps_length() {
    let out = truncate_chars("a\n\n  b   c", 100);
    assert_eq!(out, "a b c");
    let long = "x".repeat(600);
    let capped = truncate_chars(&long, SNIPPET_CHARS);
    assert_eq!(capped.chars().count(), SNIPPET_CHARS + 1); // + ellipsis
}

#[test]
fn entity_scope_filter_requires_entity_membership() {
    let bear = Uuid::nil();
    let filter = entity_scope_filter(bear, "bears-embed-v1", &["e1".into(), "e2".into()]);
    let must = filter["must"].as_array().expect("must array");
    // Three mandatory scope conditions + one entity-membership clause.
    assert_eq!(must.len(), 4, "{filter}");
    assert_eq!(must[0]["key"], "bear_id");
    assert_eq!(must[3]["key"], "entity_ids");
    let any = must[3]["match"]["any"].as_array().expect("any array");
    assert_eq!(any.len(), 2);
    assert_eq!(any[0], "e1");
    assert_eq!(any[1], "e2");
}

#[test]
fn freshness_multiplier_downranks_stale_and_boosts_strengthening() {
    assert!(freshness_multiplier("strengthening") > freshness_multiplier("stable"));
    assert!(freshness_multiplier("weakening") < freshness_multiplier("stable"));
    assert!(freshness_multiplier("stale") < freshness_multiplier("weakening"));
}

fn conflict(a: &str, b: &str, path: &str) -> den_memory::MemoryConflict {
    den_memory::MemoryConflict {
        memory_id_a: a.min(b).to_string(),
        memory_id_b: a.max(b).to_string(),
        reason: den_memory::ConflictReason::SharedLogicalPath(path.to_string()),
    }
}

#[test]
fn mark_projection_conflicts_fills_passages_and_diagnostic() {
    let mut projection = RecallProjection {
        passages: vec![
            passage("m1", "core/a.md", 0.91),
            passage("m2", "core/b.md", 0.80),
        ],
        diagnostic: json!({ "status": "ok" }),
    };
    mark_projection_conflicts(&mut projection, &[conflict("m1", "m2", "core/a.md")]);

    assert_eq!(
        projection.passages[0].conflicts_with,
        vec!["m2".to_string()]
    );
    assert_eq!(
        projection.passages[1].conflicts_with,
        vec!["m1".to_string()]
    );
    assert_eq!(projection.diagnostic["conflicts"]["pairs"], 1);
    assert_eq!(
        projection.diagnostic["conflicts"]["records"],
        json!(["m1", "m2"])
    );
}

#[test]
fn render_marks_conflicting_passages_naming_the_counterpart() {
    let mut projection = RecallProjection {
        passages: vec![
            passage("m1", "core/a.md", 0.91),
            passage("m2", "core/b.md", 0.80),
            passage("m3", "core/c.md", 0.70),
        ],
        diagnostic: json!({ "status": "ok" }),
    };
    mark_projection_conflicts(&mut projection, &[conflict("m1", "m2", "core/a.md")]);
    let block = render_recall_block(&projection, "").expect("block");

    assert!(
        block.contains("- `core/a.md` (note, score 0.91, conflicting with `core/b.md`):"),
        "{block}"
    );
    assert!(
        block.contains("- `core/b.md` (note, score 0.80, conflicting with `core/a.md`):"),
        "{block}"
    );
    assert!(
        block.contains("- `core/c.md` (note, score 0.70):"),
        "unconflicted passage keeps the plain marker: {block}"
    );
}
