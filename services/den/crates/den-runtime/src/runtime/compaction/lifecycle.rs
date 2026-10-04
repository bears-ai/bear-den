//! Source-selected turn compaction lifecycle (READ / WRITE / ENQUEUE).
//!
//! Ordinary turns and dedicated maintenance share three hooks:
//! - [`on_turn_assemble_compaction`] — sync READ (and optional sync WRITE)
//! - [`enqueue_compaction_after_turn`] — async WRITE scheduling
//! - [`run_compaction_job`] — evaluate + persist (worker, manual, emergency)

use den_core::{config::Config, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    agent_loop::load_transcript_grouping_rows,
    reflection::conductor::enqueue_archive_harvest_for_bear,
    runtime_compaction_observability::{
        build_compaction_applied_event, build_compaction_skipped_event, RuntimeCompactionEvent,
        RuntimeCompactionEventStatus,
    },
    runtime_compaction_store::{list_runtime_compaction_events, record_runtime_compaction_event},
    runtime_conversations::{RuntimeCompactionTriggerKind, RuntimeSemanticGroup},
};

use super::{
    artifact_store,
    policy::{compaction_policy_for_source, CompactionMode, CompactionSource, CompactionTiming},
    render::render_compacted_context_block,
    summarize::summarize_compacted_groups,
    {
        artifact_ref_from_decision, choose_compaction_decision, RuntimeCompactionDecision,
        RuntimeCompactionPolicy,
    },
};

/// When compaction evaluation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCompactionTrigger {
    /// Turn assembly (sync timing only) or background job default.
    TurnStart,
    /// After a user-visible turn completes (async worker).
    PostTurn,
    /// Conversation review checkpoint. It may create an iterative summary artifact below normal
    /// prompt-pressure thresholds so memory discovery is not starved by bucket size.
    ConversationReview,
    /// Operator or API request.
    Manual,
    /// Context overflow recovery (sync emergency).
    ModelSafetyMargin,
}

impl TurnCompactionTrigger {
    pub fn as_runtime_trigger(self) -> RuntimeCompactionTriggerKind {
        match self {
            Self::TurnStart | Self::PostTurn => RuntimeCompactionTriggerKind::SemanticGroupCount,
            Self::ConversationReview => RuntimeCompactionTriggerKind::ConversationReview,
            Self::Manual => RuntimeCompactionTriggerKind::Manual,
            Self::ModelSafetyMargin => RuntimeCompactionTriggerKind::ModelSafetyMargin,
        }
    }
}

/// Compaction state consumed by turn assembly.
#[derive(Debug, Clone)]
pub struct TurnCompactionState {
    pub event: RuntimeCompactionEvent,
    pub decision: Option<RuntimeCompactionDecision>,
    pub policy: RuntimeCompactionPolicy,
    pub groups: Vec<RuntimeSemanticGroup>,
    pub compacted_seq_cutoff: Option<i64>,
    pub compacted_context: Option<String>,
}

/// **READ hook** — load persisted artifact for prompt assembly (no policy evaluation).
pub async fn load_compaction_context(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    source: CompactionSource,
) -> Result<Option<TurnCompactionState>, DenError> {
    let policy = compaction_policy_for_source(source)?;
    let artifact =
        artifact_store::load_latest_iterative_summary(pool, bear_id, conversation_id).await?;
    let compacted_context = artifact
        .as_ref()
        .map(|record| render_compacted_context_block(&record.summary));
    let compacted_seq_cutoff = artifact.map(|record| record.source_message_end_seq);
    let event = latest_compaction_event_or_placeholder(pool, conversation_id, &policy).await?;
    Ok(Some(TurnCompactionState {
        event,
        decision: None,
        policy,
        groups: Vec::new(),
        compacted_seq_cutoff,
        compacted_context,
    }))
}

/// **ASSEMBLE hook** — READ for async timing; READ+WRITE for sync timing.
pub async fn on_turn_assemble_compaction(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    conversation_id: &str,
    source: CompactionSource,
) -> Result<Option<TurnCompactionState>, DenError> {
    compaction_policy_for_source(source)?;
    if CompactionMode::parse(&config.compaction_mode) == CompactionMode::Off {
        return Ok(None);
    }
    if CompactionTiming::parse(&config.compaction_timing) == CompactionTiming::Sync {
        run_compaction_job(
            pool,
            config,
            bear_id,
            conversation_id,
            source,
            TurnCompactionTrigger::TurnStart,
        )
        .await
    } else {
        load_compaction_context(pool, bear_id, conversation_id, source).await
    }
}

