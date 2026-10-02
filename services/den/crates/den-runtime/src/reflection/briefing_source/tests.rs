use super::*;
use crate::reflection::{
    conductor::{
        claim_next_memory_curate_run, create_run, enqueue_memory_curate_for_proposals,
        mark_memory_curate_completed, mark_memory_curate_failed, CreateReflectionRun,
        ProposalEnqueueParams,
    },
    conversations::{bind_memory_curate_run_conversation, ensure_memory_curate_conversation},
};
use den_service::{bears::db, conversation::persistence};
use futures::stream;

async fn bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug,
                name: "Briefing source test",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    )
}

async fn enqueue(pool: &PgPool, bear_id: BearId) -> ReflectionRunId {
    let run = enqueue_memory_curate_for_proposals(
        pool,
        ProposalEnqueueParams {
            bear_id: bear_id.as_uuid(),
            binding_id: None,
            conversation_id: None,
            conversation_key: None,
            conversation_date: None,
            trigger: "briefing_source_test",
            proposal_ids: vec![],
        },
    )
    .await
    .unwrap();
    ReflectionRunId::new(run.id)
}

async fn running_source(pool: &PgPool) -> CurateBriefingSource {
    let bear_id = bear(pool, "briefing-source-bear").await;
    let run_id = enqueue(pool, bear_id).await;
    claim_next_memory_curate_run(pool, bear_id.as_uuid())
        .await
        .unwrap()
        .unwrap();
    let date = time::Date::from_calendar_date(2026, time::Month::October, 2).unwrap();
    let conversation = ensure_memory_curate_conversation(pool, bear_id.as_uuid(), None, date)
        .await
        .unwrap();
    bind_memory_curate_run_conversation(
        pool,
        bear_id.as_uuid(),
        run_id.as_uuid(),
        conversation.conversation_id.as_deref().unwrap(),
    )
    .await
    .unwrap();
    resolve_curate_briefing_source(pool, bear_id, run_id)
        .await
        .unwrap()
}

async fn rebind(pool: &PgPool, run_id: ReflectionRunId, conversation_id: &str) {
    sqlx::query!(
        "UPDATE bear_reflection_runs SET conversation_id = $2 WHERE id = $1",
        run_id.as_uuid(),
        conversation_id,
    )
    .execute(pool)
    .await
    .unwrap();
}

fn delta(text: &str) -> Result<RuntimeStreamEvent, DenError> {
    Ok(RuntimeStreamEvent::Semantic(
        RuntimeSemanticEvent::AssistantTextDelta { text: text.into() },
    ))
}

