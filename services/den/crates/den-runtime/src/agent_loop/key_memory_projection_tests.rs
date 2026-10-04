use den_core::config::Config;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::agent_loop::key_memory_projection::{
    project_key_memory, project_key_memory_with_scope, KeyMemoryProjectionInput,
    MemoryProjectionScope,
};
use den_core::ids::HatId;
use den_core::tools::work_surface::WorkSurfaceSessionHints;
use den_memory::{
    append_memory_record, scoped::MemoryReadGrant, AccessContext, LogicalMemoryPath, MemorySource,
    MemoryStoreManager,
};
use den_service::bears::{model::RuntimeContextLabel, Bear};

fn legacy_test_bear(bear_id: Uuid) -> Bear {
    let now = OffsetDateTime::now_utc();
    Bear {
        id: bear_id,
        slug: "test-bear".to_string(),
        name: "Test Bear".to_string(),
        description: String::new(),
        default_model: None,
        default_tool_budget_multiplier: None,
        work_enabled: true,
        cabinet_enabled: true,
        tools_enabled: None,
        runtime_plan: None,
        context_profile: None,
        provisioning_version: 1,
        system_prompt: "You are a test bear.".to_string(),
        birthday: None,
        created_at: now,
        updated_at: now,
        live_reflection_enabled: true,
        live_reflection_stale_after_minutes: 30,
        live_reflection_activity_threshold: 20,
        live_reflection_sweep_limit: 25,
    }
}

fn noop_pg_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").expect("lazy pool")
}

#[tokio::test]
async fn projects_shared_identity_anchors_without_work_surface() {
    let bear_id = Uuid::new_v4();
    let bear = legacy_test_bear(bear_id);
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-kmp-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.id).await.expect("store");
    append_memory_record(
        &store,
        &LogicalMemoryPath::from_logical_path("core/bear-overview.md"),
        "overview",
        "curate",
        None,
        "Charter summary",
        &serde_json::json!({}),
    )
    .await
    .expect("append");

    let pool = noop_pg_pool();
    let result = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: WorkSurfaceSessionHints::default(),
        work_surface_status_override: None,
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");

    assert!(result.rendered_text.contains("# Projected memory"));
    assert!(result.rendered_text.contains("Charter summary"));
    assert!(result.rendered_text.contains("## Shared anchors"));
    assert!(!result.rendered_text.contains("## Work surface:"));
}

#[tokio::test]
async fn bound_turn_projection_excludes_other_sessions_and_legacy_profile_notes() {
    let bear_id = Uuid::new_v4();
    let bear = legacy_test_bear(bear_id);
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-bound-kmp-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.expect("store");
    let a = MemorySource::Conversation(Uuid::new_v4());
    let b = MemorySource::Conversation(Uuid::new_v4());
    let hat = HatId::new(Uuid::new_v4());
    for (path, content) in [
        (
            LogicalMemoryPath::source_local(a, "note"),
            "session A secret",
        ),
        (
            LogicalMemoryPath::source_local(b, "note"),
            "session B secret",
        ),
        (LogicalMemoryPath::hat(hat, "note"), "reviewed hat fact"),
        (
            LogicalMemoryPath::profile_local("pair", "note"),
            "legacy profile secret",
        ),
        (
            LogicalMemoryPath::from_logical_path("core/bear-glossary.md"),
            "shared core fact",
        ),
    ] {
        append_memory_record(
            &store,
            &path,
            "note",
            "pair",
            None,
            content,
            &serde_json::json!({}),
        )
        .await
        .expect("append memory");
    }
    // A forged legacy row under a core locator must not become a shared anchor.
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type,
            scope_profile, kind, author_profile, created_at, content_text, logical_path)
         VALUES (?, ?, 900, 'profile_local', 'pair', 'note', 'pair',
                 '2026-01-01T00:00:00Z', 'forged core-path secret', 'core/bear-overview.md')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(bear_id.to_string())
    .execute(store.pool())
    .await
    .expect("insert colliding scope");
    let pool = noop_pg_pool();
    for (scope, visible, hidden) in [
        (
            MemoryProjectionScope::Bound(MemoryReadGrant::new(a, Some(hat))),
            "session A secret",
            "session B secret",
        ),
        (
            MemoryProjectionScope::Bound(MemoryReadGrant::new(b, Some(hat))),
            "session B secret",
            "session A secret",
        ),
    ] {
        let result = project_key_memory_with_scope(
            KeyMemoryProjectionInput {
                pool: &pool,
                stores: &stores,
                bear: &bear,
                profile: RuntimeContextLabel::ArmatureConversation,
                conversation_id: "bound-conversation",
                session_hints: WorkSurfaceSessionHints::default(),
                work_surface_status_override: None,
                native_runtime: true,
                model_for_budget: None,
                access: AccessContext::empty(),
            },
            scope,
        )
        .await
        .expect("project scoped memory");
        assert!(result.rendered_text.contains(visible));
        assert!(result.rendered_text.contains("reviewed hat fact"));
        assert!(result.rendered_text.contains("shared core fact"));
        assert!(!result.rendered_text.contains(hidden));
        assert!(!result.rendered_text.contains("legacy profile secret"));
        assert!(!result.rendered_text.contains("forged core-path secret"));
    }
    let shared_only = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "unresolved-conversation",
        session_hints: WorkSurfaceSessionHints::default(),
        work_surface_status_override: None,
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project shared only");
    assert!(shared_only.rendered_text.contains("shared core fact"));
    assert!(!shared_only.rendered_text.contains("session A secret"));
    assert!(!shared_only.rendered_text.contains("reviewed hat fact"));
    assert!(!shared_only.rendered_text.contains("legacy profile secret"));
    assert!(!shared_only
        .rendered_text
        .contains("forged core-path secret"));
}

