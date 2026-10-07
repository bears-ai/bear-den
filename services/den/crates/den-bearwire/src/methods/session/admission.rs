//! Materialize a pending session only after a real hat is admitted.

use super::{client_sessions, CustomError, HatId, PgPool};

pub(super) use crate::methods::run::source_preflight::publication::PendingConversationId;

pub(super) async fn may_admit_pending_hat(
    pool: &PgPool,
    session: &client_sessions::ClientSessionRow,
) -> Result<bool, CustomError> {
    Ok(sqlx::query_scalar!(
        r#"SELECT NOT EXISTS (SELECT 1 FROM turn_runs r WHERE r.session_id = $1)
          AND NOT EXISTS (SELECT 1 FROM bear_work_runs r
              WHERE r.bearwire_session_id = $1 OR r.attached_client_session_id = $1)
          AS "allowed!""#,
        session.client_session_id,
    )
    .fetch_one(pool)
    .await?)
}

pub(super) async fn may_select_initial_hat(
    pool: &PgPool,
    session: &client_sessions::ClientSessionRow,
    conversation_id: uuid::Uuid,
) -> Result<bool, CustomError> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
            SELECT 1 FROM conversations c
            WHERE c.id = $1 AND c.bear_id = $2 AND c.created_by_user_id = $3
              AND c.status = 'active' AND c.hat_id IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM conversation_messages m WHERE m.conversation_id = c.id)
              AND NOT EXISTS (SELECT 1 FROM turn_runs r WHERE r.session_id = $4)
              AND NOT EXISTS (
                  SELECT 1 FROM client_sessions s JOIN turn_runs r ON r.session_id = s.client_session_id
                  WHERE s.bear_id = c.bear_id
                    AND (s.conversation_id = c.external_conversation_id
                         OR s.resolved_conversation_id = c.external_conversation_id)
              )
              AND NOT EXISTS (
                  SELECT 1 FROM bear_work_runs r WHERE r.bear_id = c.bear_id
                    AND (r.bearwire_session_id = $4 OR r.attached_client_session_id = $4)
              )
        ) AS "allowed!""#,
        conversation_id, session.bear_id, session.user_id, session.client_session_id,
    ).fetch_one(pool).await?)
}

pub(super) async fn materialize_pending(
    pool: &PgPool,
    session: &client_sessions::ClientSessionRow,
    hat: HatId,
) -> Result<(), CustomError> {
    crate::methods::run::source_preflight::publication::materialize_pending(pool, session, hat)
        .await
}

#[cfg(test)]
#[path = "pending_id_tests.rs"]
mod pending_id_tests;
