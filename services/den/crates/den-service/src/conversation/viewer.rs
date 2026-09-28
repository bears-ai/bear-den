//! Human-authenticated access to canonical conversations within a Bear.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

use crate::bears::db::{membership_role_for_user, role_is_bear_admin};

use super::persistence::{ConversationRecord, ConversationRow};

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
