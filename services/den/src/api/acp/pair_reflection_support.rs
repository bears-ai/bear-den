use crate::{
    api::service::ApiState,
    core::{
        acp_sessions,
        bears::{db as bears_db, BearAgentRole},
        memory_manager_head::{write_memfs_role_memory_entry, MemfsWriteRoleMemoryEntryRequest},
        memory_proposals::{self, CreateMemoryProposal},
        pair_reflection::{self, CompletePairReflectionRun, CreatePairReflectionRun},
        reflection_conductor,
    },
    errors::CustomError,
};

pub(crate) async fn run_pair_reflection_summary(
    state: &ApiState,
    session: &acp_sessions::AcpSessionRow,
    trigger: &str,
) -> Result<(), CustomError> {
    let conversation_id = session
        .resolved_conversation_id
        .as_deref()
        .or_else(|| {
            session
                .conversation_id
                .trim()
                .strip_prefix("")
                .filter(|_| false)
        })
        .or_else(|| {
            let raw = session.conversation_id.trim();
            raw.starts_with("conv-").then_some(raw)
        });
    let messages_value = if state.letta.is_enabled() {
        if let Some(conversation_id) = conversation_id {
            state
                .letta
                .list_conversation_messages(conversation_id, None, 20, None, false)
                .await
                .ok()
        } else {
            None
        }
    } else {
        None
    };
    let message_summaries = summarize_letta_messages(messages_value.as_ref());
    let run = pair_reflection::create_run(
        &state.sqlx_pool,
        CreatePairReflectionRun {
            bear_id: session.bear_id,
            user_id: session.user_id,
            acp_session_id: &session.acp_session_id,
            conversation_id,
            trigger,
            considered_message_count: message_summaries.len() as i32,
            considered_memory_paths: Vec::new(),
            diagnostic: serde_json::json!({
                "phase": "pair_reflection_started",
                "conversation_id": conversation_id,
                "message_count": message_summaries.len(),
            }),
        },
    )
    .await?;
    let body = pair_reflection::render_pair_summary_markdown(
        &session.acp_session_id,
        conversation_id,
        trigger,
        &message_summaries,
    );
    let request = MemfsWriteRoleMemoryEntryRequest {
        kind: "summary".to_string(),
        title: pair_reflection::summary_title_for_session(&session.acp_session_id),
        body,
        tags: vec!["pair-reflection".to_string(), "session-summary".to_string()],
        refs: None,
        lifecycle: Some(serde_json::json!({
            "scope": "role-local",
            "retention": "durable",
            "promotion": "maybe",
            "status": "active"
        })),
        source: Some(serde_json::json!({
            "human": { "user_id": session.user_id, "authenticated_by": "acp_token" },
            "session": {
                "acp_session_id": session.acp_session_id,
                "conversation_id": conversation_id,
                "trigger": trigger
            },
            "reflection_run_id": run.id,
        })),
        author: None,
        conversation_id: conversation_id.map(str::to_string),
        session_id: Some(session.acp_session_id.clone()),
        acp_session_id: Some(session.acp_session_id.clone()),
        conversation_selection: Some(session.conversation_id.clone()),
        runtime_target: conversation_id.map(str::to_string),
        role_agent_id: None,
        agent_role: Some(pair_reflection::pair_reflection_role().as_str().to_string()),
        request_id: None,
    };
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| {
            CustomError::System(format!("MemFS pair reflection client build failed: {e}"))
        })?;
    let write_response = write_memfs_role_memory_entry(
        &http,
        &state.config.letta_memfs_service_url,
        session.bear_id,
        BearAgentRole::Pair.as_str(),
        &request,
    )
    .await?;
    let Some(write_response) = write_response else {
        pair_reflection::complete_run(
            &state.sqlx_pool,
            CompletePairReflectionRun {
                id: run.id,
                status: "skipped",
                summary_path: None,
                summary_commit: None,
                diagnostic: serde_json::json!({"reason": "MemFS sidecar not configured"}),
            },
        )
        .await?;
        return Ok(());
    };
    let completed_run = pair_reflection::complete_run(
        &state.sqlx_pool,
        CompletePairReflectionRun {
            id: run.id,
            status: "completed",
            summary_path: Some(&write_response.path),
            summary_commit: write_response.canonical_tip.as_deref(),
            diagnostic: serde_json::json!({
                "phase": "pair_reflection_completed",
                "path": write_response.path,
                "commit": write_response.canonical_tip,
            }),
        },
    )
    .await?;

    let pair_agent_id =
        bears_db::role_agent_id(&state.sqlx_pool, session.bear_id, BearAgentRole::Pair)
            .await?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    let proposal = memory_proposals::create(
        &state.sqlx_pool,
        CreateMemoryProposal {
            bear_id: session.bear_id,
            source_role: BearAgentRole::Pair,
            source_agent_id: pair_agent_id.clone(),
            source_paths: vec![write_response.path.clone()],
            source_refs: serde_json::json!({
                "acp_session_id": session.acp_session_id,
                "conversation_id": conversation_id,
                "reflection_run_id": completed_run.id,
            }),
            suggested_action: "unspecified",
            target_ref: None,
            title: &format!("Review pair reflection summary: {}", session.acp_session_id),
            summary: "Pair reflection created a durable session summary; review for useful shared/work-visible knowledge.",
            rationale: "Pair reflection summaries may contain durable decisions, lessons, or work-visible knowledge that should be curated beyond pair-local memory.",
            proposed_content: None,
            proposed_patch: None,
            refs: serde_json::json!({
                "summary_path": write_response.path,
                "summary_commit": write_response.canonical_tip,
                "reflection_run_id": completed_run.id,
            }),
            sensitivity: "normal",
            requires_human: false,
        },
    )
    .await?;

    let reflection_date = time::OffsetDateTime::now_utc().date();
    let conversation_key = format!("memory_curate:{reflection_date}");
    reflection_conductor::enqueue_memory_curate_for_proposals(
        &state.sqlx_pool,
        reflection_conductor::ProposalEnqueueParams {
            bear_id: session.bear_id,
            role_agent_id: pair_agent_id.as_deref(),
            conversation_id,
            conversation_key: Some(&conversation_key),
            conversation_date: Some(reflection_date),
            trigger: "pair_reflection",
            proposal_ids: vec![proposal.id],
        },
    )
    .await?;
    Ok(())
}

pub(super) fn summarize_letta_messages(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let messages = value
        .get("messages")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("items"))
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array());
    let Some(messages) = messages else {
        return Vec::new();
    };
    messages
        .iter()
        .rev()
        .filter_map(|message| {
            let role = message
                .get("role")
                .or_else(|| message.get("message_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("message");
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .or_else(|| message.get("text").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                None
            } else {
                Some(format!("{role}: {}", truncate_for_reflection(content, 300)))
            }
        })
        .take(20)
        .collect()
}

pub(super) fn truncate_for_reflection(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}