/// **WRITE hook** — evaluate policy, persist artifacts/events.
pub async fn run_compaction_job(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    conversation_id: &str,
    source: CompactionSource,
    trigger: TurnCompactionTrigger,
) -> Result<Option<TurnCompactionState>, DenError> {
    let policy = compaction_policy_for_source(source)?;
    let mode = CompactionMode::parse(&config.compaction_mode);
    if mode == CompactionMode::Off {
        return Ok(None);
    }

    let runtime_trigger = trigger.as_runtime_trigger();
    let persist_artifact = should_persist_compaction_artifact(mode, &runtime_trigger);
    let prior_summary = if persist_artifact {
        artifact_store::load_latest_iterative_summary(pool, bear_id, conversation_id).await?
    } else {
        None
    };
    let prior_cutoff = prior_summary
        .as_ref()
        .map(|record| record.source_message_end_seq)
        .filter(|_| {
            !matches!(
                runtime_trigger,
                RuntimeCompactionTriggerKind::Manual
                    | RuntimeCompactionTriggerKind::ModelSafetyMargin
            )
        });
    let all_rows = load_transcript_grouping_rows(pool, bear_id, conversation_id).await?;
    let rows = if let Some(cutoff) = prior_cutoff {
        // ponytail: sequence cutoffs skip already-summarized persisted rows for normal/live
        // compaction. The ceiling is out-of-order or sequence-less transcript imports; upgrade
        // path is explicit per-message compaction watermarks.
        all_rows
            .into_iter()
            .filter(|row| row.sequence_no.is_some_and(|seq| seq > cutoff))
            .collect::<Vec<_>>()
    } else {
        all_rows
    };
    let groups = super::semantic_groups_from_conversation_messages(&rows);

    let mut decision = choose_compaction_decision(&groups, runtime_trigger.clone(), &policy);
    if decision.is_none() && mode == CompactionMode::Active {
        let transcript_chars = estimate_transcript_chars(&rows);
        if transcript_chars > policy.max_transcript_chars {
            decision = choose_compaction_decision(
                &groups,
                RuntimeCompactionTriggerKind::TokenPressure,
                &policy,
            );
        }
    }

    let (event, compacted_seq_cutoff, compacted_context) = match &decision {
        Some(decision) => {
            let artifact_id = format!(
                "{conversation_id}:{}-{}",
                decision.selected_group_start, decision.selected_group_end
            );
            let artifact_ref = artifact_ref_from_decision(artifact_id, decision, &policy);
            let event = build_compaction_applied_event(
                conversation_id.to_string(),
                decision,
                &policy,
                artifact_ref,
            );

            let (cutoff, context) = if persist_artifact {
                let compacted_groups =
                    &groups[decision.selected_group_start..=decision.selected_group_end];
                let summary = summarize_compacted_groups(
                    prior_summary.as_ref().map(|record| &record.summary),
                    compacted_groups,
                    &rows,
                    decision,
                );
                let (start_seq, end_seq) = sequence_span_for_group_range(&rows, &groups, decision)?;
                let _artifact_uuid = artifact_store::insert_iterative_summary_artifact(
                    pool,
                    bear_id,
                    conversation_id,
                    decision,
                    &policy,
                    runtime_trigger.clone(),
                    start_seq,
                    end_seq,
                    &summary,
                )
                .await?;
                if let Err(error) =
                    enqueue_archive_harvest_for_bear(pool, bear_id, "compaction_artifact_created")
                        .await
                {
                    tracing::warn!(
                        bear_id = %bear_id,
                        conversation_id,
                        error = %error,
                        "failed to enqueue archive_harvest after compaction artifact creation"
                    );
                }
                let cutoff = Some(end_seq);
                let context = Some(render_compacted_context_block(&summary));
                (cutoff, context)
            } else {
                (None, None)
            };

            (event, cutoff, context)
        }
        None => {
            let diagnostic = if groups.is_empty() {
                if prior_cutoff.is_some() {
                    "no uncompacted transcript groups to evaluate"
                } else {
                    "no transcript groups to evaluate"
                }
            } else if matches!(
                runtime_trigger,
                RuntimeCompactionTriggerKind::ConversationReview
            ) {
                "conversation review skipped; no eligible groups outside protected floor"
            } else {
                "no eligible history groups outside protected floors"
            };
            let event = build_compaction_skipped_event(
                conversation_id.to_string(),
                runtime_trigger,
                &policy,
                diagnostic,
            );
            let context = prior_summary
                .as_ref()
                .map(|record| render_compacted_context_block(&record.summary));
            (
                event,
                prior_summary.map(|record| record.source_message_end_seq),
                context,
            )
        }
    };

    record_runtime_compaction_event(pool, &event).await?;

    Ok(Some(TurnCompactionState {
        event,
        decision,
        policy,
        groups,
        compacted_seq_cutoff,
        compacted_context,
    }))
}

