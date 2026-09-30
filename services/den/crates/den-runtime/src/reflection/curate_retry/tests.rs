use super::*;
use crate::reflection::conductor::{
    claim_next_memory_curate_run, enqueue_memory_curate_for_proposals,
    list_queued_memory_curate_runs, mark_memory_curate_completed, ProposalEnqueueParams,
};
use den_core::{
    config::Config,
    ids::{BearId, UserId},
};
use den_memory::{
    append_memory_record, create_verified_hat_proposal, LogicalMemoryPath, MemorySource,
    MemoryStoreManager, VerifiedHatProposalSource,
};
use den_service::{
    bears::{db, hats},
    conversation::persistence,
};
use serde_json::json;

fn retryable(id: Uuid) -> CurateProposalOutcome {
    CurateProposalOutcome {
        proposal_id: id,
        status: "pending".into(),
        suggested_action: "propose_hat".into(),
        triage: "await_curator".into(),
        result_path: None,
        error: Some("Synthesis unavailable".into()),
        retry_reason: Some(CurateRetryReason::SynthesisUnavailable),
    }
}

#[test]
fn retry_plan_is_typed_rate_limited_and_only_for_synthesis_failures() {
    let id = Uuid::new_v4();
    let initial = json!({"proposal_ids": [id]});
    let first = retry_plan(&initial, &[retryable(id)]).unwrap();
    assert_eq!(first.attempt, 1);
    assert_eq!(first.delay, Duration::minutes(1));
    assert_eq!(first.proposal_ids, vec![id]);
    assert_eq!(
        retry_plan(&json!({"retry_attempt": 1}), &[retryable(id)])
            .unwrap()
            .attempt,
        2
    );
    for (attempt, next, delay) in [
        (2, 3, Duration::hours(1)),
        (3, 4, Duration::hours(6)),
        (4, 5, Duration::days(1)),
        (5, 5, Duration::days(1)),
        (255, 5, Duration::days(1)),
    ] {
        let plan = retry_plan(&json!({"retry_attempt": attempt}), &[retryable(id)]).unwrap();
        assert_eq!(plan.attempt, next);
        assert_eq!(plan.delay, delay);
    }
    assert!(retry_plan(&json!({"retry_attempt": "invalid"}), &[retryable(id)]).is_none());
    let mut denied = retryable(id);
    denied.retry_reason = None;
    assert!(retry_plan(&initial, &[denied]).is_none());
    let mut completed = retryable(id);
    completed.status = "approved".into();
    assert!(retry_plan(&initial, &[completed]).is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn exhausted_verified_candidate_is_recovered_once_without_a_model_call(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('curatesynth@example.test', 'curatesynth') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "recovercuratebear",
            name: "Recovery Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    db::grant_membership(&pool, user, bear_id, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Research",
        "Recovery",
    )
    .await
    .unwrap();
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, true, true)
        .await
        .unwrap();
    let conversation = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user),
        "conv-recover-curate",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), conversation.id, hat.id)
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("curate-recovery-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let note = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation.id), "note"),
        "note",
        "pair",
        None,
        "private source",
        &json!({}),
    )
    .await
    .unwrap();
    let proposal = create_verified_hat_proposal(
        &store,
        "normal",
        false,
        &json!({"summary":"Safe derived fact"}),
        VerifiedHatProposalSource {
            memory_id: Uuid::parse_str(&note.memory_id).unwrap(),
            hat_id: hat.id,
        },
    )
    .await
    .unwrap();
    let id = Uuid::parse_str(&proposal.proposal_id).unwrap();
    enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: None,
            conversation_id: None,
            conversation_key: None,
            conversation_date: None,
            trigger: "verified_hat_intake",
            proposal_ids: vec![id],
        },
    )
    .await
    .unwrap();
    let mut run = claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..2 {
        let finished = complete_and_schedule(&pool, &run, json!({}), &[retryable(id)])
            .await
            .unwrap();
        let delayed = finished.retry_run.unwrap();
        sqlx::query!(
            "UPDATE bear_reflection_runs SET available_at = NOW() WHERE id = $1",
            delayed.id
        )
        .execute(&pool)
        .await
        .unwrap();
        run = claim_next_memory_curate_run(&pool, bear_id)
            .await
            .unwrap()
            .unwrap();
    }
    // Simulate the old implementation's terminal third attempt: a completed
    // retry-2 run with a typed failure and no queued successor.
    mark_memory_curate_completed(&pool, bear_id, run.id, json!({"outcomes": [retryable(id)]}))
        .await
        .unwrap();
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, false, false)
        .await
        .unwrap();
    assert_eq!(
        recover_exhausted_once(&pool, &stores, 0)
            .await
            .unwrap()
            .queued,
        0
    );
    hats::manage::set_auto_curate_enabled(&pool, BearId::new(bear_id), hat.id, true, true)
        .await
        .unwrap();
    let first = recover_exhausted_once(&pool, &stores, 0).await.unwrap();
    assert_eq!(first.inspected, 1);
    assert_eq!(first.queued, 1);
    assert_eq!(
        recover_exhausted_once(&pool, &stores, 0)
            .await
            .unwrap()
            .queued,
        0
    );
    let queued = list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].input_summary["retry_attempt"], 5);
    assert_eq!(
        queued[0].input_summary["recovered_from"],
        run.id.to_string()
    );
    assert!(claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        den_memory::get_memory_proposal(&store, &id.to_string())
            .await
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );
    assert_eq!(
        recover_exhausted_once(&pool, &stores, 100)
            .await
            .unwrap()
            .inspected,
        0
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn retry_run_is_durable_not_due_early_and_cools_down_to_daily(pool: PgPool) {
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "curateretrybear",
            name: "Curate retry test",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    enqueue_memory_curate_for_proposals(
        &pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: None,
            conversation_id: None,
            conversation_key: None,
            conversation_date: None,
            trigger: "verified_hat_intake",
            proposal_ids: vec![id],
        },
    )
    .await
    .unwrap();
    let initial = claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .unwrap();
    let finished =
        complete_and_schedule(&pool, &initial, json!({"pending": true}), &[retryable(id)])
            .await
            .unwrap();
    assert_eq!(finished.completed.status, "completed");
    let delayed = finished.retry_run.unwrap();
    assert_eq!(delayed.input_summary["retry_attempt"], 1);
    assert_eq!(delayed.input_summary["proposal_ids"][0], id.to_string());
    assert!(claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        list_queued_memory_curate_runs(&pool, bear_id, 10)
            .await
            .unwrap()
            .len(),
        1,
        "the delayed run remains visible to operators"
    );
    sqlx::query!(
        "UPDATE bear_reflection_runs SET available_at = NOW() WHERE id = $1",
        delayed.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let first_retry = claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .unwrap();
    let finished = complete_and_schedule(&pool, &first_retry, json!({}), &[retryable(id)])
        .await
        .unwrap();
    assert_eq!(
        finished.retry_run.as_ref().unwrap().input_summary["retry_attempt"],
        2
    );
    let delayed = finished.retry_run.unwrap();
    assert!(claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .is_none());
    sqlx::query!(
        "UPDATE bear_reflection_runs SET available_at = NOW() WHERE id = $1",
        delayed.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let second_retry = claim_next_memory_curate_run(&pool, bear_id)
        .await
        .unwrap()
        .unwrap();
    let mut current = second_retry;
    for expected_attempt in [3, 4, 5, 5] {
        let finished = complete_and_schedule(&pool, &current, json!({}), &[retryable(id)])
            .await
            .unwrap();
        let delayed = finished.retry_run.unwrap();
        assert_eq!(delayed.input_summary["retry_attempt"], expected_attempt);

        assert!(claim_next_memory_curate_run(&pool, bear_id)
            .await
            .unwrap()
            .is_none());
        sqlx::query!(
            "UPDATE bear_reflection_runs SET available_at = NOW() WHERE id = $1",
            delayed.id
        )
        .execute(&pool)
        .await
        .unwrap();
        current = claim_next_memory_curate_run(&pool, bear_id)
            .await
            .unwrap()
            .unwrap();
    }
    let finished = complete_and_schedule(&pool, &current, json!({}), &[])
        .await
        .unwrap();
    assert!(
        finished.retry_run.is_none(),
        "a completed decision does not poll again"
    );
    assert!(list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .unwrap()
        .is_empty());
}
