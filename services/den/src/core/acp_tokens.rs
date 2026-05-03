use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::CustomError;

const TOKEN_PREFIX: &str = "bears_acp_";
const ACP_CHAT_SCOPE: &str = "acp:chat";

#[derive(Debug, Clone, Serialize)]
pub struct AcpTokenListRow {
    pub id: Uuid,
    pub name: String,
    pub scopes: serde_json::Value,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub bear_name: String,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
    pub token_type: String,
    pub scope_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedAcpToken {
    pub raw_token: String,
    pub id: Uuid,
}

pub fn is_acp_token(raw: &str) -> bool {
    raw.trim().starts_with(TOKEN_PREFIX)
}

fn token_hash(raw: &str) -> String {
    let digest = Sha256::digest(raw.trim().as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn generate_raw_token() -> String {
    use rand::distr::Alphanumeric;
    let rng = rand::rng();
    let suffix: String = rng
        .sample_iter(Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    format!("{TOKEN_PREFIX}{suffix}")
}

pub async fn create_for_bear(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    name: &str,
) -> Result<CreatedAcpToken, CustomError> {
    let raw_token = generate_raw_token();
    let hash = token_hash(&raw_token);
    let name = name.trim();
    if name.is_empty() {
        return Err(CustomError::ValidationError(
            "token name must not be empty".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    let row: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO acp_tokens (user_id, name, token_hash, scopes)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(name)
    .bind(hash)
    .bind(serde_json::json!([ACP_CHAT_SCOPE]))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO acp_token_bears (token_id, bear_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(row.0)
    .bind(bear_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(CreatedAcpToken {
        raw_token,
        id: row.0,
    })
}

pub async fn list_for_user(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<AcpTokenListRow>, CustomError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id,
               t.name,
               t.scopes,
               tb.bear_id,
               b.slug AS bear_slug,
               b.name AS bear_name,
               t.created_at,
               t.expires_at,
               t.last_used_at,
               t.revoked_at
        FROM acp_tokens t
        INNER JOIN acp_token_bears tb ON tb.token_id = t.id
        INNER JOIN bears b ON b.id = tb.bear_id
        WHERE t.user_id = $1
        ORDER BY t.created_at DESC, b.slug
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let scopes: serde_json::Value = row.get("scopes");
            AcpTokenListRow {
                id: row.get("id"),
                name: row.get("name"),
                scopes: scopes.clone(),
                bear_id: row.get("bear_id"),
                bear_slug: row.get("bear_slug"),
                bear_name: row.get("bear_name"),
                created_at: row.get("created_at"),
                expires_at: row.get("expires_at"),
                last_used_at: row.get("last_used_at"),
                revoked_at: row.get("revoked_at"),
                token_type: "ACP chat".to_string(),
                scope_labels: scope_labels(&scopes),
            }
        })
        .collect())
}

pub async fn revoke_for_user(
    pool: &PgPool,
    user_id: i32,
    token_id: Uuid,
) -> Result<(), CustomError> {
    let result = sqlx::query(
        r#"
        UPDATE acp_tokens
        SET revoked_at = NOW()
        WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(token_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(CustomError::NotFound("ACP token not found".to_string()));
    }
    Ok(())
}

pub async fn authenticate_for_bear_slug(
    pool: &PgPool,
    raw_token: &str,
    bear_slug: &str,
    required_scope: &str,
) -> Result<Option<i32>, CustomError> {
    let Some(auth) = authenticate_for_bear_slug_with_scopes(pool, raw_token, bear_slug).await?
    else {
        return Ok(None);
    };
    if scopes_contains(&auth.scopes, required_scope) {
        Ok(Some(auth.user_id))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone)]
pub struct AcpTokenAuth {
    pub user_id: i32,
    pub scopes: serde_json::Value,
}

pub async fn authenticate_for_bear_slug_with_scopes(
    pool: &PgPool,
    raw_token: &str,
    bear_slug: &str,
) -> Result<Option<AcpTokenAuth>, CustomError> {
    let hash = token_hash(raw_token);
    let row: Option<(Uuid, i32, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT t.id, t.user_id, t.scopes
        FROM acp_tokens t
        INNER JOIN acp_token_bears tb ON tb.token_id = t.id
        INNER JOIN bears b ON b.id = tb.bear_id
        INNER JOIN user_bear ub ON ub.user_id = t.user_id AND ub.bear_id = b.id
        WHERE t.token_hash = $1
          AND b.slug = $2
          AND t.revoked_at IS NULL
          AND (t.expires_at IS NULL OR t.expires_at > NOW())
        "#,
    )
    .bind(hash)
    .bind(bear_slug)
    .fetch_optional(pool)
    .await?;

    let Some((token_id, user_id, scopes)) = row else {
        return Ok(None);
    };

    sqlx::query("UPDATE acp_tokens SET last_used_at = NOW() WHERE id = $1")
        .bind(token_id)
        .execute(pool)
        .await?;

    Ok(Some(AcpTokenAuth { user_id, scopes }))
}

pub fn scopes_contains(scopes: &serde_json::Value, required_scope: &str) -> bool {
    scopes
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|item| item.as_str() == Some(required_scope))
        })
        .unwrap_or(false)
}

fn scope_labels(scopes: &serde_json::Value) -> Vec<String> {
    let mut labels = Vec::new();
    if scopes_contains(scopes, ACP_CHAT_SCOPE) {
        labels.push("chat".to_string());
    }

    labels
}
