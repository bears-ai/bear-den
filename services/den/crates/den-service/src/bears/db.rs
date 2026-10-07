//! SQL for bears and `user_bear`. Raw default model values are compatibility
//! projections; model configuration writes go through the canonical bridge.

use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

use den_core::{AgentLoopControlLevel, DenError};

use super::model::{
    Bear, BearProfileBinding, BearSkillManifestEntry, BearSkillProposal, BearWithMembership,
    RuntimeContextLabel,
};

pub struct BearParams<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub system_prompt: &'a str,
    pub default_model: Option<&'a str>,
    pub tools_enabled: Option<Json<serde_json::Value>>,
    pub context_profile: Option<Json<serde_json::Value>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct BearProfileModelSetting {
    pub bear_id: Uuid,
    pub profile: String,
    pub model: Option<String>,
    pub agent_loop_control_level: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct BearBifrostVirtualKey {
    pub bear_id: Uuid,
    pub virtual_key_id: Option<String>,
    pub virtual_key_name: Option<String>,
    pub virtual_key_value: Option<String>,
    pub virtual_key_value_encrypted: Option<String>,
}

/// Flat SQLx macro target for `BearWithMembership`.
///
/// `query_as!` constructs struct literals and cannot populate the flattened
/// `BearWithMembership::bear` field directly.
struct BearWithMembershipRow {
    id: Uuid,
    slug: String,
    name: String,
    description: String,
    default_model: Option<String>,
    default_tool_budget_multiplier: Option<f64>,
    tools_enabled: Option<Json<serde_json::Value>>,
    work_enabled: bool,
    cabinet_enabled: bool,
    runtime_plan: Option<Json<serde_json::Value>>,
    context_profile: Option<Json<serde_json::Value>>,
    provisioning_version: i32,
    system_prompt: String,
    birthday: Option<time::Date>,
    created_at: time::OffsetDateTime,
    updated_at: time::OffsetDateTime,
    live_reflection_enabled: bool,
    live_reflection_stale_after_minutes: i32,
    live_reflection_activity_threshold: i32,
    live_reflection_sweep_limit: i32,
    membership_role: Option<String>,
}

impl From<BearWithMembershipRow> for BearWithMembership {
    fn from(row: BearWithMembershipRow) -> Self {
        Self {
            bear: Bear {
                id: row.id,
                slug: row.slug,
                name: row.name,
                description: row.description,
                default_model: row.default_model,
                default_tool_budget_multiplier: row.default_tool_budget_multiplier,
                tools_enabled: row.tools_enabled,
                work_enabled: row.work_enabled,
                cabinet_enabled: row.cabinet_enabled,
                runtime_plan: row.runtime_plan,
                context_profile: row.context_profile,
                provisioning_version: row.provisioning_version,
                system_prompt: row.system_prompt,
                birthday: row.birthday,
                created_at: row.created_at,
                updated_at: row.updated_at,
                live_reflection_enabled: row.live_reflection_enabled,
                live_reflection_stale_after_minutes: row.live_reflection_stale_after_minutes,
                live_reflection_activity_threshold: row.live_reflection_activity_threshold,
                live_reflection_sweep_limit: row.live_reflection_sweep_limit,
            },
            membership_role: row.membership_role,
        }
    }
}

pub async fn list_bears(pool: &PgPool) -> Result<Vec<Bear>, DenError> {
    sqlx::query_as!(
        Bear,
        r#"
        SELECT id, slug, name, description, default_model, default_tool_budget_multiplier,
               tools_enabled AS "tools_enabled?: Json<serde_json::Value>",
               work_enabled, cabinet_enabled, runtime_plan AS "runtime_plan?: Json<serde_json::Value>",
               context_profile AS "context_profile?: Json<serde_json::Value>",
               provisioning_version, system_prompt, birthday, created_at, updated_at,
               live_reflection_enabled, live_reflection_stale_after_minutes,
               live_reflection_activity_threshold, live_reflection_sweep_limit
        FROM bears
        ORDER BY slug
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn get_bear(pool: &PgPool, id: Uuid) -> Result<Option<Bear>, DenError> {
    sqlx::query_as!(
        Bear,
        r#"
        SELECT id, slug, name, description, default_model, default_tool_budget_multiplier,
               tools_enabled AS "tools_enabled?: Json<serde_json::Value>",
               work_enabled, cabinet_enabled, runtime_plan AS "runtime_plan?: Json<serde_json::Value>",
               context_profile AS "context_profile?: Json<serde_json::Value>",
               provisioning_version, system_prompt, birthday, created_at, updated_at,
               live_reflection_enabled, live_reflection_stale_after_minutes,
               live_reflection_activity_threshold, live_reflection_sweep_limit
        FROM bears
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn bear_slug_exists(pool: &PgPool, slug: &str) -> Result<bool, DenError> {
    let n = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!\" FROM bears WHERE slug = $1",
        slug
    )
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn bear_slug_exists_excluding(
    pool: &PgPool,
    slug: &str,
    exclude_id: Uuid,
) -> Result<bool, DenError> {
    let n = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!\" FROM bears WHERE slug = $1 AND id <> $2",
        slug,
        exclude_id
    )
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn update_bear(pool: &PgPool, id: Uuid, params: BearParams<'_>) -> Result<(), DenError> {
    let tools_enabled = params.tools_enabled.map(|Json(value)| value);
    let mut transaction = pool.begin().await?;
    let r = sqlx::query!(
        r"
        UPDATE bears
        SET slug = $1,
            name = $2,
            description = $3,
            system_prompt = $4,
            tools_enabled = $5,
            updated_at = NOW()
        WHERE id = $6
        ",
        params.slug,
        params.name,
        params.description,
        params.system_prompt,
        tools_enabled,
        id
    )
    .execute(&mut *transaction)
    .await?;
    if r.rows_affected() == 0 {
        return Err(DenError::NotFound("bear not found".to_string()));
    }
    super::model_configurations::compatibility::set_legacy_default(
        &mut transaction,
        id.into(),
        params.default_model,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Creates a logical Bear row. Profile runtime bindings live in `bear_profile_bindings`.
pub async fn create_bear(pool: &PgPool, params: BearParams<'_>) -> Result<Uuid, DenError> {
    create_bear_with_context_profile(pool, params).await
}

/// Creates a logical Bear row with optional profile-aware context composition profile.
pub async fn create_bear_with_context_profile(
    pool: &PgPool,
    params: BearParams<'_>,
) -> Result<Uuid, DenError> {
    let tools_enabled = params.tools_enabled.map(|Json(value)| value);
    let context_profile = params.context_profile.map(|Json(value)| value);
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar!(
        r"
        INSERT INTO bears (
            slug, name, description, system_prompt, tools_enabled, context_profile
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        ",
        params.slug,
        params.name,
        params.description,
        params.system_prompt,
        tools_enabled,
        context_profile,
    )
    .fetch_one(&mut *transaction)
    .await?;
    super::model_configurations::compatibility::set_legacy_default(
        &mut transaction,
        id.into(),
        params.default_model,
    )
    .await?;
    transaction.commit().await?;
    Ok(id)
}

pub async fn update_bear_context_profile(
    pool: &PgPool,
    id: Uuid,
    context_profile: Option<Json<serde_json::Value>>,
    system_prompt: &str,
) -> Result<(), DenError> {
    let context_profile = context_profile.map(|Json(value)| value);
    let r = sqlx::query!(
        r"
        UPDATE bears
        SET context_profile = $1,
            system_prompt = $2,
            updated_at = NOW()
        WHERE id = $3
        ",
        context_profile,
        system_prompt,
        id
    )
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(DenError::NotFound("bear not found".to_string()));
    }
    Ok(())
}

/// Canonical role for users who manage membership and bear settings (not site `users.is_admin`).
pub const BEAR_ROLE_ADMIN: &str = "admin";
pub const BEAR_ROLE_MEMBER: &str = "member";

#[inline]
pub fn role_is_bear_admin(role: Option<&str>) -> bool {
    matches!(
        role.map(|s| s.trim().eq_ignore_ascii_case(BEAR_ROLE_ADMIN)),
        Some(true)
    )
}

pub async fn grant_membership(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    role: Option<&str>,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO user_bear (user_id, bear_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, bear_id) DO UPDATE SET role = EXCLUDED.role
        ",
        user_id,
        bear_id,
        role
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn revoke_membership(pool: &PgPool, user_id: i32, bear_id: Uuid) -> Result<(), DenError> {
    let r = sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        user_id,
        bear_id
    )
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(DenError::NotFound("membership not found".to_string()));
    }
    Ok(())
}

pub async fn delete_bear(pool: &PgPool, bear_id: Uuid) -> Result<(), DenError> {
    let r = sqlx::query!("DELETE FROM bears WHERE id = $1", bear_id)
        .execute(pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DenError::NotFound("bear not found".to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct BearMemberRow {
    pub user_id: i32,
    pub username: String,
    pub display_name: String,
    pub role: Option<String>,
}

pub async fn list_members_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearMemberRow>, DenError> {
    sqlx::query_as!(
        BearMemberRow,
        r"
        SELECT ub.user_id, u.username, u.display_name, ub.role
        FROM user_bear ub
        INNER JOIN users u ON u.id = ub.user_id
        WHERE ub.bear_id = $1
        ORDER BY
            CASE WHEN lower(btrim(coalesce(ub.role, ''))) = 'admin' THEN 0 ELSE 1 END,
            u.username
        ",
        bear_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn count_bear_admins(pool: &PgPool, bear_id: Uuid) -> Result<i64, DenError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)::bigint AS "count!"
        FROM user_bear
        WHERE bear_id = $1
          AND lower(btrim(coalesce(role, ''))) = 'admin'
        "#,
        bear_id
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn membership_role_for_user(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
) -> Result<Option<Option<String>>, DenError> {
    sqlx::query_scalar!(
        "SELECT role FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        user_id,
        bear_id
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct MembershipRow {
    pub user_id: i32,
    pub username: String,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub bear_name: String,
    pub role: Option<String>,
}

pub async fn list_memberships(pool: &PgPool) -> Result<Vec<MembershipRow>, DenError> {
    sqlx::query_as!(
        MembershipRow,
        r"
        SELECT ub.user_id, u.username, ub.bear_id, b.slug AS bear_slug, b.name AS bear_name, ub.role
        FROM user_bear ub
        INNER JOIN users u ON u.id = ub.user_id
        INNER JOIN bears b ON b.id = ub.bear_id
        ORDER BY u.username, b.slug
        ",
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_bears_for_user(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<BearWithMembership>, DenError> {
    sqlx::query_as!(BearWithMembershipRow,
        r#"
        SELECT b.id, b.slug, b.name, b.description, b.default_model, b.default_tool_budget_multiplier,
               b.tools_enabled AS "tools_enabled?: Json<serde_json::Value>",
               b.work_enabled, b.cabinet_enabled, b.runtime_plan AS "runtime_plan?: Json<serde_json::Value>",
               b.context_profile AS "context_profile?: Json<serde_json::Value>",
               b.provisioning_version, b.system_prompt, b.birthday, b.created_at, b.updated_at,
               b.live_reflection_enabled, b.live_reflection_stale_after_minutes,
               b.live_reflection_activity_threshold, b.live_reflection_sweep_limit,
               ub.role AS membership_role
        FROM bears b
        INNER JOIN user_bear ub ON ub.bear_id = b.id
        WHERE ub.user_id = $1
        ORDER BY b.slug
        "#,
    user_id)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(Into::into).collect())
    .map_err(Into::into)
}

/// Bear visible to the user via `user_bear`, keyed by slug (for `/bear/{slug}`).
pub async fn bear_for_user_by_slug(
    pool: &PgPool,
    user_id: i32,
    slug: &str,
) -> Result<Option<Bear>, DenError> {
    sqlx::query_as!(Bear,
        r#"
        SELECT b.id, b.slug, b.name, b.description, b.default_model, b.default_tool_budget_multiplier,
               b.tools_enabled AS "tools_enabled?: Json<serde_json::Value>",
               b.work_enabled, b.cabinet_enabled, b.runtime_plan AS "runtime_plan?: Json<serde_json::Value>",
               b.context_profile AS "context_profile?: Json<serde_json::Value>",
               b.provisioning_version, b.system_prompt, b.birthday, b.created_at, b.updated_at,
               b.live_reflection_enabled, b.live_reflection_stale_after_minutes,
               b.live_reflection_activity_threshold, b.live_reflection_sweep_limit
        FROM bears b
        INNER JOIN user_bear ub ON ub.bear_id = b.id
        WHERE ub.user_id = $1 AND b.slug = $2
        "#,
    user_id, slug)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn count_bear_members(pool: &PgPool, bear_id: Uuid) -> Result<i64, DenError> {
    let n = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!\" FROM user_bear WHERE bear_id = $1",
        bear_id
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn user_may_use_bear(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
) -> Result<bool, DenError> {
    let n = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)::bigint AS "count!" FROM user_bear WHERE user_id = $1 AND bear_id = $2
        "#,
        user_id,
        bear_id
    )
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn ensure_bear_profile_binding_rows(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<(), DenError> {
    for profile in RuntimeContextLabel::ALL {
        sqlx::query!(
            r"
            INSERT INTO bear_profile_bindings (bear_id, profile, binding_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (bear_id, profile) DO NOTHING
            ",
            bear_id,
            profile.as_str(),
            format!("den-native:{bear_id}:{}", profile.as_str())
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn list_bear_profile_bindings(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearProfileBinding>, DenError> {
    sqlx::query_as!(
        BearProfileBinding,
        r#"
        SELECT bear_id, profile, binding_id, provisioning_status,
               last_provisioned_version, last_synced_at, last_provisioning_error,
               config_hash AS "config_hash?: Json<serde_json::Value>",
               created_at, updated_at
        FROM bear_profile_bindings
        WHERE bear_id = $1
        ORDER BY CASE profile
            WHEN 'chat' THEN 1
            WHEN 'pair' THEN 2
            WHEN 'curate' THEN 3
            WHEN 'work' THEN 4
            WHEN 'watch' THEN 5
            ELSE 99
        END
        "#,
        bear_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn get_bear_profile_binding(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
) -> Result<Option<BearProfileBinding>, DenError> {
    sqlx::query_as!(
        BearProfileBinding,
        r#"
        SELECT bear_id, profile, binding_id, provisioning_status,
               last_provisioned_version, last_synced_at, last_provisioning_error,
               config_hash AS "config_hash?: Json<serde_json::Value>",
               created_at, updated_at
        FROM bear_profile_bindings
        WHERE bear_id = $1 AND profile = $2
        "#,
        bear_id,
        profile.as_str()
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Returns the canonical Den-owned runtime binding id for a Bear profile.
pub async fn profile_binding_id(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
) -> Result<Option<String>, DenError> {
    sqlx::query_scalar!(
        r"
        SELECT binding_id
        FROM bear_profile_bindings
        WHERE bear_id = $1 AND profile = $2
        ",
        bear_id,
        profile.as_str()
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn mark_bear_profile_binding_provisioning(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO bear_profile_bindings (bear_id, profile, binding_id, provisioning_status, updated_at)
        VALUES ($1, $2, $3, 'provisioning', NOW())
        ON CONFLICT (bear_id, profile)
        DO UPDATE SET provisioning_status = 'provisioning',
                      last_provisioning_error = NULL,
                      updated_at = NOW()
        ",
    bear_id, profile.as_str(), format!("den-native:{bear_id}:{}", profile.as_str()))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_bear_profile_binding_ready(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    binding_id: &str,
    version: i32,
    config_hash: &serde_json::Value,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO bear_profile_bindings (
            bear_id, profile, binding_id, provisioning_status,
            last_provisioned_version, last_synced_at, last_provisioning_error, config_hash, updated_at
        )
        VALUES ($1, $2, $3, 'ready', $4, NOW(), NULL, $5::jsonb, NOW())
        ON CONFLICT (bear_id, profile)
        DO UPDATE SET binding_id = EXCLUDED.binding_id,
                      provisioning_status = 'ready',
                      last_provisioned_version = EXCLUDED.last_provisioned_version,
                      last_synced_at = NOW(),
                      last_provisioning_error = NULL,
                      config_hash = EXCLUDED.config_hash,
                      updated_at = NOW()
        ",
    bear_id, profile.as_str(), binding_id, version, config_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_bear_profile_binding_synced(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    version: i32,
    config_hash: &serde_json::Value,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        UPDATE bear_profile_bindings
        SET provisioning_status = 'ready',
            last_provisioned_version = $3,
            last_synced_at = NOW(),
            last_provisioning_error = NULL,
            config_hash = $4::jsonb,
            updated_at = NOW()
        WHERE bear_id = $1 AND profile = $2
        ",
        bear_id,
        profile.as_str(),
        version,
        config_hash
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_bear_profile_binding_failed(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    message: &str,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO bear_profile_bindings (
            bear_id, profile, binding_id, provisioning_status, last_provisioning_error, updated_at
        )
        VALUES ($1, $2, $3, 'failed', $4, NOW())
        ON CONFLICT (bear_id, profile)
        DO UPDATE SET provisioning_status = 'failed',
                      last_provisioning_error = EXCLUDED.last_provisioning_error,
                      updated_at = NOW()
        ",
        bear_id,
        profile.as_str(),
        format!("den-native:{bear_id}:{}", profile.as_str()),
        message
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_bear_skills(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearSkillManifestEntry>, DenError> {
    sqlx::query_as!(
        BearSkillManifestEntry,
        r"
        SELECT bear_id, skill_name, skill_version, source, content_hash, applies_to_profiles,
               installed_at, last_verified_at, created_at, updated_at
        FROM bear_skills_manifest
        WHERE bear_id = $1
        ORDER BY skill_name, skill_version
        ",
        bear_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn propose_skill(
    pool: &PgPool,
    bear_id: Uuid,
    proposed_by_agent_id: &str,
    skill_payload: &serde_json::Value,
) -> Result<Uuid, DenError> {
    let id = sqlx::query_scalar!(
        r"
        INSERT INTO bear_skill_proposals (bear_id, proposed_by_agent_id, skill_payload)
        VALUES ($1, $2, $3::jsonb)
        RETURNING id
        ",
        bear_id,
        proposed_by_agent_id,
        skill_payload
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn list_pending_skill_proposals(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearSkillProposal>, DenError> {
    sqlx::query_as!(
        BearSkillProposal,
        r"
        SELECT bear_id, id, proposed_by_agent_id, proposed_at, skill_payload, status,
               reviewed_at, rejection_reason, resulting_manifest_bear_id,
               resulting_manifest_skill_name, resulting_manifest_skill_version, updated_at
        FROM bear_skill_proposals
        WHERE bear_id = $1 AND status = 'pending_review'
        ORDER BY proposed_at, id
        ",
        bear_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Seed `runtime_plan` once so callers have a BearRuntimePlan v1 snapshot.
pub async fn ensure_default_runtime_plan(
    pool: &PgPool,
    bear_id: Uuid,
    default_json: &serde_json::Value,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        UPDATE bears
        SET runtime_plan = $2::jsonb,
            updated_at = NOW()
        WHERE id = $1
          AND runtime_plan IS NULL
        ",
        bear_id,
        default_json
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_bear_bifrost_virtual_key(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<BearBifrostVirtualKey>, DenError> {
    match sqlx::query_as!(BearBifrostVirtualKey,
        r"
        SELECT bear_id, virtual_key_id, virtual_key_name, virtual_key_value, virtual_key_value_encrypted
        FROM bear_bifrost_virtual_keys
        WHERE bear_id = $1
        ",
    bear_id)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => Ok(row),
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("42P01") => Ok(None),
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("42703") => {
            sqlx::query_as!(BearBifrostVirtualKey,
                r"
                SELECT bear_id, virtual_key_id, virtual_key_name, virtual_key_value, NULL::TEXT AS virtual_key_value_encrypted
                FROM bear_bifrost_virtual_keys
                WHERE bear_id = $1
                ",
            bear_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into)
        }
        Err(err) => Err(err.into()),
    }
}

pub async fn bifrost_virtual_key_for_inference(
    pool: &PgPool,
    bear_id: Uuid,
    secret_encryption_key: &str,
) -> Result<Option<String>, DenError> {
    let Some(row) = get_bear_bifrost_virtual_key(pool, bear_id).await? else {
        return Ok(None);
    };
    bifrost_virtual_key_for_inference_from_row(row, secret_encryption_key)
}

fn bifrost_virtual_key_for_inference_from_row(
    row: BearBifrostVirtualKey,
    secret_encryption_key: &str,
) -> Result<Option<String>, DenError> {
    bifrost_virtual_key_secret_from_row(row, secret_encryption_key)
}

pub async fn bifrost_virtual_key_value_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    secret_encryption_key: &str,
) -> Result<Option<String>, DenError> {
    let Some(row) = get_bear_bifrost_virtual_key(pool, bear_id).await? else {
        return Ok(None);
    };
    bifrost_virtual_key_secret_from_row(row, secret_encryption_key)
}

fn bifrost_virtual_key_secret_from_row(
    row: BearBifrostVirtualKey,
    secret_encryption_key: &str,
) -> Result<Option<String>, DenError> {
    if let Some(encrypted) = row
        .virtual_key_value_encrypted
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return crate::secrets::decrypt_secret(encrypted, secret_encryption_key).map(Some);
    }
    Ok(row
        .virtual_key_value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

pub async fn set_bear_bifrost_virtual_key(
    pool: &PgPool,
    bear_id: Uuid,
    virtual_key_id: Option<&str>,
    virtual_key_name: Option<&str>,
    virtual_key_value: Option<&str>,
    secret_encryption_key: &str,
) -> Result<(), DenError> {
    let virtual_key_id = virtual_key_id.map(str::trim).filter(|s| !s.is_empty());
    let virtual_key_name = virtual_key_name.map(str::trim).filter(|s| !s.is_empty());
    let virtual_key_value = virtual_key_value.map(str::trim).filter(|s| !s.is_empty());
    let virtual_key_value_encrypted = virtual_key_value
        .map(|value| crate::secrets::encrypt_secret(value, secret_encryption_key))
        .transpose()?;
    if virtual_key_id.is_none()
        && virtual_key_name.is_none()
        && virtual_key_value_encrypted.is_none()
    {
        clear_bear_bifrost_virtual_key(pool, bear_id).await?;
        return Ok(());
    }
    sqlx::query!(
        r"
        INSERT INTO bear_bifrost_virtual_keys (
            bear_id, virtual_key_id, virtual_key_name, virtual_key_value, virtual_key_value_encrypted, updated_at
        )
        VALUES ($1, $2, $3, NULL, $4, NOW())
        ON CONFLICT (bear_id) DO UPDATE
        SET virtual_key_id = EXCLUDED.virtual_key_id,
            virtual_key_name = EXCLUDED.virtual_key_name,
            virtual_key_value = NULL,
            virtual_key_value_encrypted = EXCLUDED.virtual_key_value_encrypted,
            updated_at = NOW()
        ",
    bear_id, virtual_key_id, virtual_key_name, virtual_key_value_encrypted)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_bear_bifrost_virtual_key_metadata(
    pool: &PgPool,
    bear_id: Uuid,
    virtual_key_id: Option<&str>,
    virtual_key_name: Option<&str>,
) -> Result<(), DenError> {
    let virtual_key_id = virtual_key_id.map(str::trim).filter(|s| !s.is_empty());
    let virtual_key_name = virtual_key_name.map(str::trim).filter(|s| !s.is_empty());
    if virtual_key_id.is_none() && virtual_key_name.is_none() {
        return Ok(());
    }
    sqlx::query!(
        r"
        INSERT INTO bear_bifrost_virtual_keys (
            bear_id, virtual_key_id, virtual_key_name, updated_at
        )
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (bear_id) DO UPDATE
        SET virtual_key_id = EXCLUDED.virtual_key_id,
            virtual_key_name = EXCLUDED.virtual_key_name,
            updated_at = NOW()
        ",
        bear_id,
        virtual_key_id,
        virtual_key_name
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_bear_bifrost_virtual_key(pool: &PgPool, bear_id: Uuid) -> Result<(), DenError> {
    sqlx::query!(
        "DELETE FROM bear_bifrost_virtual_keys WHERE bear_id = $1",
        bear_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_profile_model_settings(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearProfileModelSetting>, DenError> {
    sqlx::query_as!(
        BearProfileModelSetting,
        r"
        SELECT bear_id, profile, model, agent_loop_control_level
        FROM bear_profile_model_settings
        WHERE bear_id = $1
        ORDER BY profile
        ",
        bear_id
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Historical profile metadata only; never a primary-model configuration or
/// hat override. Retained for legacy record compatibility, not runtime routing.
pub async fn set_profile_model_setting(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    model: Option<&str>,
) -> Result<(), DenError> {
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    sqlx::query!(
        r"
        INSERT INTO bear_profile_model_settings (bear_id, profile, model, updated_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (bear_id, profile) DO UPDATE
        SET model = EXCLUDED.model,
            updated_at = NOW()
        ",
        bear_id,
        profile.as_str(),
        model
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn bear_agent_loop_control_setting(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<AgentLoopControlLevel>, DenError> {
    let raw = sqlx::query_scalar!(
        r"
        SELECT default_agent_loop_control_level
        FROM bears
        WHERE id = $1
        ",
        bear_id
    )
    .fetch_optional(pool)
    .await?
    .flatten();
    parse_agent_loop_control_setting(raw.as_deref())
}

pub async fn set_bear_agent_loop_control_setting(
    pool: &PgPool,
    bear_id: Uuid,
    level: Option<AgentLoopControlLevel>,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        UPDATE bears
        SET default_agent_loop_control_level = $2,
            updated_at = NOW()
        WHERE id = $1
        ",
        bear_id,
        level.map(AgentLoopControlLevel::as_str)
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_bear_tool_budget_multiplier(
    pool: &PgPool,
    bear_id: Uuid,
    multiplier: Option<f64>,
) -> Result<(), DenError> {
    let multiplier = match multiplier {
        Some(value) if value.is_finite() && value > 0.0 && value <= 10.0 => Some(value),
        Some(_) => {
            return Err(DenError::ValidationError(
                "tool budget multiplier must be in (0, 10]".to_string(),
            ))
        }
        None => None,
    };
    sqlx::query!(
        r"
        UPDATE bears
        SET default_tool_budget_multiplier = $2,
            updated_at = NOW()
        WHERE id = $1
        ",
        bear_id,
        multiplier
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_profile_agent_loop_control_setting(
    pool: &PgPool,
    bear_id: Uuid,
    profile: RuntimeContextLabel,
    level: Option<AgentLoopControlLevel>,
) -> Result<(), DenError> {
    sqlx::query!(
        r"
        INSERT INTO bear_profile_model_settings (
            bear_id, profile, agent_loop_control_level, updated_at
        )
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (bear_id, profile) DO UPDATE
        SET agent_loop_control_level = EXCLUDED.agent_loop_control_level,
            updated_at = NOW()
        ",
        bear_id,
        profile.as_str(),
        level.map(AgentLoopControlLevel::as_str)
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn parse_agent_loop_control_setting(
    value: Option<&str>,
) -> Result<Option<AgentLoopControlLevel>, DenError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    match value {
        "light" => Ok(Some(AgentLoopControlLevel::Light)),
        "standard" => Ok(Some(AgentLoopControlLevel::Standard)),
        "careful" => Ok(Some(AgentLoopControlLevel::Careful)),
        "strict" => Ok(Some(AgentLoopControlLevel::Strict)),
        other => Err(DenError::ValidationError(format!(
            "unsupported agent loop control level: {other}"
        ))),
    }
}

/// Legacy projection reader for callers awaiting cutover. This cannot resolve
/// hats/effort or revalidate catalog membership; execution must use
/// `model_configurations::resolve_primary` instead.
pub fn resolve_model_for_bear(bear: &Bear, system_default_model: &str) -> String {
    resolve_model_from_values(bear.default_model.as_deref(), system_default_model)
}

fn resolve_model_from_values(
    bear_default_model: Option<&str>,
    system_default_model: &str,
) -> String {
    bear_default_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| system_default_model.trim())
        .to_string()
}

pub async fn update_live_reflection_enabled(
    pool: &PgPool,
    bear_id: Uuid,
    enabled: bool,
) -> Result<(), DenError> {
    update_live_reflection_settings(pool, bear_id, enabled, 30, 20, 25).await
}

pub async fn update_live_reflection_settings(
    pool: &PgPool,
    bear_id: Uuid,
    enabled: bool,
    stale_after_minutes: i32,
    activity_threshold: i32,
    sweep_limit: i32,
) -> Result<(), DenError> {
    let stale_after_minutes = stale_after_minutes.clamp(1, 1440);
    let activity_threshold = activity_threshold.clamp(1, 1000);
    let sweep_limit = sweep_limit.clamp(1, 100);
    let result = sqlx::query!(
        r"
        UPDATE bears
        SET live_reflection_enabled = $2,
            live_reflection_stale_after_minutes = $3,
            live_reflection_activity_threshold = $4,
            live_reflection_sweep_limit = $5,
            updated_at = NOW()
        WHERE id = $1
        ",
        bear_id,
        enabled,
        stale_after_minutes,
        activity_threshold,
        sweep_limit
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DenError::NotFound("bear not found".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod model_setting_tests;
