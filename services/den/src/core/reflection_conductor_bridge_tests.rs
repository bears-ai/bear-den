//! Bridge tests for `den_runtime::reflection::conductor`. They run DB migrations
//! via `sqlx::migrate!("./migrations")` (the `den` crate owns the migrations dir),
//! so they live in the `den` crate rather than in `den-runtime`.

use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::Config;
use den_memory::MemoryStoreManager;
use den_runtime::memory_curate_executor;
use den_runtime::reflection_conductor::*;
use den_service::bears::RuntimeContextLabel;

async fn test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1/postgres".to_string());
    let pool = PgPoolOptions::new().connect(&database_url).await.ok()?;
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;
    Some(pool)
}

#[tokio::test]
async fn memory_curate_claim_next_run_starts_oldest_queued_run() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let bear_id = Uuid::new_v4();
    let first = enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: Some("pair-agent"),
            conversation_id: Some("conv-a"),
            conversation_key: Some("memory_curate:test-a"),
            conversation_date: None,
            trigger: "test",
            proposal_ids: vec![Uuid::new_v4()],
        },
    )
    .await
    .expect("enqueue first run");
    let second = enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: Some("pair-agent"),
            conversation_id: Some("conv-b"),
            conversation_key: Some("memory_curate:test-b"),
            conversation_date: None,
            trigger: "test",
            proposal_ids: vec![Uuid::new_v4()],
        },
    )
    .await
    .expect("enqueue second run");

    let queued = list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .expect("list queued runs");
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].id, first.id);
    assert_eq!(queued[1].id, second.id);

    let claimed = claim_next_memory_curate_run(&pool, bear_id)
        .await
        .expect("claim queued run")
        .expect("queued run available");
    assert_eq!(claimed.id, first.id);
    assert_eq!(claimed.status, "started");
    assert!(claimed.started_at.is_some());

    let remaining = list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .expect("list remaining queued runs");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, second.id);
}

#[tokio::test]
async fn memory_curate_worker_loop_processes_queued_runs_until_cancelled() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let bear_id = Uuid::new_v4();
    let proposal = den_service::memory_proposals::create(
        &pool,
        den_service::memory_proposals::CreateMemoryProposal {
            bear_id,
            source_profile: RuntimeContextLabel::ArmatureConversation,
            source_agent_id: Some("pair-agent".to_string()),
            source_paths: vec!["pair/notes/worker.md".to_string()],
            source_refs: serde_json::json!({"conversation_id": "conv-memory-curate-worker-test"}),
            suggested_action: "unspecified",
            target_ref: None,
            title: "Worker memory curate proposal",
            summary: "summary",
            rationale: "rationale",
            proposed_content: None,
            proposed_patch: None,
            refs: serde_json::json!({}),
            sensitivity: "normal",
            requires_human: false,
            project_to_conversation: false,
        },
    )
    .await
    .expect("create proposal");

    enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: Some("pair-agent"),
            conversation_id: Some("conv-memory-curate-worker-test"),
            conversation_key: Some("memory_curate:test-worker"),
            conversation_date: None,
            trigger: "pair_reflection",
            proposal_ids: vec![proposal.id],
        },
    )
    .await
    .expect("enqueue run");

    let token = tokio_util::sync::CancellationToken::new();
    let worker_pool = pool.clone();
    let worker_token = token.clone();
    let config = Arc::new(Config::load());
    let worker_stores = MemoryStoreManager::new(config.as_ref());
    let handle = tokio::spawn(async move {
        run_memory_curate_worker_loop(
            worker_pool,
            config,
            worker_stores,
            worker_token,
            std::time::Duration::from_millis(25),
        )
        .await
    });

    let mut saw_completed = false;
    for _ in 0..40 {
        let runs = sqlx::query(
            "SELECT status FROM bear_reflection_runs WHERE bear_id = $1 AND lane = 'memory_curate' ORDER BY created_at DESC LIMIT 1"
        )
        .bind(bear_id)
        .fetch_all(&pool)
        .await
        .expect("query run status");
        if let Some(row) = runs.first() {
            let status: String = row.get("status");
            if status == "completed" {
                saw_completed = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        saw_completed,
        "worker did not complete queued run before timeout"
    );

    token.cancel();
    handle.await.expect("worker join").expect("worker result");

    let updated_proposal = den_service::memory_proposals::get_for_bear(&pool, bear_id, proposal.id)
        .await
        .expect("reload proposal")
        .expect("proposal exists");
    assert_eq!(updated_proposal.status, "retained_local");
}

#[tokio::test]
async fn memory_curate_runner_completes_run_and_retains_pair_reflection_proposals_locally() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let bear_id = Uuid::new_v4();
    let proposal = den_service::memory_proposals::create(
        &pool,
        den_service::memory_proposals::CreateMemoryProposal {
            bear_id,
            source_profile: RuntimeContextLabel::ArmatureConversation,
            source_agent_id: Some("pair-agent".to_string()),
            source_paths: vec!["pair/notes/example.md".to_string()],
            source_refs: serde_json::json!({"conversation_id": "conv-memory-curate-test"}),
            suggested_action: "unspecified",
            target_ref: None,
            title: "Test memory curate proposal",
            summary: "summary",
            rationale: "rationale",
            proposed_content: None,
            proposed_patch: None,
            refs: serde_json::json!({}),
            sensitivity: "normal",
            requires_human: false,
            project_to_conversation: false,
        },
    )
    .await
    .expect("create proposal");

    let queued_run = enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: Some("pair-agent"),
            conversation_id: Some("conv-memory-curate-test"),
            conversation_key: Some("memory_curate:test-runner"),
            conversation_date: None,
            trigger: "pair_reflection",
            proposal_ids: vec![proposal.id],
        },
    )
    .await
    .expect("enqueue run");

    let config = Config::load();
    let stores = MemoryStoreManager::new(&config);
    let completed_run = run_next_memory_curate_once(&pool, &config, &stores, bear_id)
        .await
        .expect("run queued memory_curate")
        .expect("queued run processed");
    assert_eq!(completed_run.id, queued_run.id);
    assert_eq!(completed_run.status, "completed");
    assert!(completed_run.started_at.is_some());
    assert!(completed_run.completed_at.is_some());
    assert_eq!(
        completed_run.output_summary["resolution_status"],
        serde_json::json!("retained_local")
    );
    assert_eq!(
        completed_run.output_summary["resolved_proposal_ids"],
        serde_json::json!([proposal.id.to_string()])
    );

    let updated_proposal = den_service::memory_proposals::get_for_bear(&pool, bear_id, proposal.id)
        .await
        .expect("reload proposal")
        .expect("proposal exists");
    assert_eq!(updated_proposal.status, "retained_local");
    assert_eq!(updated_proposal.reviewer_profile.as_deref(), Some("curate"));
    assert_eq!(
        updated_proposal.reviewer_agent_id.as_deref(),
        Some(memory_curate_executor::MEMORY_CURATE_RUNNER_AGENT_ID)
    );

    let queued = list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .expect("list queued after runner");
    assert!(queued.is_empty());
}
