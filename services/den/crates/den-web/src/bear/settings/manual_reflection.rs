//! Stage-aware manual maintenance. Diagnostics go to BearWire, never transcript
//! rows; already applied compaction/proposals remain visible if a later stage fails.
use super::{pretty_json, ManualReflectionResult};
use crate::{errors::CustomError, web::AppState};
use bearwire_protocol::wire::BearWireEvent;
use den_runtime::{
    bearwire_events,
    pair_reflection::create_pair_reflection_proposals_from_latest_summary,
    runtime::compaction::{prepare_turn_compaction, CompactionSource, TurnCompactionTrigger},
    runtime::compaction_observability::RuntimeCompactionEventStatus,
};
use den_service::conversation::persistence as conversation_persistence;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Compaction,
    Extraction,
    EventPersistence,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Compaction => "Checkpoint creation",
            Self::Extraction => "Memory extraction",
            Self::EventPersistence => "Reflection event recording",
        }
    }
}

pub(super) fn summary(result: &ManualReflectionResult) -> String {
    let checkpoint = if result.compaction_applied {
        "created"
    } else if result.compaction_skipped {
        "skipped"
    } else {
        result.compaction_status.as_str()
    };
    let proposals = if result.proposals_complete {
        format!("{} proposal(s) created", result.proposals_created)
    } else {
        "proposal creation did not finish; inspect Memory for any partial writes before retrying"
            .into()
    };
    // Redirect feedback is safe to put in a URL. Detailed provider/database
    // diagnostics stay in the authorized BearWire inspection projection.
    let outcome = match result.failed_stage {
        Some(stage) => format!(
            "{} failed; inspect the recorded evidence and retry",
            stage.label()
        ),
        None if result.error.is_some() => "failed; inspect the recorded evidence and retry".into(),
        None => result.skipped_reason.unwrap_or("processed").to_string(),
    };
    let recording = result.observability_error.as_deref().unwrap_or("");
    format!("Checkpoint {checkpoint}; {proposals}; reflection: {outcome}. {recording}")
}

fn fail(result: &mut ManualReflectionResult, stage: Stage, error: impl std::fmt::Display) {
    result.failed_stage = Some(stage);
    result.error = Some(format!(
        "{} failed: {error}. Inspect the recorded evidence and retry from this conversation.",
        stage.label()
    ));
    result.needs_attention = true;
}

