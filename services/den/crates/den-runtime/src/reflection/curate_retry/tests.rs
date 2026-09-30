use super::*;
use crate::reflection::conductor::{
    claim_next_memory_curate_run, enqueue_memory_curate_for_proposals,
    list_queued_memory_curate_runs, ProposalEnqueueParams,
};
use den_service::bears::db;
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
fn retry_plan_is_typed_bounded_and_only_for_synthesis_failures() {
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
    assert!(retry_plan(&json!({"retry_attempt": 2}), &[retryable(id)]).is_none());
    assert!(retry_plan(&json!({"retry_attempt": "invalid"}), &[retryable(id)]).is_none());
    let mut denied = retryable(id);
    denied.retry_reason = None;
    assert!(retry_plan(&initial, &[denied]).is_none());
    let mut completed = retryable(id);
    completed.status = "approved".into();
    assert!(retry_plan(&initial, &[completed]).is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn retry_run_is_durable_not_due_early_and_stops_after_three_attempts(pool: PgPool) {
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
    let finished = complete_and_schedule(&pool, &second_retry, json!({}), &[retryable(id)])
        .await
        .unwrap();
    assert!(finished.retry_run.is_none());
    assert!(list_queued_memory_curate_runs(&pool, bear_id, 10)
        .await
        .unwrap()
        .is_empty());
    let attempts: i64 = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_reflection_runs
         WHERE bear_id = $1 AND lane = 'memory_curate'",
        bear_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 3);
}