/// **ENQUEUE hook** — schedule async WRITE after a user-visible turn (best-effort).
pub async fn enqueue_compaction_after_turn(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    conversation_id: &str,
    source: CompactionSource,
) {
    if let Err(error) = compaction_policy_for_source(source) {
        tracing::warn!(?source, %error, "refusing invalid turn compaction source");
        return;
    }
    if CompactionMode::parse(&config.compaction_mode) == CompactionMode::Off {
        return;
    }
    if CompactionTiming::parse(&config.compaction_timing) != CompactionTiming::Async {
        return;
    }
    if !config.run_workers {
        return;
    }
    // Retain compatibility metadata for existing audit consumers only. The worker
    // selects ContextMaintenance from its claimed lane, never from this payload.
    let profile = match source {
        CompactionSource::Turn(origin) => Some(
            den_core::EffectivePolicy::compile_for_origin(
                origin,
                den_core::Governance::Interactive,
            )
            .context_label
            .as_str(),
        ),
        CompactionSource::ContextMaintenance => None,
    };
    let input_summary = serde_json::json!({
        "conversation_id": conversation_id,
        "profile": profile,
        "trigger": "post_turn",
    });
    let result = sqlx::query!(
        r"
        INSERT INTO bear_reflection_runs (
            bear_id, lane, trigger, status, input_summary, output_summary, conversation_id
        )
        SELECT $1, 'context_compact', 'post_turn', 'queued', $2, '{}'::jsonb, $3
        WHERE NOT EXISTS (
            SELECT 1 FROM bear_reflection_runs
            WHERE bear_id = $1
              AND lane = 'context_compact'
              AND conversation_id = $3
              AND status = 'queued'
        )
        ",
        bear_id,
        input_summary,
        conversation_id
    )
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(
            bear_id = %bear_id,
            conversation_id,
            source = ?source,
            error = %error,
            "failed to enqueue context_compact"
        );
    }
}

/// Back-compat alias for turn-start assembly (uses [`on_turn_assemble_compaction`]).
pub async fn prepare_turn_compaction(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    conversation_id: &str,
    source: CompactionSource,
    trigger: TurnCompactionTrigger,
) -> Result<Option<TurnCompactionState>, DenError> {
    match trigger {
        TurnCompactionTrigger::TurnStart => {
            on_turn_assemble_compaction(pool, config, bear_id, conversation_id, source).await
        }
        TurnCompactionTrigger::PostTurn
        | TurnCompactionTrigger::ConversationReview
        | TurnCompactionTrigger::Manual
        | TurnCompactionTrigger::ModelSafetyMargin => {
            run_compaction_job(pool, config, bear_id, conversation_id, source, trigger).await
        }
    }
}

pub fn render_compaction_prompt_context(state: &TurnCompactionState) -> String {
    let compacted = state.compacted_context.as_deref().unwrap_or_default();
    let recent_count = state
        .policy
        .protected_recent_group_count
        .min(state.groups.len());
    let status = match state.event.status {
        RuntimeCompactionEventStatus::Applied => "applied",
        RuntimeCompactionEventStatus::Skipped => "skipped",
        RuntimeCompactionEventStatus::Failed => "failed",
    };

    let mut parts = vec![format!(
        "Runtime compaction context is Den-owned. Treat active instructions, workflow state, recent uncompacted groups, and compacted summary state as distinct context layers. Current compaction evaluation: status={status} policy_version={} source_range={:?}-{:?} diagnostic={}. Recent protected groups retained: {recent_count}.",
        state.policy.policy_version,
        state.event.source_group_start,
        state.event.source_group_end,
        state.event.diagnostic.as_deref().unwrap_or("none"),
    )];

    if !compacted.is_empty() {
        parts.push(compacted.to_string());
    }

    parts.join("\n\n")
}