#[sqlx::test(migrations = "../../migrations")]
async fn curate_briefing_source_requires_running_bear_lane_and_binding(pool: PgPool) {
    let owner = bear(&pool, "briefing-source-owner").await;
    let other = bear(&pool, "briefing-source-other").await;
    let run_id = enqueue(&pool, owner).await;
    assert!(resolve_curate_briefing_source(&pool, owner, run_id)
        .await
        .is_err());
    claim_next_memory_curate_run(&pool, owner.as_uuid())
        .await
        .unwrap()
        .unwrap();
    // Running alone does not confer a conversation destination.
    assert!(resolve_curate_briefing_source(&pool, owner, run_id)
        .await
        .is_err());
    let date = time::Date::from_calendar_date(2026, time::Month::October, 2).unwrap();
    let conversation = ensure_memory_curate_conversation(&pool, owner.as_uuid(), None, date)
        .await
        .unwrap();
    let external_id = conversation.conversation_id.as_deref().unwrap();
    bind_memory_curate_run_conversation(&pool, owner.as_uuid(), run_id.as_uuid(), external_id)
        .await
        .unwrap();
    let queued_id = enqueue(&pool, owner).await;
    bind_memory_curate_run_conversation(&pool, owner.as_uuid(), queued_id.as_uuid(), external_id)
        .await
        .unwrap();
    // Even a valid binding cannot authorize a queued run.
    assert!(resolve_curate_briefing_source(&pool, owner, queued_id)
        .await
        .is_err());
    let source = resolve_curate_briefing_source(&pool, owner, run_id)
        .await
        .unwrap();
    assert_eq!(source.bear_id(), owner);
    assert_eq!(source.run_id(), run_id);
    assert_eq!(source.conversation_id().as_str(), external_id);
    assert_eq!(
        source.session_id().as_str(),
        format!("memory-curate-{}", run_id.as_uuid())
    );
    assert!(resolve_curate_briefing_source(&pool, other, run_id)
        .await
        .is_err());
    assert!(
        resolve_curate_briefing_source(&pool, owner, ReflectionRunId::new(Uuid::new_v4()))
            .await
            .is_err()
    );

    let wrong_lane = create_run(
        &pool,
        CreateReflectionRun {
            bear_id: owner.as_uuid(),
            lane: "archive_harvest",
            trigger: "briefing_source_test",
            status: "running",
            role_agent_id: None,
            conversation_id: Some(external_id),
            conversation_key: None,
            conversation_date: None,
            input_summary: serde_json::json!({}),
            output_summary: serde_json::json!({}),
            error: None,
        },
    )
    .await
    .unwrap();
    assert!(
        resolve_curate_briefing_source(&pool, owner, ReflectionRunId::new(wrong_lane.id))
            .await
            .is_err()
    );

    for invalid in ["", "   ", "conv-nonexistent"] {
        rebind(&pool, run_id, invalid).await;
        assert!(resolve_curate_briefing_source(&pool, owner, run_id)
            .await
            .is_err());
    }
    let ordinary = persistence::ensure_conversation_for_external_id(
        &pool,
        owner.as_uuid(),
        None,
        "conv-ordinary-not-curation",
        None,
        None,
    )
    .await
    .unwrap();
    rebind(
        &pool,
        run_id,
        ordinary.external_conversation_id.as_deref().unwrap(),
    )
    .await;
    assert!(resolve_curate_briefing_source(&pool, owner, run_id)
        .await
        .is_err());
    let foreign = ensure_memory_curate_conversation(&pool, other.as_uuid(), None, date)
        .await
        .unwrap();
    rebind(&pool, run_id, foreign.conversation_id.as_deref().unwrap()).await;
    assert!(resolve_curate_briefing_source(&pool, owner, run_id)
        .await
        .is_err());
    rebind(&pool, run_id, external_id).await;
    mark_memory_curate_failed(&pool, owner.as_uuid(), run_id.as_uuid(), "test failure")
        .await
        .unwrap();
    assert!(source.require_live(&pool).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn curate_briefing_text_uses_checked_source_and_concatenates_deltas(pool: PgPool) {
    let source = running_source(&pool).await;
    let result = collect_curate_briefing_text(
        &pool,
        source.clone(),
        Box::pin(stream::iter(vec![delta("Curated "), delta("summary")])),
    )
    .await
    .unwrap();
    assert_eq!(result.source, source);
    assert_eq!(result.text, "Curated summary");
}

#[sqlx::test(migrations = "../../migrations")]
async fn curate_briefing_text_rejects_completion_during_stream(pool: PgPool) {
    let source = running_source(&pool).await;
    let owner = source.bear_id();
    let run_id = source.run_id();
    let stream_pool = pool.clone();
    let events = stream::iter(vec![delta("Partial summary")]).chain(stream::once(async move {
        mark_memory_curate_completed(
            &stream_pool,
            owner.as_uuid(),
            run_id.as_uuid(),
            serde_json::json!({}),
        )
        .await
        .unwrap();
        delta(" must not be projected")
    }));
    assert!(
        collect_curate_briefing_text(&pool, source, Box::pin(events))
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn curate_briefing_text_rejects_rebinding_during_stream(pool: PgPool) {
    let source = running_source(&pool).await;
    let run_id = source.run_id();
    let replacement = persistence::ensure_conversation_for_external_id(
        &pool,
        source.bear_id().as_uuid(),
        None,
        "conv-briefing-rebound",
        None,
        None,
    )
    .await
    .unwrap();
    let stream_pool = pool.clone();
    let events = stream::once(async move {
        rebind(
            &stream_pool,
            run_id,
            replacement.external_conversation_id.as_deref().unwrap(),
        )
        .await;
        delta("Must not be projected to either destination")
    });
    assert!(
        collect_curate_briefing_text(&pool, source, Box::pin(events))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn curate_briefing_text_rejects_tool_request_without_consuming_continuation() {
    // The collector must reject before any DB access or following stream poll.
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let source = CurateBriefingSource {
        run_id: ReflectionRunId::new(Uuid::new_v4()),
        bear_id: BearId::new(Uuid::new_v4()),
        canonical_conversation_id: Uuid::new_v4(),
        conversation_id: ConversationId::new("conv-unused"),
        session_id: SessionId::new("unused"),
    };
    let tool = RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
        tool_call_id: "call-unexpected".into(),
        tool_name: "memory_write_entry".into(),
        title: None,
        kind: None,
        arguments: serde_json::json!({}),
        approval_request_id: None,
        approval_required: false,
        approval_reason: None,
        run_id: None,
    });
    let events =
        stream::iter(vec![delta("Partial summary"), Ok(tool)]).chain(stream::once(async {
            panic!("a tool-free briefing must not continue after a tool request");
            #[allow(unreachable_code)]
            delta("unreachable")
        }));
    let error = collect_curate_briefing_text(&pool, source, Box::pin(events))
        .await
        .unwrap_err();
    assert!(matches!(error, DenError::Authorization(_)));
}
