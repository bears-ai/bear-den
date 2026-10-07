//! Serialize source creation/publication, never replace a published canonical source.

use den_core::ids::{BearId, HatId, UserId};
use den_http::errors::CustomError;
use den_service::client_sessions::{ClientSessionMode, ClientSessionRow, UpsertClientSession};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// A provisional client target is a parsed locator, not an authorization grant.
#[derive(Clone, Copy)]
pub(in crate::methods) struct PendingConversationId<'a>(&'a str);

impl<'a> PendingConversationId<'a> {
    pub(in crate::methods) fn parse(value: &'a str) -> Option<Self> {
        let suffix = value.strip_prefix("new-")?;
        (!suffix.is_empty() && !value.chars().any(char::is_whitespace)).then_some(Self(value))
    }

    pub(in crate::methods) fn as_str(self) -> &'a str {
        self.0
    }
}

#[derive(Clone, Copy)]
pub(in crate::methods) enum NewSourceAuthority {
    Ordinary(HatId),
    WorkRun(Uuid),
}

pub(in crate::methods) struct NewRunSource<'a> {
    pub bear: BearId,
    pub user: UserId,
    pub session_id: &'a str,
    pub selection: &'a str,
    pub authority: NewSourceAuthority,
    pub initial_mode: Option<ClientSessionMode>,
}

#[derive(sqlx::FromRow)]
struct LockedSource {
    id: Uuid,
    conversation_id: String,
    resolved_conversation_id: Option<String>,
    closed_at: Option<sqlx::types::time::OffsetDateTime>,
    archived_at: Option<sqlx::types::time::OffsetDateTime>,
}

fn source_changed() -> CustomError {
    CustomError::Authorization("admitted canonical session conversation changed".into())
}

async fn lock_publication(
    tx: &mut Transaction<'_, Postgres>,
    session_id: &str,
) -> Result<(), CustomError> {
    // No row exists on a direct initial start. This short transaction lock also
    // serializes that case, in a namespace independent of focused-task commands.
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('bearwire.source-publication:' || $1, 0)) IS NULL AS \"locked!\"",
        session_id,
    ).fetch_one(&mut **tx).await?;
    Ok(())
}

async fn lock_source(
    tx: &mut Transaction<'_, Postgres>,
    source: &NewRunSource<'_>,
) -> Result<LockedSource, CustomError> {
    sqlx::query_as!(LockedSource,
        "SELECT id, conversation_id, resolved_conversation_id, closed_at, archived_at
         FROM client_sessions WHERE bear_id = $1 AND user_id = $2 AND client_session_id = $3 FOR UPDATE",
        source.bear.as_uuid(), source.user.get(), source.session_id,
    ).fetch_optional(&mut **tx).await?
        .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))
}