#[tokio::test]
async fn long_context_model_metadata_increases_projection_budget() {
    let bear_id = Uuid::new_v4();
    let mut bear = legacy_test_bear(bear_id);
    bear.default_model = Some("openai/gpt-4.1".to_string());
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-kmp-model-budget-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let pool = noop_pg_pool();

    let result = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: WorkSurfaceSessionHints::default(),
        work_surface_status_override: None,
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");

    assert_eq!(result.diagnostic["global_char_cap"].as_u64(), Some(16_000));
    assert_eq!(
        result
            .diagnostic
            .pointer("/model_metadata/key")
            .and_then(|v| v.as_str()),
        Some("openai/gpt-4.1")
    );
    assert_eq!(
        result
            .diagnostic
            .pointer("/model_metadata/context_window")
            .and_then(|v| v.as_u64()),
        Some(1_047_576)
    );
}

#[tokio::test]
async fn candidate_work_surface_requires_canonical_anchor_for_tier2() {
    let bear_id = Uuid::new_v4();
    let bear = legacy_test_bear(bear_id);
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-kmp-anchor-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let hints = WorkSurfaceSessionHints {
        workspace_roots: vec!["/workspace/bears-monorepo".to_string()],
        ..Default::default()
    };
    let pool = noop_pg_pool();

    let without_anchor = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: hints.clone(),
        work_surface_status_override: Some("candidate"),
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");
    assert!(!without_anchor.rendered_text.contains("## Work surface:"));
    assert_eq!(
        without_anchor.diagnostic["omitted_because_no_surface"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "tier2:slug=bears-monorepo:anchor_required"
    );

    let store = stores.store_for_bear(bear.id).await.expect("store");
    append_memory_record(
        &store,
        &LogicalMemoryPath::from_logical_path("core/work_surfaces/bears-monorepo/overview.md"),
        "overview",
        "curate",
        None,
        "Canonical overview",
        &serde_json::json!({}),
    )
    .await
    .expect("append overview");

    let with_anchor = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: hints,
        work_surface_status_override: Some("candidate"),
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");
    assert!(with_anchor
        .rendered_text
        .contains("## Work surface: bears-monorepo"));
    assert!(with_anchor.rendered_text.contains("Canonical overview"));
}

#[tokio::test]
async fn resolved_work_surface_includes_tier2_without_prior_anchor_proof() {
    let bear_id = Uuid::new_v4();
    let bear = legacy_test_bear(bear_id);
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-kmp-resolved-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.id).await.expect("store");
    append_memory_record(
        &store,
        &LogicalMemoryPath::from_logical_path("core/work_surfaces/my-app/decisions.md"),
        "decisions",
        "curate",
        None,
        "Use SQLite",
        &serde_json::json!({}),
    )
    .await
    .expect("append");

    let pool = noop_pg_pool();
    let result = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: WorkSurfaceSessionHints {
            workspace_roots: vec!["/workspace/my-app".to_string()],
            ..Default::default()
        },
        work_surface_status_override: Some("resolved"),
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");

    assert!(result.rendered_text.contains("## Work surface: my-app"));
    assert!(result.rendered_text.contains("Use SQLite"));
}

#[tokio::test]
async fn access_bearing_relation_gates_record_out_of_projection() {
    use den_memory::{append_relation, resolve, Assertion, Resolution, Signal};

    let bear_id = Uuid::new_v4();
    let bear = legacy_test_bear(bear_id);
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-kmp-access-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.id).await.expect("store");

    let record = append_memory_record(
        &store,
        &LogicalMemoryPath::from_logical_path("core/bear-overview.md"),
        "overview",
        "curate",
        None,
        "Confidential charter",
        &serde_json::json!({}),
    )
    .await
    .expect("append");

    // Confine the record to a resolved work-surface entity.
    let surface = match resolve(
        &store,
        "work_surface",
        Some("client-a"),
        &[Signal::new("git_remote", "github.com/acme/client-a")],
        Assertion::Asserted,
    )
    .await
    .expect("resolve")
    {
        Resolution::Resolved(e) => e.entity_id,
        other => panic!("expected Resolved, got {other:?}"),
    };
    append_relation(
        &store,
        &record.memory_id,
        &surface,
        "confined_to",
        &serde_json::json!({}),
        "curate",
        None,
        None,
    )
    .await
    .expect("append access rule");

    let pool = noop_pg_pool();

    // Fail-closed context hides the confined record.
    let hidden = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: WorkSurfaceSessionHints::default(),
        work_surface_status_override: None,
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty(),
    })
    .await
    .expect("project");
    assert!(!hidden.rendered_text.contains("Confidential charter"));
    assert_eq!(
        hidden.diagnostic["omitted_by_access"]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0),
        1
    );

    // Granting the confinement scope surfaces it.
    let shown = project_key_memory(KeyMemoryProjectionInput {
        pool: &pool,
        stores: &stores,
        bear: &bear,
        profile: RuntimeContextLabel::ArmatureConversation,
        conversation_id: "den-conv-test",
        session_hints: WorkSurfaceSessionHints::default(),
        work_surface_status_override: None,
        native_runtime: true,
        model_for_budget: None,
        access: AccessContext::empty().with_confinement([surface]),
    })
    .await
    .expect("project");
    assert!(shown.rendered_text.contains("Confidential charter"));
}
