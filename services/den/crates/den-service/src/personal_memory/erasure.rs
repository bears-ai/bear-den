//! Cleanup shares the caller's authority transaction, without borrowing another pool connection.
//! Commit PostgreSQL before acknowledging the SQLite ledger; failures remain retryable.

use den_core::DenError;
use den_memory::{
    personal_erasure::{self, PersonalErasureReceipt},
    AuthenticatedUserEntity, BearMemoryStore,
};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};

use super::authority;

use crate::conversation::{
    dependencies::ContextDependencies,
    message_types::{ConversationMessageType, ConversationMessageVisibility},
};

pub(super) async fn prepare_pending(
    locked: &mut authority::LockedMember,
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
) -> Result<Vec<PersonalErasureReceipt>, DenError> {
    let receipts = personal_erasure::pending_erasure_cleanups(store, binding, 100).await?;
    for receipt in &receipts {
        clean_receipt(locked.connection(), binding, receipt).await?;
    }
    Ok(receipts)
}

pub(super) async fn acknowledge_committed(
    pool: &PgPool,
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    receipts: &[PersonalErasureReceipt],
) -> Result<(), DenError> {
    if receipts.is_empty() {
        return Ok(());
    }
    // The first PG transaction has already committed and released its connection.
    // Recheck membership before recording completion; no second connection is held.
    let locked = authority::lock_member(pool, binding.bear_id(), binding.user_id()).await?;
    for receipt in receipts {
        personal_erasure::mark_erasure_cleanup_complete(store, binding, receipt.erasure_id).await?;
    }
    drop(locked);
    Ok(())
}

pub(super) async fn clean_receipt(
    connection: &mut PgConnection,
    binding: &AuthenticatedUserEntity,
    receipt: &PersonalErasureReceipt,
) -> Result<(), DenError> {
    if !receipt.affected_fact_ids.is_empty() {
        let bear_id = binding.bear_id().as_uuid();
        let bear_text = binding.bear_id().to_string();
        let ids: Vec<String> = receipt
            .affected_fact_ids
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut dependencies = ContextDependencies::default();
        for id in &receipt.affected_fact_ids {
            dependencies.union_from(&ContextDependencies::personal_fact(binding.bear_id(), *id));
        }
        // Reconstruct a value-free tombstone; do not retain arbitrary old JSON payload fields.
        let mut redacted = Value::Null;
        dependencies.attach_to_metadata(&mut redacted)?;
        let user_role = ConversationMessageType::User.as_str();
        let diagnostic = ConversationMessageVisibility::DiagnosticOnly.as_str();

        sqlx::query!(
            r"UPDATE conversation_messages m
              SET content_text = '', content_json = $4, visibility = $6
              FROM conversations c
              WHERE m.conversation_id = c.id AND c.bear_id = $1
                AND m.message_type <> $5 AND m.role IS DISTINCT FROM $5
                AND EXISTS (
                    SELECT 1 FROM jsonb_array_elements(
                        CASE WHEN jsonb_typeof(m.content_json->'den_context_dependencies'->'personal_facts') = 'array'
                             THEN m.content_json->'den_context_dependencies'->'personal_facts'
                             ELSE '[]'::jsonb END
                    ) d WHERE d->>'bear_id' = $2 AND d->>'fact_id' = ANY($3)
                )",
            bear_id, bear_text, &ids, redacted, user_role, diagnostic,
        ).execute(&mut *connection).await?;
        sqlx::query!(
            r"UPDATE conversation_compaction_artifacts a SET artifact_json = $4
              FROM conversations c
              WHERE a.conversation_id = c.id AND c.bear_id = $1
                AND EXISTS (
                    SELECT 1 FROM jsonb_array_elements(
                        CASE WHEN jsonb_typeof(a.artifact_json->'den_context_dependencies'->'personal_facts') = 'array'
                             THEN a.artifact_json->'den_context_dependencies'->'personal_facts'
                             ELSE '[]'::jsonb END
                    ) d WHERE d->>'bear_id' = $2 AND d->>'fact_id' = ANY($3)
                )",
            bear_id, bear_text, &ids, redacted,
        ).execute(&mut *connection).await?;
        sqlx::query!(
            r"DELETE FROM bearwire_events e WHERE e.bear_id = $1
                AND EXISTS (
                    SELECT 1 FROM jsonb_array_elements(
                        CASE WHEN jsonb_typeof(e.event_json->'den_context_dependencies'->'personal_facts') = 'array'
                             THEN e.event_json->'den_context_dependencies'->'personal_facts'
                             ELSE '[]'::jsonb END
                    ) d WHERE d->>'bear_id' = $2 AND d->>'fact_id' = ANY($3)
                )",
            bear_id, bear_text, &ids,
        ).execute(&mut *connection).await?;
    }
    // Only the caller can commit this transaction. Until then no cleanup receipt is
    // acknowledged, so rollback/crash cannot falsely report that bodies were removed.
    Ok(())
}
