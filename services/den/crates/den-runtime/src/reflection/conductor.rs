use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use den_memory::MemoryStoreManager;
use den_service::conversation::events::{
    canonical_persistence_context, memory_curate_completed_projection,
    memory_curate_enqueued_projection, memory_curate_failed_projection,
    memory_curate_started_projection, project_to_conversation,
    spawn_persist_assistant_summary_message, ProjectionProvenance, ProjectionSource,
};
use std::sync::Arc;

use crate::{
    memory::{record_reflection_outcome_complete, record_reflection_outcome_start},
    memory_curate_executor::{self, MemoryCurateRunOutput},
    native_runtime::{
        compose_curate_briefing_prompt, run_native_curate_briefing_collect_assistant_text,
        NativeRuntimeDeps,
    },
    recall::{reconcile_bear, QdrantRecall},
    reflection::archive_harvest::harvest_compaction_artifacts_once,
    reflection::briefing_source::CurateBriefingSource,
    reflection::conversations::{
        bind_memory_curate_run_conversation, ensure_memory_curate_conversation,
        touch_memory_curate_conversation,
    },
    reflection::curate_retry,
    reflection::ReflectionRunId,
};

use crate::runtime_compaction::{run_compaction_job, TurnCompactionState, TurnCompactionTrigger};
use den_core::ids::BearId;
use den_core::{config::Config, DenError};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ReflectionRunRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub lane: String,
    pub trigger: String,
    pub status: String,
    pub role_agent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub conversation_key: Option<String>,
    pub conversation_date: Option<Date>,
    pub input_summary: serde_json::Value,
    pub output_summary: serde_json::Value,
    pub error: Option<String>,
    pub started_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

// ---------------------------------------------------------------------------
// archive_harvest lane: mine compaction artifacts into memory proposals.
// ---------------------------------------------------------------------------

