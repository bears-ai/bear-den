use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::CustomError;

pub async fn list_for_bear(pool: &PgPool, bear_id: Uuid) -> Result<HashSet<String>, CustomError> {
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT conversation_id
        FROM archived_conversations
        WHERE bear_id = $1
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().collect())
}

pub async fn set_archived(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    archived_by_user_id: Option<i32>,
    source: &str,
    archived: bool,
) -> Result<(), CustomError> {
    if archived {
        sqlx::query(
            r#"
            INSERT INTO archived_conversations (
                bear_id, conversation_id, archived_by_user_id, source
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (bear_id, conversation_id) DO UPDATE
            SET archived_by_user_id = EXCLUDED.archived_by_user_id,
                source = EXCLUDED.source,
                archived_at = NOW(),
                updated_at = NOW()
            "#,
        )
        .bind(bear_id)
        .bind(conversation_id)
        .bind(archived_by_user_id)
        .bind(source)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            r#"
            DELETE FROM archived_conversations
            WHERE bear_id = $1 AND conversation_id = $2
            "#,
        )
        .bind(bear_id)
        .bind(conversation_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}