async fn create_and_publish(
    tx: &mut Transaction<'_, Postgres>,
    source: &NewRunSource<'_>,
    locked: &LockedSource,
) -> Result<String, CustomError> {
    let external_id = if PendingConversationId::parse(&locked.conversation_id).is_some() {
        format!("den-conv-{}", Uuid::new_v4().simple())
    } else {
        locked.conversation_id.clone()
    };
    let (hat, work) = match source.authority {
        NewSourceAuthority::Ordinary(hat) => (Some(hat.as_uuid()), None),
        NewSourceAuthority::WorkRun(run) => (None, Some(run)),
    };
    // Owner/hat insertion and publication share the locked client row's transaction.
    // Existing history can never be promoted by this new-source command.
    let created = sqlx::query_scalar!(
        r#"INSERT INTO conversations (bear_id, created_by_user_id, external_conversation_id, source_client_session_id, hat_id)
           SELECT $1, $2, $3, $4, $5 FROM user_bear ub
           WHERE ub.bear_id = $1 AND ub.user_id = $2
             AND NOT EXISTS (SELECT 1 FROM conversations c
                 WHERE c.bear_id = $1 AND c.external_conversation_id = $6)
             AND NOT EXISTS (SELECT 1 FROM archived_conversations a
                 WHERE a.bear_id = $1 AND a.conversation_id = $6)
             AND (
                 ($5::uuid IS NOT NULL AND $7::uuid IS NULL
                  AND EXISTS (SELECT 1 FROM bear_hats h WHERE h.id = $5 AND h.bear_id = $1)
                  AND NOT EXISTS (SELECT 1 FROM turn_runs r WHERE r.session_id = $4)
                  AND NOT EXISTS (SELECT 1 FROM bear_work_runs r
                      WHERE r.bearwire_session_id = $4 OR r.attached_client_session_id = $4))
                 OR ($5::uuid IS NULL AND $7::uuid IS NOT NULL
                     AND EXISTS (SELECT 1 FROM bear_work_runs r JOIN bear_jobs j ON j.id = r.job_id
                         WHERE r.id = $7 AND r.bear_id = $1 AND j.bear_id = $1
                           AND j.created_by_user_id = $2 AND r.bearwire_session_id = $4
                           AND NOT r.cancel_requested
                           AND r.state IN ('claimed', 'provisioning', 'running', 'reporting')))
             )
           RETURNING id"#,
        source.bear.as_uuid(), source.user.get(), external_id, source.session_id,
        hat, locked.conversation_id, work,
    ).fetch_optional(&mut **tx).await?;
    if created.is_none() {
        return Err(source_changed());
    }
    sqlx::query!(
        "UPDATE client_sessions SET resolved_conversation_id = $2, updated_at = NOW() WHERE id = $1",
        locked.id, external_id,
    ).execute(&mut **tx).await?;
    Ok(external_id)
}

pub(in crate::methods) async fn materialize_pending(
    pool: &PgPool,
    session: &ClientSessionRow,
    hat: HatId,
) -> Result<(), CustomError> {
    den_service::bears::hats::manage::get_hat(pool, BearId::new(session.bear_id), hat).await?;
    let source = NewRunSource {
        bear: BearId::new(session.bear_id),
        user: UserId::new(session.user_id),
        session_id: &session.client_session_id,
        selection: &session.conversation_id,
        authority: NewSourceAuthority::Ordinary(hat),
        initial_mode: None,
    };
    let mut tx = pool.begin().await?;
    let locked = lock_source(&mut tx, &source).await?;
    if locked.resolved_conversation_id.is_some()
        || locked.closed_at.is_some()
        || locked.archived_at.is_some()
        || locked.conversation_id != source.selection
        || PendingConversationId::parse(&locked.conversation_id).is_none()
    {
        return Err(CustomError::Authorization(
            "session is no longer awaiting a hat".into(),
        ));
    }
    create_and_publish(&mut tx, &source, &locked).await?;
    tx.commit().await?;
    Ok(())
}

pub(in crate::methods) async fn materialize_run_source(
    pool: &PgPool,
    source: NewRunSource<'_>,
) -> Result<String, CustomError> {
    let mut tx = pool.begin().await?;
    lock_publication(&mut tx, source.session_id).await?;
    let runtime_id = format!("bearwire:{}:{}", source.bear.as_uuid(), source.session_id);
    // A conflicting initial command must adopt the published source or fail;
    // its independent provisional selection is never written over the winner.
    sqlx::query!(
        r#"INSERT INTO client_sessions
             (user_id, bear_id, bear_slug, client_session_id, runtime_session_id, conversation_id, client, current_mode)
           SELECT $1, $2, b.slug, $3, $4, $5, $6, COALESCE($7, 'ask') FROM bears b
           JOIN user_bear ub ON ub.bear_id = b.id AND ub.user_id = $1
           WHERE b.id = $2 AND NOT EXISTS (
               SELECT 1 FROM client_sessions s WHERE s.client_session_id = $3
                 AND (s.bear_id <> $2 OR s.user_id <> $1))
           ON CONFLICT (user_id, bear_id, client_session_id) DO NOTHING"#,
        source.user.get(), source.bear.as_uuid(), source.session_id, runtime_id, source.selection,
        crate::methods::DEFAULT_CLIENT, source.initial_mode.map(ClientSessionMode::as_str),
    ).execute(&mut *tx).await?;
    let locked = lock_source(&mut tx, &source).await?;
    if locked.closed_at.is_some() || locked.archived_at.is_some() {
        return Err(source_changed());
    }
    if source.selection != locked.conversation_id
        && Some(source.selection) != locked.resolved_conversation_id.as_deref()
    {
        return Err(source_changed());
    }
    let external_id = match locked.resolved_conversation_id.as_ref() {
        Some(winner) => winner.clone(),
        None => create_and_publish(&mut tx, &source, &locked).await?,
    };
    tx.commit().await?;
    Ok(external_id)
}

