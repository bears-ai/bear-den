use super::{
    access, bearwire_events, client_sessions, create_pair_reflection_proposals_from_latest_summary,
    json, prepare_turn_compaction, resolved_or_stored_conversation_id, BearWireEvent,
    CompactionSource, ConversationReview, ConversationReviewFinding,
    ConversationReviewFindingDetail, ConversationReviewTrigger, CustomError, DenState,
    FindingSource, PgPool, TurnCompactionState, TurnCompactionTrigger, Value,
};

pub async fn reflect_open_sessions_once(state: &DenState) -> Result<usize, CustomError> {
    let candidates = client_sessions::list_open_reflection_candidates(
        &state.sqlx_pool,
        client_sessions::OpenReflectionCandidatesParams {
            stale_after_minutes: 30,
            activity_threshold: 20,
            limit: 25,
        },
    )
    .await?;
    let mut processed = 0;
    for candidate in candidates {
        let session = candidate.session();
        match reflect_pair_session(
            &state.sqlx_pool,
            state,
            &session,
            &candidate.reflection_trigger,
        )
        .await
        {
            Ok(reflection_payload) => {
                processed += 1;
                let mut event = BearWireEvent::ephemeral(
                    "session.reflected",
                    json!({
                        "session_id": session.client_session_id,
                        "bear_slug": session.bear_slug,
                        "trigger": candidate.reflection_trigger,
                        "event_count": candidate.event_count,
                        "latest_compaction_source_end_seq": candidate.latest_compaction_source_end_seq,
                        "last_reflected_source_end_seq": candidate.last_reflected_source_end_seq,
                        "pair_reflection": reflection_payload,
                    }),
                );
                event.bear_id = Some(session.bear_id.to_string());
                event.human_id = Some(session.user_id.to_string());
                event.session_id = Some(session.client_session_id.clone());
                if let Err(error) = bearwire_events::append_bearwire_event(
                    &state.sqlx_pool,
                    &session.client_session_id,
                    Some(session.bear_id),
                    Some(session.user_id),
                    event,
                )
                .await
                {
                    tracing::warn!(session_id = %session.client_session_id, error = %error, "failed to record open-session reflection event");
                }
            }
            Err(error) => {
                tracing::warn!(session_id = %session.client_session_id, error = %error, "open-session pair reflection failed");
            }
        }
    }
    Ok(processed)
}

pub(super) async fn reflect_pair_session(
    pool: &PgPool,
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
    trigger: &str,
) -> Result<Value, CustomError> {
    access::require_live_source(state, session).await?;
    if den_docket::work_runs::get_live_work_run_by_session(pool, &session.client_session_id)
        .await?
        .is_some()
    {
        return Err(CustomError::Authorization(
            "Work is not an ordinary conversation reflection source".into(),
        ));
    }
    let conversation_id = resolved_or_stored_conversation_id(session).to_string();
    let compaction_state = prepare_turn_compaction(
        pool,
        &state.config,
        session.bear_id,
        &conversation_id,
        CompactionSource::ContextMaintenance,
        TurnCompactionTrigger::ConversationReview,
    )
    .await?;
    let output = create_pair_reflection_proposals_from_latest_summary(
        pool,
        &state.config,
        &state.memory_stores,
        session.bear_id,
        &conversation_id,
        &session.client_session_id,
    )
    .await
    .map_err(CustomError::from)?;
    let review = build_pair_conversation_review(
        conversation_id.clone(),
        session.client_session_id.clone(),
        trigger,
        compaction_state.as_ref(),
        output.candidate_count,
        output.source_message_start_seq,
        output.source_message_end_seq,
    );
    Ok(json!({
        "status": if output.skipped_reason.is_some() { "skipped" } else { "processed" },
        "trigger": trigger,
        "conversation_review": review,
        "skipped_reason": output.skipped_reason,
        "candidate_count": output.candidate_count,
        "discarded_count": output.discarded_count,
        "discarded_reasons": output.discarded_reasons,
        "dropped_followup_count": output.dropped_followup_count,
        "proposal_ids": output.created_proposal_ids,
        "source_message_start_seq": output.source_message_start_seq,
        "source_message_end_seq": output.source_message_end_seq,
    }))
}

fn build_pair_conversation_review(
    conversation_id: String,
    client_session_id: String,
    trigger: &str,
    compaction_state: Option<&TurnCompactionState>,
    memory_candidate_count: usize,
    source_message_start_seq: Option<i64>,
    source_message_end_seq: Option<i64>,
) -> ConversationReview {
    let refs = source_seq_refs(source_message_start_seq, source_message_end_seq);
    let mut findings = Vec::new();

    if let Some(state) = compaction_state {
        if state.decision.is_some() {
            findings.push(ConversationReviewFinding {
                source: FindingSource::runtime(refs.clone()),
                detail: ConversationReviewFindingDetail::CompactionNeeded {
                    reason: "Conversation review produced a compaction artifact.".to_string(),
                },
            });
        }
    }

    if memory_candidate_count > 0 {
        findings.push(ConversationReviewFinding {
            source: FindingSource::runtime(refs),
            detail: ConversationReviewFindingDetail::MemoryReflectionCandidate {
                reason: format!(
                    "Pair reflection found {memory_candidate_count} memory candidate(s)."
                ),
            },
        });
    }

    ConversationReview::new(
        conversation_id,
        Some(client_session_id),
        None,
        conversation_review_trigger_from_reflection_trigger(trigger),
        findings,
    )
}

fn conversation_review_trigger_from_reflection_trigger(trigger: &str) -> ConversationReviewTrigger {
    match trigger {
        "session_close" => ConversationReviewTrigger::SessionClose,
        "manual" => ConversationReviewTrigger::Manual,
        _ => ConversationReviewTrigger::OpenSessionSweep,
    }
}

fn source_seq_refs(start_seq: Option<i64>, end_seq: Option<i64>) -> Vec<String> {
    match (start_seq, end_seq) {
        (Some(start), Some(end)) => vec![format!("conversation_seq:{start}-{end}")],
        (Some(start), None) => vec![format!("conversation_seq:{start}-")],
        (None, Some(end)) => vec![format!("conversation_seq:-{end}")],
        (None, None) => Vec::new(),
    }
}