pub(super) async fn run(
    state: &AppState,
    user_id: Option<i32>,
    bear: &den_service::bears::Bear,
    conv: &conversation_persistence::ConversationRecord,
    trigger: &str,
) -> Result<ManualReflectionResult, CustomError> {
    let mut result = ManualReflectionResult {
        compaction_status: "Not run".into(),
        ..Default::default()
    };
    let external_id = conv
        .external_conversation_id
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    let session_id = conv
        .source_client_session_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .or(external_id)
        .map(str::to_owned)
        .unwrap_or_else(|| conv.id.to_string());
    if let Some(external_id) = external_id {
        match prepare_turn_compaction(
            state.sqlx_pool(),
            &state.config,
            bear.id,
            external_id,
            CompactionSource::ContextMaintenance,
            TurnCompactionTrigger::ConversationReview,
        )
        .await
        {
            Ok(compaction) => {
                if let Some(compaction) = compaction {
                    result.compaction_applied = matches!(
                        compaction.event.status,
                        RuntimeCompactionEventStatus::Applied
                    );
                    result.compaction_skipped = matches!(
                        compaction.event.status,
                        RuntimeCompactionEventStatus::Skipped
                    );
                    result.compaction_status = compaction.event.status.as_str().into();
                    result
                        .compaction_diagnostic
                        .clone_from(&compaction.event.diagnostic);
                    result.compaction_artifact_json = compaction
                        .event
                        .artifact
                        .as_ref()
                        .and_then(|artifact| serde_json::to_string_pretty(artifact).ok())
                        .unwrap_or_default();
                    if matches!(
                        compaction.event.status,
                        RuntimeCompactionEventStatus::Failed
                    ) {
                        fail(
                            &mut result,
                            Stage::Compaction,
                            compaction
                                .event
                                .diagnostic
                                .as_deref()
                                .unwrap_or("No diagnostic recorded"),
                        );
                    }
                }
            }
            Err(error) => {
                result.compaction_status = "Failed".into();
                result.compaction_diagnostic = Some(error.to_string());
                fail(&mut result, Stage::Compaction, error);
            }
        }
        if result.error.is_none() {
            let memory_stores = state.memory_stores.clone();
            match create_pair_reflection_proposals_from_latest_summary(
                state.sqlx_pool(),
                &state.config,
                &memory_stores,
                bear.id,
                external_id,
                &session_id,
            )
            .await
            {
                Ok(output) => {
                    result.candidate_count = output.candidate_count;
                    result.discarded_count = output.discarded_count;
                    result.discarded_reasons = output.discarded_reasons;
                    result.dropped_followup_count = output.dropped_followup_count;
                    result.proposals_created = output.created_proposal_ids.len();
                    result.proposal_ids = output.created_proposal_ids;
                    result.proposals_complete = true;
                    result.skipped_reason = output.skipped_reason;
                    result.needs_attention = super::super::memory::inspection::reflection_feedback(
                        Some(if result.skipped_reason.is_some() {
                            "skipped"
                        } else {
                            "processed"
                        }),
                        result.skipped_reason,
                        None,
                    )
                    .needs_attention;
                    result.source_message_start_seq = output.source_message_start_seq;
                    result.source_message_end_seq = output.source_message_end_seq;
                }
                Err(error) => fail(&mut result, Stage::Extraction, error),
            }
        }
    } else {
        result.compaction_status = "Failed".into();
        fail(
            &mut result,
            Stage::Compaction,
            "Conversation has no external ID",
        );
    }
    let recorded_payload = payload(&result, bear, conv.id, &session_id, trigger);
    match record(state, user_id, bear.id, &session_id, recorded_payload).await {
        Ok(event) => {
            result.reflection_event_id = Some(event.id);
            result.reflection_event_sequence_no = Some(event.sequence_no);
        }
        Err(error) => {
            let prior_error = result.error.clone();
            fail(&mut result, Stage::EventPersistence, error);
            if let Some(prior_error) = prior_error {
                result.error = Some(format!(
                    "{prior_error} {}",
                    result.error.as_deref().unwrap_or("")
                ));
            }
            // A failed observability write is itself observable if the second
            // append succeeds. Do not pretend earlier proposals were rolled back.
            let failed_payload = payload(&result, bear, conv.id, &session_id, trigger);
            match record(state, user_id, bear.id, &session_id, failed_payload).await {
                Ok(event) => {
                    result.reflection_event_id = Some(event.id);
                    result.reflection_event_sequence_no = Some(event.sequence_no);
                }
                Err(_) => result.observability_error = Some("The failure event could not be recorded. Check BearWire/database availability before retrying; earlier checkpoint/proposal writes may still be saved.".into()),
            }
        }
    }
    result.reflection_payload_json =
        pretty_json(payload(&result, bear, conv.id, &session_id, trigger));
    Ok(result)
}

fn payload(
    result: &ManualReflectionResult,
    bear: &den_service::bears::Bear,
    conversation_id: Uuid,
    session_id: &str,
    trigger: &str,
) -> serde_json::Value {
    let mut reflection = json!({
        "status": if result.error.is_some() { "failed" } else if result.skipped_reason.is_some() { "skipped" } else { "processed" },
        "trigger": trigger, "skipped_reason": result.skipped_reason, "error": result.error,
        "failed_stage": result.failed_stage, "compaction_status": result.compaction_status,
        "compaction_applied": result.compaction_applied, "compaction_diagnostic": result.compaction_diagnostic,
        "source_message_start_seq": result.source_message_start_seq,
        "source_message_end_seq": result.source_message_end_seq,
    });
    if result.proposals_complete {
        reflection["candidate_count"] = json!(result.candidate_count);
        reflection["discarded_count"] = json!(result.discarded_count);
        reflection["discarded_reasons"] = json!(result.discarded_reasons);
        reflection["dropped_followup_count"] = json!(result.dropped_followup_count);
        reflection["proposal_ids"] = json!(result.proposal_ids);
    }
    json!({ "session_id": session_id, "bear_slug": bear.slug, "conversation_id": conversation_id, "trigger": trigger, "pair_reflection": reflection })
}

async fn record(
    state: &AppState,
    user_id: Option<i32>,
    bear_id: Uuid,
    session_id: &str,
    payload: serde_json::Value,
) -> Result<bearwire_events::BearWireEventRow, CustomError> {
    let mut event = BearWireEvent::ephemeral("session.reflected", payload);
    event.bear_id = Some(bear_id.to_string());
    event.human_id = user_id.map(|id| id.to_string());
    event.session_id = Some(session_id.to_string());
    Ok(bearwire_events::append_bearwire_event(
        state.sqlx_pool(),
        session_id,
        Some(bear_id),
        user_id,
        event,
    )
    .await?)
}

#[cfg(test)]
#[path = "tests/manual_reflection.rs"]
mod tests;