async fn latest_compaction_event_or_placeholder(
    pool: &PgPool,
    conversation_id: &str,
    policy: &RuntimeCompactionPolicy,
) -> Result<RuntimeCompactionEvent, DenError> {
    let rows = list_runtime_compaction_events(pool, conversation_id, 1).await?;
    if let Some(row) = rows.first() {
        return Ok(RuntimeCompactionEvent {
            conversation_id: conversation_id.to_string(),
            trigger: RuntimeCompactionTriggerKind::SemanticGroupCount,
            policy_version: row.policy_version.clone(),
            status: match row.status.as_str() {
                "Applied" => RuntimeCompactionEventStatus::Applied,
                "Failed" => RuntimeCompactionEventStatus::Failed,
                _ => RuntimeCompactionEventStatus::Skipped,
            },
            boundary: None,
            source_group_start: row.source_group_start,
            source_group_end: row.source_group_end,
            artifact: None,
            diagnostic: row.diagnostic.clone(),
        });
    }
    Ok(build_compaction_skipped_event(
        conversation_id.to_string(),
        RuntimeCompactionTriggerKind::SemanticGroupCount,
        policy,
        "no prior compaction event",
    ))
}

fn should_persist_compaction_artifact(
    mode: CompactionMode,
    trigger: &RuntimeCompactionTriggerKind,
) -> bool {
    mode == CompactionMode::Active
        || matches!(trigger, RuntimeCompactionTriggerKind::ConversationReview)
}

fn estimate_transcript_chars(rows: &[super::TranscriptGroupingRow]) -> usize {
    rows.iter()
        .map(|row| row.content_text.chars().count())
        .sum()
}

fn sequence_span_for_group_range(
    rows: &[super::TranscriptGroupingRow],
    groups: &[RuntimeSemanticGroup],
    decision: &RuntimeCompactionDecision,
) -> Result<(i64, i64), DenError> {
    let start_group = groups
        .get(decision.selected_group_start)
        .ok_or_else(|| DenError::System("compaction group start out of range".into()))?;
    let end_group = groups
        .get(decision.selected_group_end)
        .ok_or_else(|| DenError::System("compaction group end out of range".into()))?;

    let start_seq = message_sequence(rows, start_group.start_message_id.as_deref())
        .ok_or_else(|| DenError::System("compaction start message missing sequence".into()))?;
    let end_seq = message_sequence(rows, end_group.end_message_id.as_deref())
        .or_else(|| message_sequence(rows, end_group.start_message_id.as_deref()))
        .ok_or_else(|| DenError::System("compaction end message missing sequence".into()))?;
    Ok((start_seq, end_seq))
}

fn message_sequence(
    rows: &[super::TranscriptGroupingRow],
    message_id: Option<&str>,
) -> Option<i64> {
    let message_id = message_id?;
    rows.iter()
        .find(|row| row.message_id.as_deref() == Some(message_id))
        .and_then(|row| row.sequence_no)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_conversations::RuntimeSemanticGroupKind;

    #[test]
    fn compaction_timing_defaults_to_async() {
        assert_eq!(CompactionTiming::parse("async"), CompactionTiming::Async);
        assert_eq!(CompactionTiming::parse(""), CompactionTiming::Async);
        assert_eq!(CompactionTiming::parse("sync"), CompactionTiming::Sync);
    }

    #[test]
    fn conversation_review_persists_artifact_even_in_observe_mode() {
        assert!(should_persist_compaction_artifact(
            CompactionMode::Observe,
            &RuntimeCompactionTriggerKind::ConversationReview
        ));
        assert!(!should_persist_compaction_artifact(
            CompactionMode::Observe,
            &RuntimeCompactionTriggerKind::Manual
        ));
        assert!(should_persist_compaction_artifact(
            CompactionMode::Active,
            &RuntimeCompactionTriggerKind::Manual
        ));
    }

    #[test]
    fn render_compaction_prompt_context_includes_status_and_summary() {
        let state = TurnCompactionState {
            event: build_compaction_skipped_event(
                "conv-1",
                RuntimeCompactionTriggerKind::SemanticGroupCount,
                &compaction_policy_for_source(CompactionSource::Turn(
                    den_core::TurnExecutionOrigin::AuthorizedWorkRun(
                        den_core::ArmatureAvailability::Absent,
                    ),
                ))
                .unwrap(),
                "test",
            ),
            decision: None,
            policy: compaction_policy_for_source(CompactionSource::Turn(
                den_core::TurnExecutionOrigin::AuthorizedWorkRun(
                    den_core::ArmatureAvailability::Absent,
                ),
            ))
            .unwrap(),
            groups: vec![RuntimeSemanticGroup {
                kind: RuntimeSemanticGroupKind::UserTurn,
                start_message_id: None,
                end_message_id: None,
                message_count: 1,
                protected: false,
            }],
            compacted_seq_cutoff: None,
            compacted_context: Some("# Compacted context\n\n- goal: ship".into()),
        };
        let text = render_compaction_prompt_context(&state);
        assert!(text.contains("status=skipped"));
        assert!(text.contains("# Compacted context"));
    }
}
