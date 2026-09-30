//! Human-authenticated access to canonical conversations within a Bear.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

use crate::bears::db::{membership_role_for_user, role_is_bear_admin};

use super::persistence::{ConversationRecord, ConversationRow};

/// A bounded set of canonical conversations whose private source notes belong to this human.
#[derive(Debug, Clone)]
pub struct OwnedNoteSource {
    pub id: Uuid,
    pub external_conversation_id: Option<String>,
    pub title: Option<String>,
}

/// Resolve only after the caller has authenticated the human user ID.
/// A cached viewer cannot grant access after membership or admin role is removed:
/// every read also checks the current `user_bear` row in its SQL predicate.
#[derive(Debug, Clone)]
pub struct ConversationViewer {
    bear_id: BearId,
    user_id: UserId,
    is_admin: bool,
}

impl ConversationViewer {
    pub async fn resolve(
        pool: &PgPool,
        bear_id: BearId,
        user_id: UserId,
    ) -> Result<Option<Self>, DenError> {
        let role = membership_role_for_user(pool, user_id.get(), bear_id.as_uuid()).await?;
        Ok(role.map(|role| Self {
            bear_id,
            user_id,
            is_admin: role_is_bear_admin(role.as_deref()),
        }))
    }

    pub async fn may_access_external(
        &self,
        pool: &PgPool,
        external_id: &str,
    ) -> Result<bool, DenError> {
        let allowed = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations c
                JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = $2
                WHERE c.bear_id = $1 AND c.external_conversation_id = $3
                  AND (c.created_by_user_id = $2 OR
                       ($4 AND lower(btrim(coalesce(ub.role, ''))) = 'admin'))
            ) AS "allowed!"
            "#,
            self.bear_id.as_uuid(),
            self.user_id.get(),
            external_id,
            self.is_admin,
        )
        .fetch_one(pool)
        .await?;
        Ok(allowed)
    }

    /// Source-local notes are private to the canonical conversation creator.
    /// Bear-admin transcript inspection is not a grant to browse that source's
    /// uncurated notes through the ordinary member interface.
    pub async fn may_read_own_source(&self, pool: &PgPool, id: Uuid) -> Result<bool, DenError> {
        let allowed = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                SELECT 1 FROM conversations c
                JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = $2
                WHERE c.bear_id = $1 AND c.id = $3 AND c.status = 'active'
                  AND c.created_by_user_id = $2
            ) AS "allowed!""#,
            self.bear_id.as_uuid(),
            self.user_id.get(),
            id,
        )
        .fetch_one(pool)
        .await?;
        Ok(allowed)
    }

    pub async fn may_access_id(&self, pool: &PgPool, id: Uuid) -> Result<bool, DenError> {
        let allowed = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations c
                JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = $2
                WHERE c.bear_id = $1 AND c.id = $3
                  AND (c.created_by_user_id = $2 OR
                       ($4 AND lower(btrim(coalesce(ub.role, ''))) = 'admin'))
            ) AS "allowed!"
            "#,
            self.bear_id.as_uuid(),
            self.user_id.get(),
            id,
            self.is_admin,
        )
        .fetch_one(pool)
        .await?;
        Ok(allowed)
    }

    /// This is narrower than transcript visibility: even Bear admins cannot use
    /// their inspection rights to browse another human's private notes here.
    pub async fn recent_own_note_sources(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<OwnedNoteSource>, DenError> {
        let rows = sqlx::query!(
            r#"SELECT c.id, c.external_conversation_id, c.current_title
               FROM conversations c
               JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = $2
               WHERE c.bear_id = $1 AND c.created_by_user_id = $2
                 AND c.status = 'active' AND c.hat_id IS NOT NULL
               ORDER BY c.updated_at DESC, c.id DESC
               LIMIT 200"#,
            self.bear_id.as_uuid(),
            self.user_id.get(),
        )
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| OwnedNoteSource {
                id: row.id,
                external_conversation_id: row.external_conversation_id,
                title: row.current_title,
            })
            .collect())
    }

    pub async fn list_visible(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<ConversationRecord>, DenError> {
        // Filter by membership and owner before LIMIT, not after fetching a page.
        let rows = sqlx::query_as!(
            ConversationRow,
            r#"
            SELECT c.id, c.bear_id, c.external_conversation_id, c.source_client_session_id,
                   c.current_title,
                   c.latest_context_budget_json AS "latest_context_budget_json?: Json<serde_json::Value>",
                   c.latest_context_budget_updated_at, c.updated_at
            FROM conversations c
            JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = $2
            WHERE c.bear_id = $1
              AND (c.created_by_user_id = $2 OR
                   ($3 AND lower(btrim(coalesce(ub.role, ''))) = 'admin'))
            ORDER BY c.updated_at DESC, c.id DESC
            LIMIT $4
            "#,
            self.bear_id.as_uuid(),
            self.user_id.get(),
            self.is_admin,
            limit.clamp(1, 200),
        )
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(ConversationRecord::try_from).collect()
    }
}

#[cfg(test)]
mod tests;
