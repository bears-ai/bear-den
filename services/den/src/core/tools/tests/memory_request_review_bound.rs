use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::tools::{
    arguments::DenToolChannelContext,
    constants::DEN_MEMORY_REQUEST_REVIEW,
    session::{invoke_den_tool_for_origin, DenToolInvocationContext},
};
use den_core::ids::{BearId, UserId};
use den_core::{ArmatureAvailability, Governance, TurnExecutionOrigin};
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};
use den_service::{
    bears::{db, hats, RuntimeContextLabel},
    conversation::persistence,
};

#[sqlx::test]
async fn bound_review_records_verified_source_and_hat_without_publishing(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "boundreviewbear",
            name: "Bound review Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await?;
    let admin = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('boundreviewadmin@example.test', 'boundreviewadmin') RETURNING id"
    ).fetch_one(&pool).await?;
    let member = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('boundreviewmember@example.test', 'boundreviewmember') RETURNING id"
    ).fetch_one(&pool).await?;
    db::grant_membership(&pool, admin, bear_id, Some(db::BEAR_ROLE_ADMIN)).await?;
    db::grant_membership(&pool, member, bear_id, Some(db::BEAR_ROLE_MEMBER)).await?;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Security",
        "Review code",
    )
    .await?;
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Support",
        "Help customers",
    )
    .await?;
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, true, true).await?;
    let agent_id = format!("den-native:pair-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO bear_profile_bindings (bear_id, profile, binding_id) VALUES ($1, 'pair', $2)",
    )
    .bind(bear_id)
    .bind(&agent_id)
    .execute(&pool)
    .await?;
    let a = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin),
        "conv-bound-review-a",
        None,
        None,
    )
    .await?;
    let b = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(member),
        "conv-bound-review-b",
        None,
        None,
    )
    .await?;
    let c = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin),
        "conv-bound-review-other-hat",
        None,
        None,
    )
    .await?;
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), a.id, hat.id).await?;
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), b.id, hat.id).await?;
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), c.id, other_hat.id).await?;

    let mut config = crate::config::Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("bound-review-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await?;
    let private = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(a.id), "note"),
        "note",
        "pair",
        None,
        "Private source note, never directly shared",
        &json!({}),
    )
    .await?;
    let private_id = Uuid::parse_str(&private.memory_id)?;
    let other = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(c.id), "note"),
        "note",
        "pair",
        None,
        "Another hat's private source",
        &json!({}),
    )
    .await?;
    let shared = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("note"),
        "note",
        "curate",
        None,
        "Existing shared fact",
        &json!({}),
    )
    .await?;

    let context = |user_id, conversation_id: &str| DenToolInvocationContext {
        bear_id,
        bear_slug: "boundreviewbear".to_string(),
        binding_id: hats::turn_binding::NativeTurnSource::Conversation(match conversation_id {
            "conv-bound-review-a" => a.id,
            "conv-bound-review-b" => b.id,
            "conv-bound-review-other-hat" => c.id,
            _ => unreachable!("unknown fixture conversation"),
        })
        .binding_id(bear_id.into()),
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id,
        username: None,
        membership_role: None,
        conversation_id: conversation_id.to_string(),
        session_id: format!("review-session-{user_id}"),
        work_run_id: None,
        client_session_id: Some(format!("review-session-{user_id}")),
        conversation_selection: Some(conversation_id.to_string()),
        runtime_target: None,
        workspace_roots: vec![],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(Uuid::new_v4().to_string()),
        channel: DenToolChannelContext::default(),
    };
    for (user, external) in [
        (admin, "conv-bound-review-a"),
        (member, "conv-bound-review-b"),
    ] {
        crate::core::tools::tests::source_fixture::bind_tool_client(
            &pool,
            &context(user, external),
        )
        .await?;
    }
    let request = |id: Uuid| {
        json!({
            "source_memory_id": id,
            "suggested_action": "propose_hat",
            "title": "Distill code safety",
            "summary": "A candidate for shared security practice",
            "proposed_content": "Proposed text is not published",
            "refs": {"verified_hat_source": {"memory_id": shared.memory_id, "hat_id": other_hat.id}},
            "sensitivity": "normal",
        })
    };
    let invoke = |args, ctx| {
        invoke_den_tool_for_origin(
            &pool,
            &config,
            &stores,
            DEN_MEMORY_REQUEST_REVIEW,
            args,
            ctx,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            Governance::Interactive,
        )
    };
    let result = invoke(request(private_id), context(admin, "conv-bound-review-a")).await?;
    let id = Uuid::parse_str(result["proposal"]["id"].as_str().unwrap())?;
    let queued = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_reflection_runs
         WHERE bear_id = $1 AND lane = 'memory_curate' AND trigger = 'verified_hat_intake'",
        bear_id,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        queued, 1,
        "verified proposals enqueue autonomous Curate work"
    );
    assert_eq!(
        result["proposal"]["verified_hat_source"]["memory_id"],
        private.memory_id
    );
    assert_eq!(
        result["proposal"]["verified_hat_source"]["hat_id"],
        hat.id.to_string()
    );
    assert!(result["proposal"]["source_paths"]
        .as_array()
        .unwrap()
        .is_empty());
    let stored = den_memory::get_memory_proposal(&store, &id.to_string())
        .await?
        .unwrap();
    assert_eq!(stored.verified_hat_source.unwrap().memory_id, private_id);
    assert_eq!(stored.verified_hat_source.unwrap().hat_id, hat.id);
    assert!(
        stored.payload_json["refs"]["verified_hat_source"]["memory_id"] != private.memory_id,
        "model-provided JSON remains untrusted"
    );
    let curation = den_runtime::memory_curate_executor::execute_memory_curate_proposals(
        &pool,
        &config,
        &stores,
        bear_id,
        Some("verified_hat_intake"),
        &[id],
    )
    .await?;
    assert_eq!(curation.outcomes[0].status, "pending");
    assert_eq!(curation.outcomes[0].triage, "await_curator");
    assert!(curation.resolved_proposal_ids.is_empty());
    assert!(
        curation.briefing.is_empty(),
        "no automatic human-review queue"
    );
    assert_eq!(
        den_memory::get_memory_proposal(&store, &id.to_string())
            .await?
            .unwrap()
            .status,
        "pending"
    );
    let published: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memory_records WHERE scope_type = 'hat'")
            .fetch_one(store.pool())
            .await?;
    assert_eq!(published, 0);

    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.id.as_uuid()
    )
    .execute(&pool)
    .await?;
    let job_shared_note = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(a.id), "job-shared-note"),
        "note",
        "pair",
        None,
        "A private note for the Job-capable hat",
        &json!({}),
    )
    .await?;
    let job_candidate = invoke(
        request(Uuid::parse_str(&job_shared_note.memory_id)?),
        context(admin, "conv-bound-review-a"),
    )
    .await?;
    assert_eq!(
        job_candidate["proposal"]["verified_hat_source"]["hat_id"],
        hat.id.to_string()
    );
    assert_eq!(
        sqlx::query_scalar!(
            "SELECT count(*) AS \"count!: i64\" FROM bear_reflection_runs
         WHERE bear_id = $1 AND lane = 'memory_curate' AND trigger = 'verified_hat_intake'",
            bear_id,
        )
        .fetch_one(&pool)
        .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM memory_records WHERE scope_type = 'hat'"
        )
        .fetch_one(store.pool())
        .await?,
        0_i64
    );

    // Same hat, different person's source; another hat, same person; and a
    // known core ID are all locators, never evidence of source authority.
    for (args, ctx) in [
        (request(private_id), context(member, "conv-bound-review-b")),
        (
            request(Uuid::parse_str(&other.memory_id)?),
            context(admin, "conv-bound-review-a"),
        ),
        (
            request(Uuid::parse_str(&shared.memory_id)?),
            context(admin, "conv-bound-review-a"),
        ),
    ] {
        assert!(invoke(args, ctx).await.is_err());
    }
    assert!(invoke(
        json!({
            "source_paths": ["pair/guess.md"], "suggested_action": "unspecified",
            "title": "Path claim", "summary": "Cannot select a hat with a path",
        }),
        context(admin, "conv-bound-review-a")
    )
    .await
    .is_err());
    assert!(invoke(
        json!({
            "source_memory_id": private_id, "source_paths": ["pair/guess.md"],
            "suggested_action": "propose_hat", "title": "Mixed source", "summary": "No mixed source"
        }),
        context(admin, "conv-bound-review-a")
    )
    .await
    .is_err());
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, false, false)
        .await?;
    assert!(
        invoke(request(private_id), context(admin, "conv-bound-review-a"))
            .await
            .is_err(),
        "disabling autonomous sharing must immediately stop new candidate intake"
    );
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, true, true).await?;
    den_memory::mark_memory_record_lifecycle(
        &store,
        &private.memory_id,
        "archived",
        Some("no longer eligible"),
    )
    .await?;
    let stale = den_runtime::memory_curate_executor::execute_memory_curate_proposals(
        &pool,
        &config,
        &stores,
        bear_id,
        Some("verified_hat_intake"),
        &[id],
    )
    .await?;
    assert_eq!(stale.outcomes[0].status, "rejected");
    assert!(stale.briefing.is_empty());
    assert_eq!(
        den_memory::get_memory_proposal(&store, &id.to_string())
            .await?
            .unwrap()
            .status,
        "rejected"
    );

    let active_note = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(a.id), "new-note"),
        "note",
        "pair",
        None,
        "A later private note",
        &json!({}),
    )
    .await?;
    let second = invoke(
        request(Uuid::parse_str(&active_note.memory_id)?),
        context(admin, "conv-bound-review-a"),
    )
    .await?;
    let second_id = Uuid::parse_str(second["proposal"]["id"].as_str().unwrap())?;
    sqlx::query!(
        "UPDATE conversations SET status = 'archived' WHERE id = $1",
        a.id
    )
    .execute(&pool)
    .await?;
    let revoked = den_runtime::memory_curate_executor::execute_memory_curate_proposals(
        &pool,
        &config,
        &stores,
        bear_id,
        Some("verified_hat_intake"),
        &[second_id],
    )
    .await?;
    assert_eq!(revoked.outcomes[0].status, "rejected");
    assert!(revoked.briefing.is_empty());
    Ok(())
}
