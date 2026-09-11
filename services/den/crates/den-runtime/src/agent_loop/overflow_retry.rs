//! Emergency compaction + message reassembly when the LLM rejects an oversized prompt.

use den_core::{config::Config, profile::BearProfile, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    agent_loop::{
        context::{
            load_transcript_messages, load_transcript_messages_after_seq,
            repair_tool_call_message_chain,
        },
        session_store::AgentLoopSession,
    },
    llm::ChatMessage,
    runtime_compaction::{
        render_compaction_prompt_context, run_compaction_job, CompactionMode, TurnCompactionState,
        TurnCompactionTrigger,
    },
};

const COMPACTION_BLOCK_MARKER: &str = "Runtime compaction context is Den-owned";

/// Sync emergency compaction for context overflow; returns rebuilt session messages.
///
/// When `COMPACTION_MODE` is `active` and compaction produces a transcript cutoff, messages
/// shrink. In `observe` mode the compaction event is still recorded but messages are unchanged
/// and `recovered` is false (retry is skipped by the caller).
pub async fn compact_session_messages_for_overflow(
    pool: &PgPool,
    config: &Config,
    session: &AgentLoopSession,
    profile: BearProfile,
) -> Result<(Vec<ChatMessage>, bool), DenError> {
    let mode = CompactionMode::parse(&config.compaction_mode);
    let state = run_compaction_job(
        pool,
        config,
        session.bear_id,
        &session.conversation_id,
        profile,
        TurnCompactionTrigger::ModelSafetyMargin,
    )
    .await?;

    let Some(state) = state else {
        return Ok((session.messages.clone(), false));
    };

    if mode != CompactionMode::Active {
        return Ok((session.messages.clone(), false));
    }

    let recovered = state.compacted_seq_cutoff.is_some();
    let messages = rebuild_messages_after_overflow_compaction(
        pool,
        session.bear_id,
        &session.conversation_id,
        profile,
        &session.messages,
        &state,
    )
    .await?;
    Ok((messages, recovered))
}

async fn rebuild_messages_after_overflow_compaction(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    profile: BearProfile,
    existing: &[ChatMessage],
    state: &TurnCompactionState,
) -> Result<Vec<ChatMessage>, DenError> {
    let system = existing
        .first()
        .filter(|m| m.role == "system")
        .cloned()
        .unwrap_or_else(|| ChatMessage {
            role: "system".to_string(),
            content: Some(String::new()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        });

    let compaction_text = render_compaction_prompt_context(state);
    let system_text = upsert_compaction_block(
        system.content.as_deref().unwrap_or_default(),
        &compaction_text,
    );

    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: Some(system_text),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    }];

    let cutoff = state.compacted_seq_cutoff;
    let transcript = if cutoff.is_some() {
        load_transcript_messages_after_seq(pool, bear_id, conversation_id, cutoff).await?
    } else {
        load_transcript_messages(pool, bear_id, conversation_id).await?
    };
    messages.extend(transcript);
    messages.extend(collect_ephemeral_tail(existing));

    let messages = repair_tool_call_message_chain(messages);
    let messages = if cutoff.is_some() {
        messages
    } else if den_core::EffectivePolicy::compile(
        profile,
        den_core::Governance::Interactive,
        den_core::ArmatureAvailability::Absent,
    )
    .capabilities
    .contains(den_core::BearCapability::Converse)
    {
        crate::agent_loop::context::prune_messages_for_native_conversation(messages)
    } else {
        messages
    };
    Ok(messages)
}

fn upsert_compaction_block(system_text: &str, compaction_text: &str) -> String {
    if compaction_text.trim().is_empty() {
        return system_text.to_string();
    }
    if let Some(idx) = system_text.find(COMPACTION_BLOCK_MARKER) {
        let prefix = system_text[..idx].trim_end();
        if prefix.is_empty() {
            compaction_text.to_string()
        } else {
            format!("{prefix}\n\n{compaction_text}")
        }
    } else if system_text.trim().is_empty() {
        compaction_text.to_string()
    } else {
        format!("{system_text}\n\n{compaction_text}")
    }
}

/// Trailing in-session tool/assistant-tool messages not yet reflected in persisted transcript.
fn collect_ephemeral_tail(existing: &[ChatMessage]) -> Vec<ChatMessage> {
    if existing.len() <= 1 {
        return Vec::new();
    }
    let mut tail = Vec::new();
    for msg in existing.iter().skip(1).rev() {
        if msg.role == "tool"
            || (msg.role == "assistant"
                && msg
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty()))
        {
            tail.push(msg.clone());
        } else {
            break;
        }
    }
    tail.reverse();
    tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_compaction_block_replaces_existing_section() {
        let system = "Base prompt.\n\nRuntime compaction context is Den-owned. old=1";
        let updated =
            upsert_compaction_block(system, "Runtime compaction context is Den-owned. new=2");
        assert!(updated.starts_with("Base prompt."));
        assert!(updated.contains("new=2"));
        assert!(!updated.contains("old=1"));
    }

    #[test]
    fn collect_ephemeral_tail_keeps_trailing_tool_messages() {
        let existing = vec![
            ChatMessage {
                role: "system".to_string(),
                content: Some("sys".into()),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some("hi".into()),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("ok".into()),
                tool_call_id: Some("call_1".into()),
                name: None,
                tool_calls: None,
            },
        ];
        let tail = collect_ephemeral_tail(&existing);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].role, "tool");
    }
}
