use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::errors::CustomError;

const TOKEN_PREFIX: &str = "bear_arm_";
const ARMATURE_CHAT_SCOPE: &str = "armature:chat";
const ARMATURE_TOOLS_SCOPE: &str = "armature:tools";

pub fn armature_chat_scope() -> &'static str {
    ARMATURE_CHAT_SCOPE
}

pub fn armature_tools_scope() -> &'static str {
    ARMATURE_TOOLS_SCOPE
}

#[derive(Debug, Clone, Serialize)]
pub struct ArmatureTokenListRow {
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
pub struct CreatedArmatureToken {
    pub raw_token: String,
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArmatureTokenDiagnostics {
    pub bear_slug: String,
    pub required_scope: String,
    pub token_prefix_ok: bool,
    pub token_found: bool,
    pub token_active: bool,
    pub bear_found: bool,
    pub token_bound_to_bear: bool,
    pub token_owner_is_bear_member: bool,
    pub required_scope_present: bool,
    pub ok: bool,
    pub failure_reasons: Vec<String>,
}

impl ArmatureTokenDiagnostics {
    pub fn summary(&self) -> String {
        format!(
            "bear_slug={:?}; token_prefix_ok={}; token_found={}; token_active={}; bear_found={}; token_bound_to_bear={}; token_owner_is_bear_member={}; required_scope_present={}; failure_reasons={}",
            self.bear_slug,
            self.token_prefix_ok,
            self.token_found,
            self.token_active,
            self.bear_found,
            self.token_bound_to_bear,
            self.token_owner_is_bear_member,
            self.required_scope_present,
            if self.failure_reasons.is_empty() {
                "none".to_string()
            } else {
                self.failure_reasons.join(", ")
            }
        )
    }
}

pub fn is_armature_token(raw: &str) -> bool {
    raw.trim().starts_with(TOKEN_PREFIX)
}

pub fn hash_raw_token_for_seed(raw: &str) -> String {
    token_hash(raw)
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
) -> Result<CreatedArmatureToken, CustomError> {
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
        r"
        INSERT INTO armature_tokens (user_id, name, token_hash, scopes)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        ",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash)
    .bind(serde_json::json!([
        ARMATURE_CHAT_SCOPE,
        ARMATURE_TOOLS_SCOPE
    ]))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r"
        INSERT INTO armature_token_bears (token_id, bear_id)
        VALUES ($1, $2)
        ",
    )
    .bind(row.0)
    .bind(bear_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(CreatedArmatureToken {
        raw_token,
        id: row.0,
    })
}

pub async fn list_for_user(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<ArmatureTokenListRow>, CustomError> {
    let rows = sqlx::query(
        r"
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
        FROM armature_tokens t
        INNER JOIN armature_token_bears tb ON tb.token_id = t.id
        INNER JOIN bears b ON b.id = tb.bear_id
        WHERE t.user_id = $1
        ORDER BY t.created_at DESC, b.slug
        ",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let scopes: serde_json::Value = row.get("scopes");
            ArmatureTokenListRow {
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
                token_type: if scopes_contains(&scopes, ARMATURE_TOOLS_SCOPE) {
                    "Armature code".to_string()
                } else {
                    "Armature chat".to_string()
                },
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
        r"
        UPDATE armature_tokens
        SET revoked_at = NOW()
        WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
        ",
    )
    .bind(token_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(CustomError::NotFound(
            "Armature token not found".to_string(),
        ));
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

pub async fn diagnose_for_bear_slug(
    pool: &PgPool,
    raw_token: &str,
    bear_slug: &str,
    required_scope: &str,
) -> Result<ArmatureTokenDiagnostics, CustomError> {
    let bear_slug = bear_slug.trim();
    let token_prefix_ok = is_armature_token(raw_token);
    let hash = token_hash(raw_token);

    let token: Option<(Uuid, i32, serde_json::Value, bool)> = sqlx::query_as(
        r"
        SELECT id,
               user_id,
               scopes,
               revoked_at IS NULL AND (expires_at IS NULL OR expires_at > NOW()) AS active
        FROM armature_tokens
        WHERE token_hash = $1
        ",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?;

    let bear: Option<(Uuid,)> = sqlx::query_as(
        r"
        SELECT id
        FROM bears
        WHERE slug = $1
        ",
    )
    .bind(bear_slug)
    .fetch_optional(pool)
    .await?;

    let token_found = token.is_some();
    let token_active = token.as_ref().is_some_and(|(_, _, _, active)| *active);
    let bear_found = bear.is_some();
    let required_scope_present = token
        .as_ref()
        .is_some_and(|(_, _, scopes, _)| scopes_contains(scopes, required_scope));

    let mut token_bound_to_bear = false;
    let mut token_owner_is_bear_member = false;
    if let (Some((token_id, user_id, _, _)), Some((bear_id,))) = (&token, &bear) {
        let bound: (bool,) = sqlx::query_as(
            r"
            SELECT EXISTS(
                SELECT 1 FROM armature_token_bears
                WHERE token_id = $1 AND bear_id = $2
            )
            ",
        )
        .bind(token_id)
        .bind(bear_id)
        .fetch_one(pool)
        .await?;
        token_bound_to_bear = bound.0;

        let member: (bool,) = sqlx::query_as(
            r"
            SELECT EXISTS(
                SELECT 1 FROM user_bear
                WHERE user_id = $1 AND bear_id = $2
            )
            ",
        )
        .bind(user_id)
        .bind(bear_id)
        .fetch_one(pool)
        .await?;
        token_owner_is_bear_member = member.0;
    }

    let mut failure_reasons = Vec::new();
    if !token_prefix_ok {
        failure_reasons.push("token does not start with bear_arm_".to_string());
    }
    if !token_found {
        failure_reasons.push("token hash was not found in this Den database".to_string());
    }
    if token_found && !token_active {
        failure_reasons.push("token is revoked or expired".to_string());
    }
    if !bear_found {
        failure_reasons.push("bear slug does not exist in this Den database".to_string());
    }
    if token_found && bear_found && !token_bound_to_bear {
        failure_reasons.push("token is not granted to this Bear".to_string());
    }
    if token_found && bear_found && token_bound_to_bear && !token_owner_is_bear_member {
        failure_reasons.push("token owner is no longer a member of this Bear".to_string());
    }
    if token_found && !required_scope_present {
        failure_reasons.push(format!("token is missing required scope {required_scope}"));
    }

    let ok = token_prefix_ok
        && token_found
        && token_active
        && bear_found
        && token_bound_to_bear
        && token_owner_is_bear_member
        && required_scope_present;

    Ok(ArmatureTokenDiagnostics {
        bear_slug: bear_slug.to_string(),
        required_scope: required_scope.to_string(),
        token_prefix_ok,
        token_found,
        token_active,
        bear_found,
        token_bound_to_bear,
        token_owner_is_bear_member,
        required_scope_present,
        ok,
        failure_reasons,
    })
}

#[derive(Debug, Clone)]
pub struct ArmatureTokenAuth {
    pub token_id: Uuid,
    pub user_id: i32,
    pub scopes: serde_json::Value,
}

pub async fn authenticate_for_bear_slug_with_scopes(
    pool: &PgPool,
    raw_token: &str,
    bear_slug: &str,
) -> Result<Option<ArmatureTokenAuth>, CustomError> {
    let hash = token_hash(raw_token);
    let row: Option<(Uuid, i32, serde_json::Value)> = sqlx::query_as(
        r"
        SELECT t.id, t.user_id, t.scopes
        FROM armature_tokens t
        INNER JOIN armature_token_bears tb ON tb.token_id = t.id
        INNER JOIN bears b ON b.id = tb.bear_id
        INNER JOIN user_bear ub ON ub.user_id = t.user_id AND ub.bear_id = b.id
        WHERE t.token_hash = $1
          AND b.slug = $2
          AND t.revoked_at IS NULL
          AND (t.expires_at IS NULL OR t.expires_at > NOW())
        ",
    )
    .bind(hash)
    .bind(bear_slug)
    .fetch_optional(pool)
    .await?;

    let Some((token_id, user_id, scopes)) = row else {
        return Ok(None);
    };

    sqlx::query("UPDATE armature_tokens SET last_used_at = NOW() WHERE id = $1")
        .bind(token_id)
        .execute(pool)
        .await?;

    Ok(Some(ArmatureTokenAuth {
        token_id,
        user_id,
        scopes,
    }))
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
    if scopes_contains(scopes, ARMATURE_CHAT_SCOPE) {
        labels.push("chat".to_string());
    }
    if scopes_contains(scopes, ARMATURE_TOOLS_SCOPE) {
        labels.push("tools".to_string());
    }

    labels
}