pub async fn enqueue_archive_harvest_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    trigger: &str,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(ReflectionRunRow, r"
        INSERT INTO bear_reflection_runs (bear_id, lane, trigger, status, input_summary, output_summary)
        SELECT $1, 'archive_harvest', $2, 'queued', '{}'::jsonb, '{}'::jsonb
        WHERE NOT EXISTS (
            SELECT 1 FROM bear_reflection_runs
            WHERE bear_id = $1 AND lane = 'archive_harvest' AND status = 'queued'
        )
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ", bear_id, trigger)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn claim_next_archive_harvest_run(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        WITH next_run AS (
            SELECT id
            FROM bear_reflection_runs
            WHERE bear_id = $1 AND lane = 'archive_harvest' AND status = 'queued'
            ORDER BY created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE bear_reflection_runs runs
        SET status = 'running', started_at = COALESCE(started_at, NOW())
        FROM next_run
        WHERE runs.id = next_run.id
        RETURNING runs.id, runs.bear_id, runs.lane, runs.trigger, runs.status,
                  runs.role_agent_id, runs.conversation_id, runs.conversation_key,
                  runs.conversation_date, runs.input_summary, runs.output_summary,
                  runs.error, runs.started_at, runs.completed_at, runs.created_at
        ",
        bear_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn mark_archive_harvest_completed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    output_summary: serde_json::Value,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'completed', output_summary = $3, error = NULL, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'archive_harvest'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        output_summary,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn mark_archive_harvest_failed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    error: &str,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'failed', error = $3, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'archive_harvest'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        error,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_bears_with_queued_archive_harvest_runs(pool: &PgPool) -> Result<Vec<Uuid>, DenError> {
    let rows = sqlx::query_scalar!(
        r"
        SELECT DISTINCT bear_id
        FROM bear_reflection_runs
        WHERE lane = 'archive_harvest' AND status = 'queued'
        ORDER BY bear_id
        "
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn run_next_archive_harvest_once(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let Some(run) = claim_next_archive_harvest_run(pool, bear_id).await? else {
        return Ok(None);
    };

    let run_id = run.id.to_string();
    match harvest_compaction_artifacts_once(pool, config, stores, bear_id, 20, Some(&run_id)).await
    {
        Ok(output) => {
            let completed = mark_archive_harvest_completed(
                pool,
                bear_id,
                run.id,
                serde_json::json!({
                    "scanned_artifacts": output.scanned_artifacts,
                    "candidate_count": output.candidate_count,
                    "discarded_count": output.discarded_count,
                    "no_candidate_count": output.no_candidate_count,
                    "created_proposal_ids": output.created_proposal_ids,
                }),
            )
            .await?;
            Ok(Some(completed))
        }
        Err(error) => {
            let failed =
                mark_archive_harvest_failed(pool, bear_id, run.id, &error.to_string()).await?;
            Ok(Some(failed))
        }
    }
}

pub async fn run_archive_harvest_worker_loop(
    pool: PgPool,
    config: Arc<Config>,
    stores: MemoryStoreManager,
    worker_token: tokio_util::sync::CancellationToken,
    poll_interval: std::time::Duration,
) -> Result<(), DenError> {
    loop {
        tokio::select! {
            () = worker_token.cancelled() => { break; }
            () = tokio::time::sleep(poll_interval) => {}
        }

        let bear_ids = list_bears_with_queued_archive_harvest_runs(&pool).await?;
        for bear_id in bear_ids {
            if worker_token.is_cancelled() {
                break;
            }
            match run_next_archive_harvest_once(&pool, config.as_ref(), &stores, bear_id).await {
                Ok(Some(run)) => tracing::info!(
                    bear_id = %bear_id,
                    reflection_run_id = %run.id,
                    status = %run.status,
                    output = %run.output_summary,
                    "archive_harvest worker processed queued run"
                ),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    bear_id = %bear_id,
                    error = %error,
                    "archive_harvest worker run failed; continuing"
                ),
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CreateReflectionRun<'a> {
    pub bear_id: Uuid,
    pub lane: &'a str,
    pub trigger: &'a str,
    pub status: &'a str,
    pub role_agent_id: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub conversation_key: Option<&'a str>,
    pub conversation_date: Option<Date>,
    pub input_summary: serde_json::Value,
    pub output_summary: serde_json::Value,
    pub error: Option<&'a str>,
}

pub async fn create_run(
    pool: &PgPool,
    params: CreateReflectionRun<'_>,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        INSERT INTO bear_reflection_runs (
            bear_id, lane, trigger, status, role_agent_id,
            conversation_id, conversation_key, conversation_date,
            input_summary, output_summary, error
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        params.bear_id,
        params.lane,
        params.trigger,
        params.status,
        params.role_agent_id,
        params.conversation_id,
        params.conversation_key,
        params.conversation_date,
        params.input_summary,
        params.output_summary,
        params.error
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub struct ProposalEnqueueParams<'a> {
    pub bear_id: Uuid,
    pub binding_id: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub conversation_key: Option<&'a str>,
    pub conversation_date: Option<Date>,
    pub trigger: &'a str,
    pub proposal_ids: Vec<Uuid>,
}

fn reflection_conductor_provenance(row: &ReflectionRunRow) -> ProjectionProvenance {
    ProjectionProvenance {
        source: ProjectionSource::ReflectionConductor,
        scope_id: format!("bear:{}:lane:{}", row.bear_id, row.lane),
    }
}

fn project_memory_curate_enqueued(pool: &PgPool, row: &ReflectionRunRow, proposal_ids: Vec<Uuid>) {
    project_to_conversation(
        pool,
        row.bear_id,
        None,
        row.conversation_id.as_deref(),
        memory_curate_enqueued_projection(
            reflection_conductor_provenance(row),
            row.id,
            row.lane.clone(),
            row.trigger.clone(),
            row.status.clone(),
            proposal_ids,
            row.conversation_key.clone(),
            row.conversation_date,
            row.created_at,
        ),
    );
}

fn project_memory_curate_started(pool: &PgPool, row: &ReflectionRunRow, proposal_ids: Vec<Uuid>) {
    project_to_conversation(
        pool,
        row.bear_id,
        None,
        row.conversation_id.as_deref(),
        memory_curate_started_projection(
            reflection_conductor_provenance(row),
            row.id,
            row.lane.clone(),
            row.trigger.clone(),
            row.status.clone(),
            proposal_ids,
            row.conversation_key.clone(),
            row.conversation_date,
            row.started_at,
        ),
    );
}

fn project_memory_curate_completed(pool: &PgPool, row: &ReflectionRunRow, proposal_ids: Vec<Uuid>) {
    project_to_conversation(
        pool,
        row.bear_id,
        None,
        row.conversation_id.as_deref(),
        memory_curate_completed_projection(
            reflection_conductor_provenance(row),
            row.id,
            row.lane.clone(),
            row.trigger.clone(),
            row.status.clone(),
            proposal_ids,
            row.conversation_key.clone(),
            row.conversation_date,
            row.completed_at,
        ),
    );
}

fn project_memory_curate_failed(pool: &PgPool, row: &ReflectionRunRow, proposal_ids: Vec<Uuid>) {
    project_to_conversation(
        pool,
        row.bear_id,
        None,
        row.conversation_id.as_deref(),
        memory_curate_failed_projection(
            reflection_conductor_provenance(row),
            row.id,
            row.lane.clone(),
            row.trigger.clone(),
            row.status.clone(),
            proposal_ids,
            row.conversation_key.clone(),
            row.conversation_date,
            row.error.clone(),
            row.completed_at,
        ),
    );
}

pub async fn enqueue_memory_curate_for_proposals(
    pool: &PgPool,
    params: ProposalEnqueueParams<'_>,
) -> Result<ReflectionRunRow, DenError> {
    let proposal_ids = params.proposal_ids;
    let proposal_id_values: Vec<serde_json::Value> = proposal_ids
        .iter()
        .map(|id| serde_json::Value::String(id.to_string()))
        .collect();
    let row = create_run(
        pool,
        CreateReflectionRun {
            bear_id: params.bear_id,
            lane: "memory_curate",
            trigger: params.trigger,
            status: "queued",
            role_agent_id: params.binding_id,
            conversation_id: params.conversation_id,
            conversation_key: params.conversation_key,
            conversation_date: params.conversation_date,
            input_summary: serde_json::json!({ "proposal_ids": proposal_id_values }),
            output_summary: serde_json::json!({}),
            error: None,
        },
    )
    .await?;
    project_memory_curate_enqueued(pool, &row, proposal_ids);
    Ok(row)
}

pub async fn list_queued_memory_curate_runs(
    pool: &PgPool,
    bear_id: Uuid,
    limit: i64,
) -> Result<Vec<ReflectionRunRow>, DenError> {
    let rows = sqlx::query_as!(
        ReflectionRunRow,
        r"
        SELECT id, bear_id, lane, trigger, status, role_agent_id,
               conversation_id, conversation_key, conversation_date,
               input_summary, output_summary, error,
               started_at, completed_at, created_at
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane = 'memory_curate'
          AND status = 'queued'
        ORDER BY available_at ASC, created_at ASC
        LIMIT $2
        ",
        bear_id,
        limit.clamp(1, 200)
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn claim_next_memory_curate_run(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        WITH next_run AS (
            SELECT id
            FROM bear_reflection_runs
            WHERE bear_id = $1
              AND lane = 'memory_curate'
              AND status = 'queued'
              AND available_at <= NOW()
            ORDER BY available_at ASC, created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE bear_reflection_runs runs
        SET status = 'running',
            started_at = COALESCE(started_at, NOW())
        FROM next_run
        WHERE runs.id = next_run.id
        RETURNING runs.id, runs.bear_id, runs.lane, runs.trigger, runs.status,
                  runs.role_agent_id, runs.conversation_id, runs.conversation_key,
                  runs.conversation_date, runs.input_summary, runs.output_summary,
                  runs.error, runs.started_at, runs.completed_at, runs.created_at
        ",
        bear_id
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut run = row;
    if let Some(conversation_date) = run.conversation_date {
        let reflection_conversation = ensure_memory_curate_conversation(
            pool,
            run.bear_id,
            run.role_agent_id.as_deref(),
            conversation_date,
        )
        .await?;
        if let Some(conversation_id) = reflection_conversation.conversation_id.as_deref() {
            bind_memory_curate_run_conversation(pool, run.bear_id, run.id, conversation_id).await?;
            run.conversation_id = Some(conversation_id.to_string());
        }
        let _ = touch_memory_curate_conversation(pool, run.bear_id, conversation_date).await;
    }
    project_memory_curate_started(pool, &run, proposal_ids_from_summary(&run.input_summary));
    Ok(Some(run))
}

pub async fn mark_memory_curate_started(
    pool: &PgPool,
    bear_id: Uuid,
    reflection_run_id: Uuid,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'running',
            started_at = COALESCE(started_at, NOW())
        WHERE bear_id = $1 AND id = $2 AND lane = 'memory_curate'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        reflection_run_id
    )
    .fetch_one(pool)
    .await?;
    let run = row;
    project_memory_curate_started(pool, &run, proposal_ids_from_summary(&run.input_summary));
    Ok(run)
}

pub async fn mark_memory_curate_completed(
    pool: &PgPool,
    bear_id: Uuid,
    reflection_run_id: Uuid,
    output_summary: serde_json::Value,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'completed',
            output_summary = $3,
            error = NULL,
            completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'memory_curate'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        reflection_run_id,
        output_summary
    )
    .fetch_one(pool)
    .await?;
    let run = row;
    project_memory_curate_completed(pool, &run, proposal_ids_from_summary(&run.input_summary));
    Ok(run)
}

pub async fn mark_memory_curate_failed(
    pool: &PgPool,
    bear_id: Uuid,
    reflection_run_id: Uuid,
    error: &str,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'failed',
            error = $3,
            completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'memory_curate'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        reflection_run_id,
        error
    )
    .fetch_one(pool)
    .await?;
    let run = row;
    project_memory_curate_failed(pool, &run, proposal_ids_from_summary(&run.input_summary));
    Ok(run)
}

pub async fn run_next_memory_curate_once(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let Some(run) = claim_next_memory_curate_run(pool, bear_id).await? else {
        return Ok(None);
    };

    let input_summary = run.input_summary.to_string();
    let _ = record_reflection_outcome_start(
        stores,
        bear_id,
        &run.id.to_string(),
        &run.lane,
        &run.trigger,
        Some(input_summary.as_str()),
    )
    .await;

    let proposal_ids = proposal_ids_from_summary(&run.input_summary);
    let output = match execute_memory_curate_run(
        pool,
        config,
        stores,
        run.id,
        run.bear_id,
        run.trigger.as_str(),
        &proposal_ids,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            let _ = record_reflection_outcome_complete(
                stores,
                bear_id,
                &run.id.to_string(),
                "failed",
                Some(error.to_string().as_str()),
                &proposal_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>(),
            )
            .await;
            let failed_run =
                mark_memory_curate_failed(pool, run.bear_id, run.id, &error.to_string()).await?;
            return Ok(Some(failed_run));
        }
    };

    let summary = memory_curate_output_summary(&output).to_string();
    let _ = record_reflection_outcome_complete(
        stores,
        bear_id,
        &run.id.to_string(),
        "completed",
        Some(summary.as_str()),
        &proposal_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>(),
    )
    .await;

    if !output.briefing.is_empty() {
        maybe_run_native_curate_briefing_turn(pool, config, stores, bear_id, &run, &output).await;
    }

    let completion = curate_retry::complete_and_schedule(
        pool,
        &run,
        memory_curate_output_summary(&output),
        &output.outcomes,
    )
    .await?;
    project_memory_curate_completed(
        pool,
        &completion.completed,
        proposal_ids_from_summary(&completion.completed.input_summary),
    );
    if let Some(retry) = completion.retry_run {
        project_memory_curate_enqueued(
            pool,
            &retry,
            proposal_ids_from_summary(&retry.input_summary),
        );
    }
    Ok(Some(completion.completed))
}

async fn execute_memory_curate_run(
    pool: &PgPool,
    config: &Config,
    _stores: &MemoryStoreManager,
    reflection_run_id: Uuid,
    bear_id: Uuid,
    trigger: &str,
    proposal_ids: &[Uuid],
) -> Result<MemoryCurateRunOutput, DenError> {
    let output = memory_curate_executor::execute_memory_curate_proposals(
        pool,
        config,
        _stores,
        bear_id,
        Some(trigger),
        proposal_ids,
    )
    .await?;
    for outcome in &output.outcomes {
        record_memory_curate_run_item(
            pool,
            reflection_run_id,
            outcome.proposal_id,
            &outcome.status,
        )
        .await?;
    }
    Ok(output)
}

async fn record_memory_curate_run_item(
    pool: &PgPool,
    run_id: Uuid,
    proposal_id: Uuid,
    status: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO bear_reflection_run_items (run_id, item_kind, item_id, status)
        VALUES ($1, 'memory_proposal', $2, $3)
        ",
        run_id,
        proposal_id.to_string(),
        status
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn memory_curate_output_summary(output: &MemoryCurateRunOutput) -> serde_json::Value {
    serde_json::json!({
        "resolved_proposal_ids": output.resolved_proposal_ids,
        "resolution_status": output.resolution_status,
        "status_counts": output.status_counts,
        "outcomes": output.outcomes,
        "briefing": output.briefing,
    })
}

fn native_curate_llm_briefing_enabled() -> bool {
    // Cached after first read: this is process-startup config, not a hot toggle.
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("NATIVE_CURATE_LLM_BRIEFING") {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        Err(_) => true,
    })
}

async fn maybe_run_native_curate_briefing_turn(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
    run: &ReflectionRunRow,
    output: &MemoryCurateRunOutput,
) {
    if !native_curate_llm_briefing_enabled() || output.briefing.is_empty() {
        return;
    }
    let prompt = compose_curate_briefing_prompt(&output.briefing);
    let deps = NativeRuntimeDeps {
        pool,
        config,
        stores,
    };
    match run_native_curate_briefing_collect_assistant_text(
        &deps,
        bear_id,
        ReflectionRunId::new(run.id),
        &prompt,
    )
    .await
    {
        Ok(briefing) if briefing.text.trim().is_empty() => {}
        Ok(briefing) => {
            project_curate_briefing_to_conversation(pool, &briefing.source, &briefing.text);
        }
        Err(error) => {
            tracing::warn!(
                reflection_run_id = %run.id,
                bear_id = %bear_id,
                error = %error,
                "native curate briefing turn failed; rule-based outcomes retained"
            );
        }
    }
}

fn project_curate_briefing_to_conversation(
    pool: &PgPool,
    source: &CurateBriefingSource,
    text: &str,
) {
    let bear_id = source.bear_id().as_uuid();
    let conversation_id = source.conversation_id().as_str();
    let context = canonical_persistence_context(
        pool.clone(),
        bear_id,
        None,
        conversation_id.to_string(),
        None,
        None,
        format!("bear:{bear_id}:lane:memory_curate"),
        false,
    );
    spawn_persist_assistant_summary_message(
        context,
        text.to_string(),
        Some(format!("curate-briefing-{}", source.run_id().as_uuid())),
    );
}

pub async fn run_memory_curate_worker_loop(
    pool: PgPool,
    config: Arc<Config>,
    stores: MemoryStoreManager,
    worker_token: tokio_util::sync::CancellationToken,
    poll_interval: std::time::Duration,
) -> Result<(), DenError> {
    let mut next_recovery_scan = OffsetDateTime::now_utc();
    let mut recovery_offset = 0_i64;
    loop {
        tokio::select! {
            () = worker_token.cancelled() => {
                break;
            }
            () = tokio::time::sleep(poll_interval) => {}
        }

        if OffsetDateTime::now_utc() >= next_recovery_scan {
            match curate_retry::recover_exhausted_once(&pool, &stores, recovery_offset).await {
                Ok(progress) => {
                    recovery_offset = if progress.inspected == 100 {
                        recovery_offset + 100
                    } else {
                        0
                    };
                    next_recovery_scan = OffsetDateTime::now_utc() + time::Duration::days(1);
                    if progress.queued > 0 {
                        tracing::info!(
                            queued = progress.queued,
                            "recovered exhausted private Curate candidates for delayed retry"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "exhausted Curate candidate sweep failed; retrying later");
                    next_recovery_scan = OffsetDateTime::now_utc() + time::Duration::minutes(10);
                }
            }
        }
        let bear_ids = list_bears_with_queued_memory_curate_runs(&pool).await?;
        for bear_id in bear_ids {
            if worker_token.is_cancelled() {
                break;
            }
            if let Some(run) =
                run_next_memory_curate_once(&pool, config.as_ref(), &stores, bear_id).await?
            {
                tracing::info!(
                    bear_id = %bear_id,
                    reflection_run_id = %run.id,
                    status = %run.status,
                    "memory_curate worker processed queued run"
                );
            }
        }
    }
    Ok(())
}

async fn list_bears_with_queued_memory_curate_runs(pool: &PgPool) -> Result<Vec<Uuid>, DenError> {
    let rows = sqlx::query_scalar!(
        r"
        SELECT DISTINCT bear_id
        FROM bear_reflection_runs
        WHERE lane = 'memory_curate'
          AND status = 'queued'
          AND available_at <= NOW()
        ORDER BY bear_id
        "
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn proposal_ids_from_summary(summary: &serde_json::Value) -> Vec<Uuid> {
    summary
        .get("proposal_ids")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .filter_map(|value| Uuid::parse_str(value).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// recall_index lane (ADR-0038 Phase 1b): derived recall reconcile jobs.
// Mirrors the memory_curate queue (FOR UPDATE SKIP LOCKED) without the
// conversation-projection machinery — recall indexing is invisible plumbing.
// ---------------------------------------------------------------------------

/// Enqueue a derived-recall reconcile for a Bear. **Coalesces**: if a `recall_index` run is
/// already queued for the Bear, returns `None` rather than piling up duplicate work.
pub async fn enqueue_recall_index(
    pool: &PgPool,
    bear_id: Uuid,
    trigger: &str,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(ReflectionRunRow, r"
        INSERT INTO bear_reflection_runs (bear_id, lane, trigger, status, input_summary, output_summary)
        SELECT $1, 'recall_index', $2, 'queued', '{}'::jsonb, '{}'::jsonb
        WHERE NOT EXISTS (
            SELECT 1 FROM bear_reflection_runs
            WHERE bear_id = $1 AND lane = 'recall_index' AND status = 'queued'
        )
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ", bear_id, trigger)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Best-effort recall enqueue for the memory write path: a no-op when recall is disabled
/// (`QDRANT_URL` unset) and a logged warning on failure — it must never fail the caller's
/// tool/turn. Coalescing is handled by [`enqueue_recall_index`].
pub async fn enqueue_recall_index_if_enabled(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    trigger: &str,
) {
    if config.qdrant_url.is_none() {
        return;
    }
    if let Err(error) = enqueue_recall_index(pool, bear_id, trigger).await {
        tracing::warn!(
            bear_id = %bear_id,
            trigger,
            error = %error,
            "failed to enqueue recall_index"
        );
    }
}

async fn claim_next_recall_index_run(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        WITH next_run AS (
            SELECT id
            FROM bear_reflection_runs
            WHERE bear_id = $1 AND lane = 'recall_index' AND status = 'queued'
            ORDER BY created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE bear_reflection_runs runs
        SET status = 'running', started_at = COALESCE(started_at, NOW())
        FROM next_run
        WHERE runs.id = next_run.id
        RETURNING runs.id, runs.bear_id, runs.lane, runs.trigger, runs.status,
                  runs.role_agent_id, runs.conversation_id, runs.conversation_key,
                  runs.conversation_date, runs.input_summary, runs.output_summary,
                  runs.error, runs.started_at, runs.completed_at, runs.created_at
        ",
        bear_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn mark_recall_index_completed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    output_summary: serde_json::Value,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'completed', output_summary = $3, error = NULL, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'recall_index'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        output_summary,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn mark_recall_index_failed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    error: &str,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'failed', error = $3, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'recall_index'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        error,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_bears_with_queued_recall_index_runs(pool: &PgPool) -> Result<Vec<Uuid>, DenError> {
    let rows = sqlx::query_scalar!(
        r"
        SELECT DISTINCT bear_id
        FROM bear_reflection_runs
        WHERE lane = 'recall_index' AND status = 'queued'
        ORDER BY bear_id
        "
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Claim and run the next queued `recall_index` reconcile for a Bear. Recall is optional and
/// derived: when Qdrant is unconfigured the run is marked completed (skipped) so the queue
/// drains instead of piling up.
pub async fn run_next_recall_index_once(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let Some(run) = claim_next_recall_index_run(pool, bear_id).await? else {
        return Ok(None);
    };

    let Some(qdrant) = QdrantRecall::from_config(config) else {
        let completed = mark_recall_index_completed(
            pool,
            bear_id,
            run.id,
            serde_json::json!({ "skipped": "recall disabled (QDRANT_URL unset)" }),
        )
        .await?;
        return Ok(Some(completed));
    };

    let embedder =
        match den_service::recall::authenticated_embedder(pool, config, BearId::new(bear_id)).await
        {
            Ok(Some(embedder)) => embedder,
            Ok(None) => {
                let completed = mark_recall_index_completed(
                    pool,
                    bear_id,
                    run.id,
                    serde_json::json!({ "skipped": "embeddings API is not configured" }),
                )
                .await?;
                return Ok(Some(completed));
            }
            Err(error) => {
                let failed =
                    mark_recall_index_failed(pool, bear_id, run.id, &error.to_string()).await?;
                return Ok(Some(failed));
            }
        };

    let store = match stores.store_for_bear(bear_id).await {
        Ok(store) => store,
        Err(error) => {
            let failed =
                mark_recall_index_failed(pool, bear_id, run.id, &error.to_string()).await?;
            return Ok(Some(failed));
        }
    };

    match reconcile_bear(pool, &qdrant, &embedder, &store, &config.embedding_standard).await {
        Ok(outcome) => {
            let summary = serde_json::json!({
                "indexed_records": outcome.indexed_records,
                "embedded_chunks": outcome.embedded_chunks,
                "reused_chunks": outcome.reused_chunks,
                "removed_records": outcome.removed_records,
                "removed_points": outcome.removed_points,
            });
            let completed = mark_recall_index_completed(pool, bear_id, run.id, summary).await?;
            Ok(Some(completed))
        }
        Err(error) => {
            let failed =
                mark_recall_index_failed(pool, bear_id, run.id, &error.to_string()).await?;
            Ok(Some(failed))
        }
    }
}

/// Poll loop for the `recall_index` lane. A single Bear's failure is logged and skipped so a
/// transient embed/Qdrant error never tears down the worker.
pub async fn run_recall_index_worker_loop(
    pool: PgPool,
    config: Arc<Config>,
    stores: MemoryStoreManager,
    worker_token: tokio_util::sync::CancellationToken,
    poll_interval: std::time::Duration,
) -> Result<(), DenError> {
    loop {
        tokio::select! {
            () = worker_token.cancelled() => { break; }
            () = tokio::time::sleep(poll_interval) => {}
        }

        let bear_ids = list_bears_with_queued_recall_index_runs(&pool).await?;
        for bear_id in bear_ids {
            if worker_token.is_cancelled() {
                break;
            }
            match run_next_recall_index_once(&pool, config.as_ref(), &stores, bear_id).await {
                Ok(Some(run)) => tracing::info!(
                    bear_id = %bear_id,
                    reflection_run_id = %run.id,
                    status = %run.status,
                    output = %run.output_summary,
                    "recall_index worker processed queued run"
                ),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    bear_id = %bear_id,
                    error = %error,
                    "recall_index worker run failed; continuing"
                ),
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// context_compact lane: async post-turn compaction WRITE jobs.
// Mirrors recall_index queue semantics (FOR UPDATE SKIP LOCKED, coalesced enqueue).
// ---------------------------------------------------------------------------

async fn claim_next_context_compact_run(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        WITH next_run AS (
            SELECT id
            FROM bear_reflection_runs
            WHERE bear_id = $1 AND lane = 'context_compact' AND status = 'queued'
            ORDER BY created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE bear_reflection_runs runs
        SET status = 'running', started_at = COALESCE(started_at, NOW())
        FROM next_run
        WHERE runs.id = next_run.id
        RETURNING runs.id, runs.bear_id, runs.lane, runs.trigger, runs.status,
                  runs.role_agent_id, runs.conversation_id, runs.conversation_key,
                  runs.conversation_date, runs.input_summary, runs.output_summary,
                  runs.error, runs.started_at, runs.completed_at, runs.created_at
        ",
        bear_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn mark_context_compact_completed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    output_summary: serde_json::Value,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'completed', output_summary = $3, error = NULL, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'context_compact'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        output_summary,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn mark_context_compact_failed(
    pool: &PgPool,
    bear_id: Uuid,
    run_id: Uuid,
    error: &str,
) -> Result<ReflectionRunRow, DenError> {
    let row = sqlx::query_as!(
        ReflectionRunRow,
        r"
        UPDATE bear_reflection_runs
        SET status = 'failed', error = $3, completed_at = NOW()
        WHERE bear_id = $1 AND id = $2 AND lane = 'context_compact'
        RETURNING id, bear_id, lane, trigger, status, role_agent_id,
                  conversation_id, conversation_key, conversation_date,
                  input_summary, output_summary, error,
                  started_at, completed_at, created_at
        ",
        bear_id,
        run_id,
        error,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_bears_with_queued_context_compact_runs(pool: &PgPool) -> Result<Vec<Uuid>, DenError> {
    let rows = sqlx::query_scalar!(
        r"
        SELECT DISTINCT bear_id
        FROM bear_reflection_runs
        WHERE lane = 'context_compact' AND status = 'queued'
        ORDER BY bear_id
        "
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod compaction_tests;

fn parse_context_compact_input(run: &ReflectionRunRow) -> Result<String, String> {
    let conversation_id = run
        .input_summary
        .get("conversation_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| run.conversation_id.clone())
        .ok_or_else(|| "context_compact missing conversation_id".to_string())?;
    Ok(conversation_id)
}

fn context_compact_output_summary(state: &TurnCompactionState) -> serde_json::Value {
    serde_json::json!({
        "event": state.event,
        "decision": state.decision,
        "compacted_seq_cutoff": state.compacted_seq_cutoff,
    })
}

/// Claim and run the next queued `context_compact` job for a Bear.
pub async fn run_next_context_compact_once(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
) -> Result<Option<ReflectionRunRow>, DenError> {
    let Some(run) = claim_next_context_compact_run(pool, bear_id).await? else {
        return Ok(None);
    };

    let conversation_id = match parse_context_compact_input(&run) {
        Ok(parsed) => parsed,
        Err(error) => {
            let failed = mark_context_compact_failed(pool, bear_id, run.id, &error).await?;
            return Ok(Some(failed));
        }
    };

    match run_compaction_job(
        pool,
        config,
        bear_id,
        &conversation_id,
        crate::runtime_compaction::CompactionSource::ContextMaintenance,
        TurnCompactionTrigger::PostTurn,
    )
    .await
    {
        Ok(Some(state)) => {
            let completed = mark_context_compact_completed(
                pool,
                bear_id,
                run.id,
                context_compact_output_summary(&state),
            )
            .await?;
            Ok(Some(completed))
        }
        Ok(None) => {
            let completed = mark_context_compact_completed(
                pool,
                bear_id,
                run.id,
                serde_json::json!({ "skipped": "compaction disabled" }),
            )
            .await?;
            Ok(Some(completed))
        }
        Err(error) => {
            let failed =
                mark_context_compact_failed(pool, bear_id, run.id, &error.to_string()).await?;
            Ok(Some(failed))
        }
    }
}

/// Poll loop for the `context_compact` lane. Enabled whenever workers run (no Qdrant gate).
pub async fn run_context_compact_worker_loop(
    pool: PgPool,
    config: Arc<Config>,
    worker_token: tokio_util::sync::CancellationToken,
    poll_interval: std::time::Duration,
) -> Result<(), DenError> {
    loop {
        tokio::select! {
            () = worker_token.cancelled() => { break; }
            () = tokio::time::sleep(poll_interval) => {}
        }

        let bear_ids = list_bears_with_queued_context_compact_runs(&pool).await?;
        for bear_id in bear_ids {
            if worker_token.is_cancelled() {
                break;
            }
            match run_next_context_compact_once(&pool, config.as_ref(), bear_id).await {
                Ok(Some(run)) => tracing::info!(
                    bear_id = %bear_id,
                    reflection_run_id = %run.id,
                    status = %run.status,
                    output = %run.output_summary,
                    "context_compact worker processed queued run"
                ),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    bear_id = %bear_id,
                    error = %error,
                    "context_compact worker run failed; continuing"
                ),
            }
        }
    }
    Ok(())
}