pub(in crate::methods::run) async fn publish_run_metadata(
    pool: &PgPool,
    session: UpsertClientSession,
) -> Result<(), CustomError> {
    // CAS against the latest canonical ID, not its still-valid pending alias.
    let published = sqlx::query_scalar!(
        r#"INSERT INTO client_sessions (
             user_id, bear_id, bear_slug, client_session_id, runtime_session_id,
             conversation_id, resolved_conversation_id, client, cwd, current_mode)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, 'ask'))
           ON CONFLICT (user_id, bear_id, client_session_id) DO UPDATE
           SET bear_slug = EXCLUDED.bear_slug, runtime_session_id = EXCLUDED.runtime_session_id, client = EXCLUDED.client,
               cwd = COALESCE(EXCLUDED.cwd, client_sessions.cwd), updated_at = NOW(),
               resolved_conversation_id = COALESCE(client_sessions.resolved_conversation_id, EXCLUDED.resolved_conversation_id)
           WHERE client_sessions.closed_at IS NULL AND client_sessions.archived_at IS NULL
             AND COALESCE(client_sessions.resolved_conversation_id, client_sessions.conversation_id) = EXCLUDED.resolved_conversation_id
           RETURNING id"#,
        session.user_id, session.bear_id, session.bear_slug, session.client_session_id,
        session.runtime_session_id, session.conversation_id, session.resolved_conversation_id,
        session.client, session.cwd, session.current_mode.map(ClientSessionMode::as_str),
    ).fetch_optional(pool).await?;
    if published.is_none() {
        return Err(source_changed());
    }
    Ok(())
}

pub(in crate::methods) async fn publish_open_metadata(
    pool: &PgPool,
    session: UpsertClientSession,
) -> Result<(), CustomError> {
    let mut tx = pool.begin().await?;
    lock_publication(&mut tx, &session.client_session_id).await?;
    let published = sqlx::query_scalar!(
        r#"INSERT INTO client_sessions (
             user_id, bear_id, bear_slug, client_session_id, runtime_session_id,
             conversation_id, resolved_conversation_id, client, cwd, current_mode)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, 'ask'))
           ON CONFLICT (user_id, bear_id, client_session_id) DO UPDATE
           SET bear_slug = EXCLUDED.bear_slug, runtime_session_id = EXCLUDED.runtime_session_id, client = EXCLUDED.client,
               cwd = COALESCE(EXCLUDED.cwd, client_sessions.cwd), updated_at = NOW(),
               closed_at = NULL, archived_at = NULL
           WHERE client_sessions.conversation_id = EXCLUDED.conversation_id
             AND (EXCLUDED.resolved_conversation_id IS NULL
                  OR client_sessions.resolved_conversation_id = EXCLUDED.resolved_conversation_id)
           RETURNING id"#,
        session.user_id, session.bear_id, session.bear_slug, session.client_session_id,
        session.runtime_session_id, session.conversation_id, session.resolved_conversation_id,
        session.client, session.cwd, session.current_mode.map(ClientSessionMode::as_str),
    ).fetch_optional(&mut *tx).await?;
    if published.is_none() {
        return Err(source_changed());
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
