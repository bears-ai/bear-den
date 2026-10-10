//! Bear-scoped settings at `/bear/{slug}/…`: members can view shared settings;
//! inspection of raw activity, context, and diagnostics requires a Bear admin.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use axum_login::tower_sessions::Session;

use minijinja::context;
use serde::{Deserialize, Serialize};
use serde_json::json;

use std::path::{Path as FsPath, PathBuf};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    auth_backend::{AuthSession, SessionUser},
    core::web_policy,
    errors::CustomError,
    web::{self, AppState},
};
use den_core::{ids::BearId, AgentLoopControlLevel};
use den_memory::{bear_memory_admin_stats, BearMemoryAdminStats};
use den_protocol::ContextBudgetReport;

use den_service::prompt_memory_block_store::list_prompt_memory_blocks_for_bear_profile;
use den_service::recall::recall_watermark_for_bear;
use den_service::{
    bears::{
        context_profile_from_json, db as bears_db, db::BEAR_ROLE_MEMBER, get_compiled_bear_config,
        hats, managed_blocks::BearCompiledConfigRow,
    },
    conversation::persistence::{self as conversation_persistence, list_messages_page},
};

use crate::web::admin::bears::{
    bear_plan_mode_rows, bear_web_approvals, bear_web_fetches, bear_web_sources,
    AddWebApprovalForm, AddWebSourceForm, BearPlanModeRow, BearWebApprovalRow, BearWebFetchRow,
    BearWebSourceRow,
};
use crate::web::bear::create_support::{bear_slug_base, provision_bifrost_virtual_key_for_bear};

use super::member::{email_verify_redirect, load_bear_member, viewer_can_manage_bear};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(model_configurations::router())
        .merge(import_review::router())
        .route_with_tsr("/bear/{slug}/overview", get(overview_view))
        .route_with_tsr("/bear/{slug}/people", get(access_view))
        .route_with_tsr("/bear/{slug}/persona", get(persona_view))
        .route_with_tsr("/bear/{slug}/stances", get(stances_list_redirect))
        .route_with_tsr("/bear/{slug}/profiles", get(stances_list_redirect))
        .route_with_tsr(
            "/bear/{slug}/models",
            get(models_view).post(advanced_models::save),
        )
        .route_with_tsr(
            "/bear/{slug}/models/provision-bifrost-key",
            post(provision_bifrost_virtual_key_action),
        )
        .route_with_tsr("/bear/{slug}/activity", get(conversations_view))
        .route_with_tsr("/bear/{slug}/conversations", get(conversations_view))
        .route_with_tsr(
            "/bear/{slug}/conversations/reflect",
            post(reflect_conversations_post),
        )
        .route_with_tsr("/bear/{slug}/reflections", get(reflections_view))
        .route_with_tsr(
            "/bear/{slug}/conversations/{conversation_id}",
            get(conversation_detail_view),
        )
        .route_with_tsr(
            "/bear/{slug}/conversations/{conversation_id}/reflect",
            post(reflect_conversation_post),
        )
        .route_with_tsr(
            "/bear/{slug}/conversations/{conversation_id}/reconsider",
            post(reconsider_conversation_post),
        )
        .route_with_tsr("/bear/{slug}/context", get(context_view))
        .route_with_tsr("/bear/{slug}/resources", get(policy_view))
        .route_with_tsr("/bear/{slug}/advanced", get(advanced_view))
        .route_with_tsr(
            "/bear/{slug}/advanced/live-reflection",
            post(live_reflection_post),
        )
        .route_with_tsr("/bear/{slug}/export.bear", get(export_bear_bundle))
        .route_with_tsr("/bear/{slug}/members/grant", post(people::grant))
        .route_with_tsr(
            "/bear/{slug}/members/{user_id}/revoke",
            post(people::revoke),
        )
        .route_with_tsr("/bear/{slug}/web-sources", post(add_web_source_action))
        .route_with_tsr(
            "/bear/{slug}/web-sources/{source_id}/delete",
            post(delete_web_source_action),
        )
        .route_with_tsr("/bear/{slug}/web-approvals", post(add_web_approval_action))
        .route_with_tsr(
            "/bear/{slug}/web-approvals/{approval_id}/revoke",
            post(revoke_web_approval_action),
        )
}

#[derive(Debug, Deserialize)]
struct DomainQuery {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct BearModelsForm {
    #[serde(default)]
    bear_tool_budget_multiplier: String,
    #[serde(default)]
    bear_loop_control: String,
    #[serde(default)]
    bifrost_virtual_key_id: String,
    #[serde(default)]
    bifrost_virtual_key_name: String,
    #[serde(default)]
    bifrost_virtual_key_value: String,
    #[serde(default)]
    bifrost_virtual_key_clear: String,
}

#[derive(Debug, Deserialize)]
struct LiveReflectionForm {
    #[serde(default)]
    enabled: String,
    #[serde(default)]
    stale_after_minutes: i32,
    #[serde(default)]
    activity_threshold: i32,
    #[serde(default)]
    sweep_limit: i32,
}

#[derive(Debug, Deserialize)]
struct ReflectConversationsForm {
    #[serde(default)]
    conversation_ids: Vec<Uuid>,
    #[serde(default)]
    bulk_action: String,
}

#[derive(Debug, Default, Serialize)]
struct ManualReflectionResult {
    compaction_applied: bool,
    compaction_skipped: bool,
    compaction_status: String,
    compaction_diagnostic: Option<String>,
    compaction_artifact_json: String,
    candidate_count: usize,
    discarded_count: usize,
    discarded_reasons: Vec<String>,
    dropped_followup_count: usize,
    proposals_created: usize,
    proposal_ids: Vec<Uuid>,
    skipped_reason: Option<&'static str>,
    source_message_start_seq: Option<i64>,
    source_message_end_seq: Option<i64>,
    reflection_event_id: Option<Uuid>,
    reflection_event_sequence_no: Option<i64>,
    reflection_payload_json: String,
    failed_stage: Option<manual_reflection::Stage>,
    error: Option<String>,
    observability_error: Option<String>,
    proposals_complete: bool,
    needs_attention: bool,
}

#[derive(Debug, Serialize)]
struct BifrostUsageBudgetRow {
    scope: String,
    max_limit: String,
    current_usage: String,
    remaining: String,
    reset_duration: String,
}

#[derive(Debug, Serialize)]
struct BifrostUsageModelRow {
    model: String,
    provider: String,
    total_requests: String,
    total_tokens: String,
    total_cost: String,
}

#[derive(Debug, Serialize)]
struct BifrostUsageProviderRow {
    provider: String,
    allowed_models: String,
    budget_count: usize,
}

#[derive(Debug, Serialize)]
struct BifrostUsageView {
    status: String,
    error: String,
    virtual_key_name: String,
    is_active: String,
    auth_mode: String,
    budget_rows: Vec<BifrostUsageBudgetRow>,
    model_usage_rows: Vec<BifrostUsageModelRow>,
    provider_rows: Vec<BifrostUsageProviderRow>,
    has_budgets: bool,
    has_model_usage: bool,
    has_providers: bool,
}

const MODELS_FLASH_MESSAGE_KEY: &str = "bear_models_flash_message";
const MODELS_FLASH_ERROR_KEY: &str = "bear_models_flash_error";

const BEAR_BUNDLE_FORMAT: &str = "bear";
const BEAR_BUNDLE_VERSION: u32 = 3;
mod advanced_models;
mod bundle_io;
mod import_creation;
mod import_jobs;
mod import_namespace;
mod import_outcome;
mod import_review;
mod import_staging;
mod manual_reflection;
pub(crate) mod model_configurations;
mod people;
pub(crate) mod portable_hats;
mod portable_models;
use bundle_io::build_bear_bundle;
pub use import_namespace::{cleanup_expired_import_reviews, start_import_staging_cleanup};
use manual_reflection::run as reflect_persisted_conversation;
const BEAR_BUNDLE_MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BearBundleManifest {
    format: String,
    version: u32,
    bear: BearBundleIdentity,
    prompts: BearBundlePrompts,
    #[serde(default)]
    profiles: serde_json::Value,
    #[serde(default)]
    hats: Vec<portable_hats::PortableHat>,
    #[serde(default)]
    ide_default_hat: Option<den_core::ids::HatId>,
    #[serde(default)]
    skills: Vec<den_service::skills::PortableSkill>,
    // Absence identifies legacy raw-default bundles; an empty list explicitly
    // preserves deployment inheritance without invoking the compatibility bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_configurations: Option<Vec<portable_models::PortableModelConfiguration>>,
    #[serde(default)]
    default_model_configuration_id: Option<den_core::ids::ModelConfigurationId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BearBundleIdentity {
    slug: String,
    name: String,
    description: String,
    birthdate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools_enabled: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BearBundlePrompts {
    system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_profile: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MemberGrantForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    user_id: Option<i32>,
    #[serde(default)]
    role: String,
}

impl Default for MemberGrantForm {
    fn default() -> Self {
        Self {
            username: String::new(),
            user_id: None,
            role: BEAR_ROLE_MEMBER.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ConversationAdminRow {
    id: Uuid,
    external_id: String,
    title: String,
    source_session: String,
    updated_at: String,
    latest_context_budget: Option<ContextBudgetReport>,
    latest_context_budget_updated_at: Option<String>,
    latest_context_budget_summary: Option<String>,
}

#[derive(Debug, Serialize)]
struct MessageAdminRow {
    sequence_no: i64,
    message_type: String,
    role: String,
    visibility: String,
    preview: String,
}

#[derive(Clone, Debug, Serialize)]
struct ReflectionAdminRow {
    created_at: String,
    event_type: String,
    session_id: String,
    conversation_id: Option<Uuid>,
    conversation_title: Option<String>,
    trigger: Option<String>,
    status: Option<String>,
    skipped_reason: Option<String>,
    error: Option<String>,
    status_label: String,
    status_explanation: String,
    counts_label: String,
    needs_attention: bool,
    retry_href: Option<String>,
    candidate_count: Option<i64>,
    dropped_followup_count: Option<i64>,
    #[serde(serialize_with = "serialize_proposal_link_count")]
    proposal_count: Option<i64>,
    proposal_links: Vec<String>,
    source_message_start_seq: Option<i64>,
    source_message_end_seq: Option<i64>,
    payload_json: String,
}

// Inspection templates compare this value numerically to decide whether to
// offer a proposal link. Missing extraction totals stay unknown in Rust, the
// counts label and the recorded payload; they imply no recorded links, not zero writes.
fn serialize_proposal_link_count<S: serde::Serializer>(
    count: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_i64(count.unwrap_or(0))
}

#[derive(Debug, Serialize)]
struct LiveReflectionStatusAdmin {
    enabled: bool,
    workers_enabled: bool,
    status_label: String,
    status_explanation: String,
    last_event_at: Option<String>,
    checked_24h: i64,
    processed_24h: i64,
    skipped_24h: i64,
}

#[derive(Debug, Serialize)]
struct ConversationTimelineRow {
    created_at: String,
    kind: String,
    label: String,
    details: String,
}

#[derive(Debug, Serialize)]
struct ReflectionWatermarkAdmin {
    latest_message_sequence_no: Option<i64>,
    last_reflected_at: Option<String>,
    reflected_through_sequence_no: Option<i64>,
    new_message_count: Option<i64>,
    explanation: String,
}

#[derive(Debug, Serialize)]
struct CheckpointArtifactAdminRow {
    run_id: String,
    checkpoint_id: String,
    reason: String,
    control_level: String,
    validation_status: String,
    visibility: String,
    replay_policy: String,
    related_task_list_id: Option<String>,
    related_task_item_id: Option<String>,
    request_json: String,
    response_json: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct CompactionArtifactAdminRow {
    id: Uuid,
    artifact_kind: String,
    policy_version: String,
    trigger: String,
    source_message_start_seq: i64,
    source_message_end_seq: i64,
    source_group_start: Option<i32>,
    source_group_end: Option<i32>,
    artifact_json: String,
    superseded_by: Option<Uuid>,
    created_at: String,
}

fn context_budget_summary(report: &ContextBudgetReport) -> String {
    match report.context_window {
        Some(limit) => format!(
            "{} / {} tokens (reserve {})",
            report.estimated_total_tokens, limit, report.reserved_output_tokens
        ),
        None => format!(
            "{} tokens estimated (reserve {})",
            report.estimated_total_tokens, report.reserved_output_tokens
        ),
    }
}

fn parse_tool_budget_multiplier_form_value(raw: &str) -> Result<Option<f64>, CustomError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("inherit") || raw.eq_ignore_ascii_case("default")
    {
        return Ok(None);
    }
    let value = raw.parse::<f64>().map_err(|_| {
        CustomError::ValidationError("Tool budget multiplier must be a number.".to_string())
    })?;
    if !value.is_finite() || value <= 0.0 || value > 10.0 {
        return Err(CustomError::ValidationError(
            "Tool budget multiplier must be greater than 0 and at most 10.".to_string(),
        ));
    }
    Ok(Some(value))
}

#[derive(Debug, Serialize)]
struct PromptMemoryAdminRow {
    block_id: String,
    scope: String,
    block_type: String,
    state: String,
    title: String,
    body_preview: String,
}

#[derive(Debug, Serialize)]
struct CompiledRolePromptRow {
    role: String,
    prompt_preview: String,
    char_count: usize,
}

pub(crate) fn bear_nav_context(bear: &den_service::bears::Bear, active: &str) -> minijinja::Value {
    context! {
        bear,
        bear_nav_active => active,
    }
}

async fn set_models_flash(session: &Session, message: &str) -> Result<(), CustomError> {
    session
        .insert(MODELS_FLASH_MESSAGE_KEY, message.to_string())
        .await
        .map_err(|err| CustomError::System(format!("could not set models flash message: {err}")))
}

async fn take_models_flash(
    session: &Session,
) -> Result<(Option<String>, Option<String>), CustomError> {
    let message = session
        .get::<String>(MODELS_FLASH_MESSAGE_KEY)
        .await
        .map_err(|err| {
            CustomError::System(format!("could not read models flash message: {err}"))
        })?;
    if message.is_some() {
        session
            .remove::<String>(MODELS_FLASH_MESSAGE_KEY)
            .await
            .map_err(|err| {
                CustomError::System(format!("could not clear models flash message: {err}"))
            })?;
    }

    let error = session
        .get::<String>(MODELS_FLASH_ERROR_KEY)
        .await
        .map_err(|err| CustomError::System(format!("could not read models flash error: {err}")))?;
    if error.is_some() {
        session
            .remove::<String>(MODELS_FLASH_ERROR_KEY)
            .await
            .map_err(|err| {
                CustomError::System(format!("could not clear models flash error: {err}"))
            })?;
    }

    Ok((message, error))
}

pub(crate) async fn session_user(auth_session: &AuthSession) -> Result<&SessionUser, CustomError> {
    auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))
}

pub(crate) async fn load_session_bear(
    state: &AppState,
    auth_session: &AuthSession,
    slug: &str,
) -> Result<Result<(den_service::bears::Bear, bool), Redirect>, CustomError> {
    let user = session_user(auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(Err(r));
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    Ok(Ok((bear, can_manage_bear)))
}

pub(crate) async fn load_session_bear_manage(
    state: &AppState,
    auth_session: &AuthSession,
    slug: &str,
) -> Result<Result<den_service::bears::Bear, Redirect>, CustomError> {
    let (bear, can_manage) = match load_session_bear(state, auth_session, slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(Err(r)),
    };
    if !can_manage {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }
    Ok(Ok(bear))
}

fn memory_sqlite_path(config: &den_core::config::Config, bear_id: Uuid) -> PathBuf {
    FsPath::new(&config.bear_sqlite_data_dir).join(format!("{bear_id}.sqlite"))
}

fn sqlite_string_literal(path: &FsPath) -> Result<String, CustomError> {
    let raw = path
        .to_str()
        .ok_or_else(|| CustomError::System("sqlite path is not valid UTF-8".to_string()))?;
    Ok(format!("'{}'", raw.replace('\'', "''")))
}

fn pretty_json(value: serde_json::Value) -> String {
    // `Value` serialization is expected to be infallible; fall back to compact JSON if the pretty
    // formatter ever errors so the admin page can still render diagnostic payloads.
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

fn json_i64(value: &serde_json::Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
    })
}

fn reflection_counts_label(
    status: Option<&str>,
    skipped_reason: Option<&str>,
    candidate_count: Option<i64>,
    proposal_count: Option<i64>,
) -> String {
    if skipped_reason.is_some() && !matches!(status, Some("failed" | "error")) {
        return "not extracted".into();
    }
    if status == Some("skipped") && candidate_count == Some(0) && proposal_count == Some(0) {
        return "not extracted".into();
    }
    format!(
        "candidates {}, proposals {}",
        candidate_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into()),
        proposal_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into())
    )
}

async fn live_reflection_status_for_bear(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    enabled: bool,
    workers_enabled: bool,
) -> Result<LiveReflectionStatusAdmin, CustomError> {
    let row = sqlx::query_as::<_, (Option<time::OffsetDateTime>, i64, i64, i64)>(
        r"
        SELECT MAX(created_at) AS last_event_at,
               COUNT(*) FILTER (WHERE created_at > NOW() - INTERVAL '24 hours')::bigint AS checked_24h,
               COUNT(*) FILTER (
                   WHERE created_at > NOW() - INTERVAL '24 hours'
                     AND COALESCE(event_json->'data'->'pair_reflection'->>'status', event_json->'data'->>'status') = 'processed'
               )::bigint AS processed_24h,
               COUNT(*) FILTER (
                   WHERE created_at > NOW() - INTERVAL '24 hours'
                     AND COALESCE(event_json->'data'->'pair_reflection'->>'status', event_json->'data'->>'status') = 'skipped'
               )::bigint AS skipped_24h
        FROM bearwire_events
        WHERE bear_id = $1
          AND event_type = 'session.reflected'
          AND COALESCE(event_json->'data'->'pair_reflection'->>'trigger', event_json->'data'->>'trigger') = 'open-session-stale'
        ",
    )
    .bind(bear_id)
    .fetch_one(pool)
    .await
    .map_err(|err| CustomError::Database(format!("live reflection status: {err}")))?;

    let (status_label, status_explanation) = if !enabled {
        (
            "Off".to_string(),
            "This Bear will not proactively spend tokens reflecting open conversations."
                .to_string(),
        )
    } else if !workers_enabled {
        (
            "Configured on; worker not running".to_string(),
            "RUN_WORKERS is off for this Den process, so proactive sweeps will not run here."
                .to_string(),
        )
    } else if row.0.is_some() {
        (
            "On".to_string(),
            "The background worker has recorded live reflection sweep activity for this Bear."
                .to_string(),
        )
    } else {
        (
            "On; waiting for first sweep".to_string(),
            "The background worker is enabled, but no live reflection event has been recorded for this Bear yet.".to_string(),
        )
    };

    Ok(LiveReflectionStatusAdmin {
        enabled,
        workers_enabled,
        status_label,
        status_explanation,
        last_event_at: row.0.map(|value| value.to_string()),
        checked_24h: row.1,
        processed_24h: row.2,
        skipped_24h: row.3,
    })
}

async fn reflection_rows_for_bear(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    conversation_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ReflectionAdminRow>, CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT e.created_at,
               e.event_type,
               e.session_id,
               c.id AS "conversation_id?",
               c.current_title AS "conversation_title?",
               COALESCE(e.event_json->'data'->'pair_reflection', e.event_json->'data') AS "payload!: serde_json::Value"
        FROM bearwire_events e
        LEFT JOIN client_sessions s ON s.bear_id = e.bear_id
             AND s.user_id = e.user_id
             AND s.client_session_id = e.session_id
        LEFT JOIN LATERAL (
            SELECT c.id, c.current_title
            FROM conversations c
            WHERE c.bear_id = e.bear_id
              AND (
                  c.id::text = e.event_json->'data'->>'conversation_id'
                  OR (
                      e.event_json->'data'->>'conversation_id' IS NULL
                      AND (c.source_client_session_id = e.session_id
                           OR c.external_conversation_id = s.conversation_id
                           OR c.external_conversation_id = s.resolved_conversation_id)
                  )
              )
            ORDER BY c.updated_at DESC, c.id DESC
            LIMIT 1
        ) c ON TRUE
        WHERE e.bear_id = $1
          AND e.event_type IN ('session.reflected', 'session.closed')
          AND ($2::uuid IS NULL OR c.id = $2)
          AND COALESCE(e.event_json->'data'->'pair_reflection', e.event_json->'data') IS NOT NULL
        ORDER BY e.created_at DESC, e.sequence_no DESC
        LIMIT $3
        "#,
        bear_id, conversation_id, limit.clamp(1, 100),
    )
    .fetch_all(pool)
    .await
    .map_err(|err| CustomError::Database(format!("list reflection events: {err}")))?;

    Ok(
        rows.into_iter()
            .map(|row| {
                let (
                    created_at,
                    event_type,
                    session_id,
                    conversation_id,
                    conversation_title,
                    payload,
                ) = (
                    row.created_at,
                    row.event_type,
                    row.session_id,
                    row.conversation_id,
                    row.conversation_title,
                    row.payload,
                );
                let proposal_ids = payload
                    .get("proposal_ids")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let proposal_count = payload
                    .get("proposal_ids")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|ids| i64::try_from(ids.len()).ok());
                let status = payload
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let skipped_reason = payload
                    .get("skipped_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let candidate_count = json_i64(&payload, "candidate_count");
                let dropped_followup_count = json_i64(&payload, "dropped_followup_count");
                let feedback = super::memory::inspection::reflection_feedback(
                    status.as_deref(),
                    skipped_reason.as_deref(),
                    payload.get("error").and_then(serde_json::Value::as_str),
                );
                let counts_label = reflection_counts_label(
                    status.as_deref(),
                    skipped_reason.as_deref(),
                    candidate_count,
                    proposal_count,
                );
                let status_label = if !feedback.needs_attention
                    && status.as_deref() == Some("processed")
                    && skipped_reason.is_none()
                    && candidate_count == Some(0)
                {
                    "Inspected, no memories found".to_string()
                } else {
                    feedback.status_label
                };
                let status_explanation = feedback.status_explanation;
                let needs_attention = feedback.needs_attention;
                ReflectionAdminRow {
                    created_at: created_at.to_string(),
                    event_type,
                    session_id,
                    conversation_id,
                    conversation_title,
                    trigger: payload
                        .get("trigger")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    status,
                    skipped_reason,
                    error: feedback.error,
                    status_label,
                    status_explanation,
                    counts_label,
                    needs_attention,
                    retry_href: conversation_id.map(|id| format!("conversations/{id}/reflect")),
                    candidate_count,
                    dropped_followup_count,
                    proposal_count,
                    proposal_links: proposal_ids
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    source_message_start_seq: json_i64(&payload, "source_message_start_seq"),
                    source_message_end_seq: json_i64(&payload, "source_message_end_seq"),
                    payload_json: pretty_json(payload),
                }
            })
            .collect(),
    )
}

fn reflection_watermark_admin(
    latest_message_sequence_no: Option<i64>,
    reflections: &[ReflectionAdminRow],
) -> ReflectionWatermarkAdmin {
    let latest_reflection = reflections.iter().find(|row| {
        row.status.as_deref() == Some("processed")
            && row.skipped_reason.is_none()
            && row.error.is_none()
    });
    let reflected_through_sequence_no =
        latest_reflection.and_then(|row| row.source_message_end_seq);
    let new_message_count = latest_message_sequence_no
        .zip(reflected_through_sequence_no)
        .map(|(latest, reflected_through)| latest.saturating_sub(reflected_through));
    let explanation = match latest_reflection {
        None => "This conversation has not been reflected yet.".to_string(),
        Some(_row) if reflected_through_sequence_no.is_some() => {
            "Reflection events carry a source-message watermark, so repeated runs can show whether new transcript content exists.".to_string()
        }
        Some(row) => format!(
            "Last reflected at {}; this older event has no source-message watermark, so duplicate-prevention visibility is timestamp-only.",
            row.created_at
        ),
    };
    ReflectionWatermarkAdmin {
        latest_message_sequence_no,
        last_reflected_at: latest_reflection.map(|row| row.created_at.clone()),
        reflected_through_sequence_no,
        new_message_count,
        explanation,
    }
}

fn conversation_timeline_rows(reflections: &[ReflectionAdminRow]) -> Vec<ConversationTimelineRow> {
    let mut rows: Vec<_> = reflections
        .iter()
        .map(|reflection| ConversationTimelineRow {
            created_at: reflection.created_at.clone(),
            kind: "Reflection".to_string(),
            label: reflection.status_label.clone(),
            details: reflection.status_explanation.clone(),
        })
        .collect();
    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    rows
}

async fn conversation_checkpoint_artifacts(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    session_id: Option<&str>,
    limit: i64,
) -> Result<Vec<CheckpointArtifactAdminRow>, CustomError> {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            serde_json::Value,
            Option<serde_json::Value>,
            time::OffsetDateTime,
            time::OffsetDateTime,
        ),
    >(
        r"
        SELECT
            c.run_id,
            c.checkpoint_id,
            c.reason,
            c.control_level,
            c.validation_status,
            c.visibility,
            c.replay_policy,
            c.related_task_list_id,
            c.related_task_item_id,
            c.request,
            c.response,
            c.created_at,
            c.updated_at
        FROM bear_run_checkpoints c
        INNER JOIN turn_runs r ON r.run_id = c.run_id
        WHERE r.bear_id = $1 AND r.session_id = $2
        ORDER BY c.created_at DESC, c.checkpoint_id DESC
        LIMIT $3
        ",
    )
    .bind(bear_id)
    .bind(session_id)
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                run_id,
                checkpoint_id,
                reason,
                control_level,
                validation_status,
                visibility,
                replay_policy,
                related_task_list_id,
                related_task_item_id,
                request,
                response,
                created_at,
                updated_at,
            )| CheckpointArtifactAdminRow {
                run_id,
                checkpoint_id,
                reason,
                control_level,
                validation_status,
                visibility,
                replay_policy,
                related_task_list_id,
                related_task_item_id,
                request_json: pretty_json(request),
                response_json: response
                    .map(pretty_json)
                    .unwrap_or_else(|| "null".to_string()),
                created_at: created_at.to_string(),
                updated_at: updated_at.to_string(),
            },
        )
        .collect())
}

async fn conversation_compaction_artifacts(
    pool: &sqlx::PgPool,
    conversation_id: Uuid,
    limit: i64,
) -> Result<Vec<CompactionArtifactAdminRow>, CustomError> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            i64,
            i64,
            Option<i32>,
            Option<i32>,
            serde_json::Value,
            Option<Uuid>,
            time::OffsetDateTime,
        ),
    >(
        r"
        SELECT id,
               artifact_kind,
               policy_version,
               trigger,
               source_message_start_seq,
               source_message_end_seq,
               source_group_start,
               source_group_end,
               artifact_json,
               superseded_by,
               created_at
        FROM conversation_compaction_artifacts
        WHERE conversation_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        ",
    )
    .bind(conversation_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|err| CustomError::Database(format!("list compaction artifacts: {err}")))?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                artifact_kind,
                policy_version,
                trigger,
                source_message_start_seq,
                source_message_end_seq,
                source_group_start,
                source_group_end,
                artifact_json,
                superseded_by,
                created_at,
            )| CompactionArtifactAdminRow {
                id,
                artifact_kind,
                policy_version,
                trigger,
                source_message_start_seq,
                source_message_end_seq,
                source_group_start,
                source_group_end,
                artifact_json: pretty_json(artifact_json),
                superseded_by,
                created_at: created_at.to_string(),
            },
        )
        .collect())
}

async fn unique_import_slug(pool: &sqlx::PgPool, requested: &str) -> Result<String, CustomError> {
    let base = bear_slug_base(requested);
    if !bears_db::bear_slug_exists(pool, &base).await? {
        return Ok(base);
    }
    for idx in 2..=999 {
        let candidate = format!("{base}-{idx}");
        if !bears_db::bear_slug_exists(pool, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(CustomError::ValidationError(
        "could not find available slug for imported Bear".to_string(),
    ))
}

fn manifest_for_bear(bear: &den_service::bears::Bear) -> Result<BearBundleManifest, CustomError> {
    let exported_birthdate = match bear.birthday {
        Some(date) => date.to_string(),
        None => bear
            .created_at
            .format(&Rfc3339)
            .map_err(|err| CustomError::System(format!("format Bear birthdate failed: {err}")))?
            .chars()
            .take(10)
            .collect(),
    };
    Ok(BearBundleManifest {
        format: BEAR_BUNDLE_FORMAT.to_string(),
        version: BEAR_BUNDLE_VERSION,
        bear: BearBundleIdentity {
            slug: bear.slug.clone(),
            name: bear.name.clone(),
            description: bear.description.clone(),
            birthdate: exported_birthdate,
            default_model: None,
            tools_enabled: bear.tools_enabled.as_ref().map(|v| v.0.clone()),
        },
        prompts: BearBundlePrompts {
            system_prompt: bear.system_prompt.clone(),
            context_profile: bear.context_profile.as_ref().map(|v| v.0.clone()),
        },
        profiles: json!({}),
        hats: Vec::new(),
        ide_default_hat: None,
        skills: Vec::new(),
        model_configurations: Some(Vec::new()),
        default_model_configuration_id: None,
    })
}

async fn snapshot_memory_sqlite(state: &AppState, bear_id: Uuid) -> Result<Vec<u8>, CustomError> {
    let manager = state.memory_stores.clone();
    let store = manager.store_for_bear(bear_id).await?;
    let snapshot_path =
        std::env::temp_dir().join(format!("bear-export-{bear_id}-{}.sqlite", Uuid::new_v4()));
    if snapshot_path.exists() {
        let _ = std::fs::remove_file(&snapshot_path);
    }
    let literal = sqlite_string_literal(&snapshot_path)?;
    sqlx::query(&format!("VACUUM INTO {literal}"))
        .execute(store.pool())
        .await
        .map_err(|err| CustomError::System(format!("snapshot memory sqlite failed: {err}")))?;
    let bytes = std::fs::read(&snapshot_path)
        .map_err(|err| CustomError::System(format!("read memory sqlite snapshot failed: {err}")))?;
    if let Err(err) = std::fs::remove_file(&snapshot_path) {
        tracing::warn!(path = %snapshot_path.display(), error = %err, "failed to remove Bear export SQLite snapshot");
    }
    Ok(bytes)
}

async fn rewrite_imported_memory_bear_id(
    state: &AppState,
    bear_id: Uuid,
) -> Result<(), CustomError> {
    let manager = state.memory_stores.clone();
    let store = manager.store_for_bear(bear_id).await?;
    let new_id = bear_id.to_string();
    for table in [
        "memory_records",
        "entities",
        "entity_handles",
        "memory_relations",
        "memory_access_rules",
        "memory_promotions",
        "memory_proposals",
        "memory_observations",
        "reflection_run_outcomes",
    ] {
        sqlx::query(&format!("UPDATE {table} SET bear_id = ?"))
            .bind(&new_id)
            .execute(store.pool())
            .await
            .map_err(|err| CustomError::System(format!("rewrite {table}.bear_id failed: {err}")))?;
    }
    let integrity: Vec<(String,)> = sqlx::query_as("PRAGMA integrity_check")
        .fetch_all(store.pool())
        .await
        .map_err(|err| {
            CustomError::System(format!("imported memory integrity check failed: {err}"))
        })?;
    if integrity.first().map(|row| row.0.as_str()) != Some("ok") {
        return Err(CustomError::ValidationError(format!(
            "imported memory.sqlite failed integrity check: {:?}",
            integrity
        )));
    }
    Ok(())
}

async fn export_bear_bundle(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let mut manifest = manifest_for_bear(&bear)?;
    manifest.hats = portable_hats::export(&state, BearId::new(bear.id)).await?;
    manifest.model_configurations =
        Some(portable_models::export(state.sqlx_pool(), BearId::new(bear.id)).await?);
    manifest.default_model_configuration_id =
        den_service::bears::model_configurations::default_configuration_id(
            state.sqlx_pool(),
            BearId::new(bear.id),
        )
        .await?;
    manifest.skills = den_service::skills::export(state.sqlx_pool(), BearId::new(bear.id)).await?;
    manifest.ide_default_hat =
        hats::ide_default_hat(state.sqlx_pool(), BearId::new(bear.id)).await?;
    portable_models::validate(
        manifest.model_configurations.as_deref(),
        manifest.default_model_configuration_id,
        &manifest.hats,
    )?;
    let manifest_yaml = serde_yml::to_string(&manifest)
        .map_err(|err| CustomError::System(format!("serialize bear.yaml failed: {err}")))?;
    let memory_sqlite = snapshot_memory_sqlite(&state, bear.id).await?;
    let bundle = build_bear_bundle(&manifest_yaml, &memory_sqlite)?;
    let filename = format!("{}.bear", bear_slug_base(&bear.slug));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(Body::from(bundle))
        .map_err(|err| CustomError::System(format!("build Bear export response failed: {err}")))
}

async fn memory_stats_for_bear(
    state: &AppState,
    bear_id: Uuid,
) -> Result<Option<BearMemoryAdminStats>, CustomError> {
    let manager = state.memory_stores.clone();
    match bear_memory_admin_stats(&manager, state.config.as_ref(), bear_id).await {
        Ok(stats) => Ok(Some(stats)),
        Err(err) => {
            tracing::warn!(%bear_id, "bear memory stats unavailable: {err}");
            Ok(None)
        }
    }
}

#[derive(Serialize)]
struct RecallHealthView {
    status: &'static str,
    detail: String,
    lag_count: Option<i64>,
    failed_run_count: Option<i64>,
    last_success_at: Option<String>,
}

async fn recall_health_for_bear(state: &AppState, bear_id: Uuid) -> RecallHealthView {
    match recall_watermark_for_bear(
        state.sqlx_pool(),
        state.config.as_ref(),
        &state.memory_stores,
        bear_id,
    )
    .await
    {
        Ok(None) => RecallHealthView {
            status: "unavailable",
            detail: "Semantic recall is not configured; memory search uses the keyword fallback."
                .to_string(),
            lag_count: None,
            failed_run_count: None,
            last_success_at: None,
        },
        Ok(Some(watermark)) => {
            let healthy = watermark.is_healthy();
            RecallHealthView {
                status: if healthy { "healthy" } else { "degraded" },
                detail: if healthy {
                    "Semantic recall is current and its worker has no failures since the last success."
                        .to_string()
                } else {
                    format!(
                        "Semantic recall is behind: {} record(s) awaiting indexing and {} failed worker run(s).",
                        watermark.lag_count, watermark.failed_run_count
                    )
                },
                lag_count: Some(watermark.lag_count),
                failed_run_count: Some(watermark.failed_run_count),
                last_success_at: watermark.last_success_at,
            }
        }
        Err(err) => {
            tracing::warn!(%bear_id, "bear recall health unavailable: {err}");
            RecallHealthView {
                status: "unknown",
                detail: "Recall health could not be checked.".to_string(),
                lag_count: None,
                failed_run_count: None,
                last_success_at: None,
            }
        }
    }
}

async fn overview_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let overview_summary = super::overview::summary(
        &state,
        BearId::new(bear.id),
        session_user(&auth_session).await?.id,
        can_manage_bear,
    )
    .await?;
    if !can_manage_bear {
        return web::render_template(
            &state,
            "bear/settings/overview.html",
            auth_session,
            context! {
                message => query.message,
                can_manage_bear,
                overview_summary,
                ..bear_nav_context(&bear, "overview"),
            },
        )
        .await;
    }

    let id = bear.id;
    let member_count = bears_db::count_bear_members(state.sqlx_pool(), id).await?;
    let memory_stats = {
        let manager = state.memory_stores.clone();
        match bear_memory_admin_stats(&manager, state.config.as_ref(), id).await {
            Ok(stats) => Some(stats),
            Err(err) => {
                tracing::warn!(%id, "hub memory stats unavailable: {err}");
                None
            }
        }
    };
    let recall_health = recall_health_for_bear(&state, id).await;
    let conversation_count: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!: i64\" FROM conversations WHERE bear_id = $1",
        id
    )
    .fetch_one(state.sqlx_pool())
    .await
    .map_err(|err| CustomError::Database(format!("count bear conversations: {err}")))?;
    let pending_reviews = crate::management_hub::pending_memory_reviews(&state, id)
        .await
        .ok();
    let recent_rows: Vec<(Uuid, Option<String>, String)> = sqlx::query!(
        "SELECT id, current_title, to_char(updated_at, 'YYYY-MM-DD HH24:MI') AS \"updated!: String\" \
         FROM conversations WHERE bear_id = $1 ORDER BY updated_at DESC LIMIT 5",
        id
    )
    .fetch_all(state.sqlx_pool())
    .await
    .map_err(|err| CustomError::Database(format!("recent bear conversations: {err}")))?
    .into_iter()
    .map(|row| (row.id, row.current_title, row.updated))
    .collect();
    let recent_conversations: Vec<serde_json::Value> = recent_rows
        .into_iter()
        .map(|(cid, title, updated)| {
            json!({
                "id": cid.to_string(),
                "title": title.unwrap_or_else(|| "Untitled conversation".to_string()),
                "updated_at": updated,
            })
        })
        .collect();
    let weekly_rows: Vec<(String, i64)> = sqlx::query!(
        "SELECT to_char(date_trunc('week', updated_at), 'YYYY-MM-DD') AS \"week!: String\", \
                COUNT(*)::bigint AS \"count!: i64\" \
         FROM conversations WHERE bear_id = $1 \
           AND updated_at > now() - interval '8 weeks' \
         GROUP BY 1 ORDER BY 1 DESC",
        id
    )
    .fetch_all(state.sqlx_pool())
    .await
    .map_err(|err| CustomError::Database(format!("bear activity over time: {err}")))?
    .into_iter()
    .map(|row| (row.week, row.count))
    .collect();
    let weekly_max = weekly_rows.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let weekly_activity: Vec<serde_json::Value> = weekly_rows
        .into_iter()
        .map(|(week, n)| {
            let pct = if weekly_max > 0 {
                (((n as f64 / weekly_max as f64) * 10.0).ceil() as i64) * 10
            } else {
                0
            };
            json!({ "week": week, "count": n, "pct": pct })
        })
        .collect();

    web::render_template(
        &state,
        "bear/settings/overview.html",
        auth_session,
        context! {
            message => query.message,
            member_count,
            native_runtime => true,
            context_profile_enabled => bear.context_profile.is_some(),
            memory_stats,
            recall_health,
            legacy_import_locked => memory_stats.as_ref().map(|stats| stats.record_count > 0).unwrap_or(true),
            conversation_count,
            pending_reviews,
            recent_conversations,
            weekly_activity,
            can_manage_bear,
            overview_summary,
            bear_nav_active => "overview",
            ..bear_nav_context(&bear, "overview"),
        },
    )
    .await
}

async fn access_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    people::render(
        &state,
        auth_session,
        bear,
        can_manage_bear,
        MemberGrantForm::default(),
        query.message,
        query.error,
    )
    .await
}

/// The standalone compiled-prompts page merged into `/context` (prompt
/// assembly). Keep the old path working.
async fn persona_view(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/bear/{slug}/context"))
}

/// Retired stance/profile list URLs remain harmless redirects to diagnostics.
async fn stances_list_redirect(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/bear/{slug}/advanced"))
}

fn parse_loop_control_form_value(raw: &str) -> Result<Option<AgentLoopControlLevel>, CustomError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("inherit") {
        return Ok(None);
    }
    match trimmed {
        "light" => Ok(Some(AgentLoopControlLevel::Light)),
        "standard" => Ok(Some(AgentLoopControlLevel::Standard)),
        "careful" => Ok(Some(AgentLoopControlLevel::Careful)),
        "strict" => Ok(Some(AgentLoopControlLevel::Strict)),
        other => Err(CustomError::ValidationError(format!(
            "unsupported agent loop control level `{other}`"
        ))),
    }
}

fn display_number(value: Option<f64>) -> String {
    value
        .map(|value| {
            if value.fract().abs() < f64::EPSILON {
                format!("{}", value as i64)
            } else {
                format!("{value:.4}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
        })
        .unwrap_or_else(|| "—".to_string())
}

fn display_money(value: Option<f64>) -> String {
    value
        .map(|value| {
            format!("${value:.4}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string())
}

fn json_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(serde_json::Value::as_f64)
}

fn json_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("—")
        .to_string()
}

fn add_budget_rows(
    rows: &mut Vec<BifrostUsageBudgetRow>,
    model_rows: &mut Vec<BifrostUsageModelRow>,
    scope: String,
    budgets: Option<&Vec<serde_json::Value>>,
) {
    let Some(budgets) = budgets else {
        return;
    };
    for budget in budgets {
        let max_limit = json_f64(budget, "max_limit");
        let current_usage = json_f64(budget, "current_usage");
        rows.push(BifrostUsageBudgetRow {
            scope: scope.clone(),
            max_limit: display_money(max_limit),
            current_usage: display_money(current_usage),
            remaining: display_money(
                max_limit
                    .zip(current_usage)
                    .map(|(max, current)| max - current),
            ),
            reset_duration: json_str(budget, "reset_duration"),
        });
        if let Some(per_model) = budget
            .get("per_model_usage")
            .and_then(serde_json::Value::as_array)
        {
            for model in per_model {
                model_rows.push(BifrostUsageModelRow {
                    model: json_str(model, "model"),
                    provider: json_str(model, "provider"),
                    total_requests: display_number(json_f64(model, "total_requests")),
                    total_tokens: display_number(json_f64(model, "total_tokens")),
                    total_cost: display_money(json_f64(model, "total_cost")),
                });
            }
        }
    }
}

fn bifrost_usage_from_quota(
    quota: &den_service::bifrost_governance::BifrostVirtualKeyQuota,
) -> BifrostUsageView {
    let payload = &quota.payload;
    let mut budget_rows = Vec::new();
    let mut model_usage_rows = Vec::new();
    let top_level_budgets = payload.get("budgets").and_then(serde_json::Value::as_array);
    add_budget_rows(
        &mut budget_rows,
        &mut model_usage_rows,
        "Virtual key".to_string(),
        top_level_budgets,
    );

    let mut provider_rows = Vec::new();
    if let Some(providers) = payload
        .get("provider_configs")
        .and_then(serde_json::Value::as_array)
    {
        for provider in providers {
            let provider_name = json_str(provider, "provider");
            let allowed_models = provider
                .get("allowed_models")
                .and_then(serde_json::Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "—".to_string());
            let budgets = provider
                .get("budgets")
                .and_then(serde_json::Value::as_array);
            add_budget_rows(
                &mut budget_rows,
                &mut model_usage_rows,
                format!("Provider {provider_name}"),
                budgets,
            );
            provider_rows.push(BifrostUsageProviderRow {
                provider: provider_name,
                allowed_models,
                budget_count: budgets.map(Vec::len).unwrap_or(0),
            });
        }
    }

    if let Some(model_configs) = payload
        .get("model_configs")
        .and_then(serde_json::Value::as_array)
    {
        for model_config in model_configs {
            let model_name = json_str(model_config, "model_name");
            let provider = json_str(model_config, "provider");
            let scope = if provider == "—" {
                format!("Model {model_name}")
            } else {
                format!("Model {provider}/{model_name}")
            };
            add_budget_rows(
                &mut budget_rows,
                &mut model_usage_rows,
                scope,
                model_config
                    .get("budgets")
                    .and_then(serde_json::Value::as_array),
            );
        }
    }

    BifrostUsageView {
        status: "ok".to_string(),
        error: String::new(),
        virtual_key_name: json_str(payload, "virtual_key_name"),
        is_active: payload
            .get("is_active")
            .and_then(serde_json::Value::as_bool)
            .map(|value| if value { "yes" } else { "no" }.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        auth_mode: quota.auth_mode.as_str().to_string(),
        has_budgets: !budget_rows.is_empty(),
        has_model_usage: !model_usage_rows.is_empty(),
        has_providers: !provider_rows.is_empty(),
        budget_rows,
        model_usage_rows,
        provider_rows,
    }
}

fn bifrost_usage_from_management(
    details: &den_service::bifrost_governance::BifrostVirtualKeyDetails,
    rankings: Option<&serde_json::Value>,
) -> BifrostUsageView {
    let quota = den_service::bifrost_governance::BifrostVirtualKeyQuota {
        auth_mode: den_service::bifrost_governance::BifrostVirtualKeyAuthMode::XApiKey,
        payload: details.payload.clone(),
    };
    let mut view = bifrost_usage_from_quota(&quota);
    view.auth_mode = "management".to_string();
    if !details.name.trim().is_empty() {
        view.virtual_key_name.clone_from(&details.name);
    }

    if let Some(ranking_rows) = rankings
        .and_then(|value| value.get("rankings"))
        .and_then(serde_json::Value::as_array)
        .filter(|rows| !rows.is_empty())
    {
        view.model_usage_rows = ranking_rows
            .iter()
            .map(|row| BifrostUsageModelRow {
                model: json_str(row, "model"),
                provider: json_str(row, "provider"),
                total_requests: display_number(json_f64(row, "total_requests")),
                total_tokens: display_number(json_f64(row, "total_tokens")),
                total_cost: display_money(json_f64(row, "total_cost")),
            })
            .collect();
        view.has_model_usage = !view.model_usage_rows.is_empty();
    }

    view
}

async fn bifrost_usage_view_for_bear(state: &AppState, bear_id: Uuid) -> BifrostUsageView {
    let row = match bears_db::get_bear_bifrost_virtual_key(state.sqlx_pool(), bear_id).await {
        Ok(row) => row,
        Err(_) => {
            return BifrostUsageView {
                status: "error".to_string(),
                error: "Could not read the Bear's Bifrost virtual key metadata from Den storage."
                    .to_string(),
                virtual_key_name: String::new(),
                is_active: String::new(),
                auth_mode: String::new(),
                budget_rows: Vec::new(),
                model_usage_rows: Vec::new(),
                provider_rows: Vec::new(),
                has_budgets: false,
                has_model_usage: false,
                has_providers: false,
            };
        }
    };
    let Some(row) = row else {
        return BifrostUsageView {
            status: "missing".to_string(),
            error: "No Bifrost virtual key is configured for this Bear.".to_string(),
            virtual_key_name: String::new(),
            is_active: String::new(),
            auth_mode: String::new(),
            budget_rows: Vec::new(),
            model_usage_rows: Vec::new(),
            provider_rows: Vec::new(),
            has_budgets: false,
            has_model_usage: false,
            has_providers: false,
        };
    };
    let Some(virtual_key_id) = row
        .virtual_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return BifrostUsageView {
            status: "missing".to_string(),
            error: "No Bifrost virtual key id is configured for this Bear.".to_string(),
            virtual_key_name: row.virtual_key_name.unwrap_or_default(),
            is_active: String::new(),
            auth_mode: String::new(),
            budget_rows: Vec::new(),
            model_usage_rows: Vec::new(),
            provider_rows: Vec::new(),
            has_budgets: false,
            has_model_usage: false,
            has_providers: false,
        };
    };

    let client = den_service::bifrost_governance::BifrostGovernanceClient::new(&state.config);
    let details = match client.get_virtual_key_details_by_id(virtual_key_id).await {
        Ok(Some(details)) => details,
        Ok(None) => {
            return BifrostUsageView {
                status: "error".to_string(),
                error: format!(
                    "Bifrost management API could not find stored virtual key id {virtual_key_id}. Reprovision this Bear's virtual key."
                ),
                virtual_key_name: row.virtual_key_name.unwrap_or_default(),
                is_active: String::new(),
                auth_mode: String::new(),
                budget_rows: Vec::new(),
                model_usage_rows: Vec::new(),
                provider_rows: Vec::new(),
                has_budgets: false,
                has_model_usage: false,
                has_providers: false,
            };
        }
        Err(err) => {
            tracing::warn!(%bear_id, %virtual_key_id, error = %err, "Bifrost management virtual-key lookup failed while rendering usage");
            return BifrostUsageView {
                status: "unavailable".to_string(),
                error: "Bifrost usage details are temporarily unavailable because the Bifrost management API is not ready. Inference can still work while this panel is unavailable. Try refreshing this page shortly.".to_string(),
                virtual_key_name: row.virtual_key_name.unwrap_or_default(),
                is_active: "unknown".to_string(),
                auth_mode: "management".to_string(),
                budget_rows: Vec::new(),
                model_usage_rows: Vec::new(),
                provider_rows: Vec::new(),
                has_budgets: false,
                has_model_usage: false,
                has_providers: false,
            };
        }
    };

    let rankings = match client.get_model_usage_rankings(virtual_key_id).await {
        Ok(rankings) => Some(rankings),
        Err(err) => {
            tracing::warn!(%bear_id, %virtual_key_id, error = %err, "Bifrost model usage rankings unavailable while rendering usage");
            None
        }
    };
    bifrost_usage_from_management(&details, rankings.as_ref())
}

async fn render_models_page(
    state: AppState,
    auth_session: AuthSession,
    bear: den_service::bears::Bear,
    can_manage_bear: bool,
    message: Option<String>,
    error: Option<String>,
    pending: model_configurations::PendingModelsForm,
) -> Result<Response, CustomError> {
    render_models_page_with_draft(
        state,
        auth_session,
        bear,
        can_manage_bear,
        message,
        error,
        pending,
        None,
        std::collections::BTreeMap::new(),
    )
    .await
}

async fn render_models_page_with_draft(
    state: AppState,
    auth_session: AuthSession,
    bear: den_service::bears::Bear,
    can_manage_bear: bool,
    message: Option<String>,
    error: Option<String>,
    pending: model_configurations::PendingModelsForm,
    advanced_form: Option<advanced_models::Draft>,
    field_errors: std::collections::BTreeMap<&'static str, String>,
) -> Result<Response, CustomError> {
    let bear_id = BearId::new(bear.id);
    let catalog = crate::model_availability::BearModelCatalog::load(&state, bear_id).await?;
    let catalog_error = catalog.diagnostic();
    let model_options = model_configurations::catalog_options(state.sqlx_pool(), &catalog).await?;
    let configurations = model_configurations::configuration_views(
        state.sqlx_pool(),
        &catalog,
        bear_id,
        pending.configuration.as_ref(),
    )
    .await?;
    let default_configuration_id =
        den_service::bears::model_configurations::default_configuration_id(
            state.sqlx_pool(),
            bear_id,
        )
        .await?;
    let default_selection = pending
        .default_selection
        .selected_id(default_configuration_id)
        .map(|id| id.to_string())
        .unwrap_or_default();
    let new_configuration = pending
        .configuration
        .as_ref()
        .filter(|(id, _)| id.is_none())
        .map(|(_, form)| form.clone())
        .unwrap_or_default();
    let effective_model = model_configurations::effective_model(
        state.sqlx_pool(),
        &catalog,
        bear_id,
        None,
        &state.config.default_llm_model,
    )
    .await?;
    let bear_loop_control = bears_db::bear_agent_loop_control_setting(state.sqlx_pool(), bear.id)
        .await?
        .map(AgentLoopControlLevel::as_str)
        .unwrap_or("inherit");
    let bear_tool_budget_multiplier = bear
        .default_tool_budget_multiplier
        .map(|value| value.to_string())
        .unwrap_or_default();

    let bifrost_virtual_key =
        bears_db::get_bear_bifrost_virtual_key(state.sqlx_pool(), bear.id).await?;
    let bifrost_usage = bifrost_usage_view_for_bear(&state, bear.id).await;
    web::render_template(
        &state,
        "bear/settings/models.html",
        auth_session,
        context! {
            model_options,
            catalog_error,
            configurations,
            default_selection,
            new_configuration,
            effective_model,
            stored_bear_loop_control => bear_loop_control,
            stored_bear_tool_budget_multiplier => bear_tool_budget_multiplier,
            bear_loop_control => advanced_form.as_ref().map(|form| form.bear_loop_control.as_str()).unwrap_or(bear_loop_control),
            bear_tool_budget_multiplier => advanced_form.as_ref().map(|form| form.bear_tool_budget_multiplier.as_str()).unwrap_or(&bear_tool_budget_multiplier),
            bifrost_virtual_key_id => advanced_form.as_ref().map(|form| form.bifrost_virtual_key_id.as_str()).unwrap_or_else(|| bifrost_virtual_key.as_ref().and_then(|row| row.virtual_key_id.as_deref()).unwrap_or("")),
            bifrost_virtual_key_name => advanced_form.as_ref().map(|form| form.bifrost_virtual_key_name.as_str()).unwrap_or_else(|| bifrost_virtual_key.as_ref().and_then(|row| row.virtual_key_name.as_deref()).unwrap_or("")),
            bifrost_virtual_key_clear => advanced_form.as_ref().is_some_and(|form| form.bifrost_virtual_key_clear),
            advanced_form,
            field_errors,
            bifrost_virtual_key_configured => bifrost_virtual_key.as_ref().map(|row| {
                row.virtual_key_value_encrypted.as_deref().map(|value| !value.trim().is_empty()).unwrap_or(false)
                    || row.virtual_key_value.as_deref().map(|value| !value.trim().is_empty()).unwrap_or(false)
            }).unwrap_or(false),
            bifrost_usage,
            message,
            error,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "identity"),
        },
    )
    .await
}

async fn models_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let (flash_message, flash_error) = take_models_flash(&session).await?;
    render_models_page(
        state,
        auth_session,
        bear,
        can_manage_bear,
        flash_message.or(query.message),
        flash_error.or(query.error),
        model_configurations::PendingModelsForm::default(),
    )
    .await
}

async fn provision_bifrost_virtual_key_action(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let reset_usage_tracking =
        provision_bifrost_virtual_key_for_bear(&state, bear.id, &bear.slug).await?;
    let message = if reset_usage_tracking {
        "Bifrost virtual key provisioned for this Bear. The previous key with this Bear name was archived, so Bifrost usage and budget tracking start fresh for the replacement key."
    } else {
        "Bifrost virtual key provisioned for this Bear."
    };
    set_models_flash(&session, message).await?;
    Ok(Redirect::to(&format!("/bear/{}/models", bear.slug)).into_response())
}

async fn conversations_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let can_manage_bear = true;
    let rows =
        conversation_persistence::list_conversations_for_bear(state.sqlx_pool(), bear.id, 50)
            .await?;
    let conversations: Vec<ConversationAdminRow> = rows
        .into_iter()
        .map(|c| {
            let external_id = c
                .external_conversation_id
                .unwrap_or_else(|| "(none)".to_string());
            ConversationAdminRow {
                id: c.id,
                external_id,
                title: c
                    .current_title
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "Untitled".to_string()),
                source_session: c
                    .source_client_session_id
                    .unwrap_or_else(|| "—".to_string()),
                updated_at: c.updated_at.to_string(),

                latest_context_budget_updated_at: c
                    .latest_context_budget_updated_at
                    .map(|value| value.to_string()),
                latest_context_budget_summary: c
                    .latest_context_budget
                    .as_ref()
                    .map(context_budget_summary),
                latest_context_budget: c.latest_context_budget,
            }
        })
        .collect();

    let live_reflection_status = live_reflection_status_for_bear(
        state.sqlx_pool(),
        bear.id,
        bear.live_reflection_enabled,
        state.config.run_workers,
    )
    .await?;
    web::render_template(
        &state,
        "bear/settings/conversations.html",
        auth_session,
        context! {
            conversations,
            live_reflection_status,
            can_manage_bear,
            native_runtime => true,
            live_reflection_enabled => bear.live_reflection_enabled,
            message => query.message,
            error => query.error,

            ..bear_nav_context(&bear, "activity"),
        },
    )
    .await
}

async fn reflections_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let can_manage_bear = true;
    let reflections = reflection_rows_for_bear(state.sqlx_pool(), bear.id, None, 100).await?;
    web::render_template(
        &state,
        "bear/settings/reflections.html",
        auth_session,
        context! {
            reflections,
            can_manage_bear,
            native_runtime => true,
            live_reflection_enabled => bear.live_reflection_enabled,
            ..bear_nav_context(&bear, "reflections"),
        },
    )
    .await
}

async fn conversation_detail_view(
    Path((slug, conversation_id)): Path<(String, Uuid)>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let can_manage_bear = true;
    let conv = conversation_persistence::get_conversation_by_id(state.sqlx_pool(), conversation_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("conversation not found".to_string()))?;
    if conv.bear_id != bear.id {
        return Err(CustomError::NotFound("conversation not found".to_string()));
    }
    let bound_hat =
        hats::bindings::conversation_hat(state.sqlx_pool(), BearId::new(bear.id), conversation_id)
            .await?;
    let available_hats = hats::list_hats(state.sqlx_pool(), BearId::new(bear.id)).await?;
    let messages = list_messages_page(state.sqlx_pool(), conversation_id, None, 40).await?;
    let can_bind_hat = hats::bindings::conversation_can_bind_hat(
        state.sqlx_pool(),
        BearId::new(bear.id),
        conversation_id,
    )
    .await?;
    // Legacy runtime_compaction_events only carries an external conversation id,
    // so it cannot safely be attributed to this Bear. Show only persisted,
    // conversation-id-scoped artifacts below.
    let compaction_artifacts =
        conversation_compaction_artifacts(state.sqlx_pool(), conversation_id, 10).await?;
    let checkpoint_artifacts = conversation_checkpoint_artifacts(
        state.sqlx_pool(),
        bear.id,
        conv.source_client_session_id.as_deref(),
        20,
    )
    .await?;
    let reflections =
        reflection_rows_for_bear(state.sqlx_pool(), bear.id, Some(conversation_id), 20).await?;
    let processing_timeline = conversation_timeline_rows(&reflections);
    let reflection_watermark =
        reflection_watermark_admin(messages.iter().map(|m| m.sequence_no).max(), &reflections);
    let message_rows: Vec<MessageAdminRow> = messages
        .into_iter()
        .rev()
        .map(|m| {
            let preview: String = m.content_text.chars().take(280).collect();
            MessageAdminRow {
                sequence_no: m.sequence_no,
                message_type: m.message_type,
                role: m.role.unwrap_or_else(|| "—".to_string()),
                visibility: m.visibility,
                preview,
            }
        })
        .collect();
    web::render_template(
        &state,
        "bear/settings/conversation.html",
        auth_session,
        context! {
            conv,
            bound_hat,
            available_hats,
            can_bind_hat,
            message_rows,
            compaction_artifacts,
            checkpoint_artifacts,
            reflections,
            processing_timeline,
            reflection_watermark,
            message => query.message,
            error => query.error,
            can_manage_bear,
            native_runtime => true,
            live_reflection_enabled => bear.live_reflection_enabled,
            ..bear_nav_context(&bear, "activity"),
        },
    )
    .await
}

/// Context: the explanatory view of per-turn prompt assembly. Merges the
/// former compiled-prompts (persona) and prompt-memory-block pages into one
/// page organized by assembly layer, not by storage.
async fn context_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    use den_core::tools::prompt_memory::{PromptMemoryBlockScope, PromptMemoryBlockState};

    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let can_manage_bear = true;
    let id = bear.id;

    // Read existing snapshots only; inspection must not compile or register runtimes.
    use super::memory::inspection::read_result;
    let mut inspection_errors = Vec::new();
    let context_profile_enabled = bear.context_profile.is_some();
    let template_id = read_result(
        context_profile_from_json(&bear.context_profile),
        "Context configuration",
        &mut inspection_errors,
    )
    .flatten()
    .and_then(|profile| profile.template_id);
    let compiled: Option<BearCompiledConfigRow> = read_result(
        get_compiled_bear_config(state.sqlx_pool(), id).await,
        "Compiled prompt snapshot",
        &mut inspection_errors,
    )
    .flatten();
    let mut compiled_bound_prompts: Vec<CompiledRolePromptRow> = Vec::new();
    let mut compiled_roles: Vec<CompiledRolePromptRow> = Vec::new();
    if let Some(ref row) = compiled {
        if let Some(prompts) = read_result(
            serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                row.rendered_prompts_json.0.clone(),
            ),
            "Compiled prompt snapshot decode",
            &mut inspection_errors,
        ) {
            for (key, label) in [
                ("bound_base", "Bear base"),
                ("bound_chat_mode", "Chat mode"),
                ("bound_pair_mode", "Pair mode"),
                ("bound_work_mode", "Work mode"),
            ] {
                if let Some(text) = prompts.get(key).and_then(|v| v.as_str()) {
                    compiled_bound_prompts.push(CompiledRolePromptRow {
                        role: label.to_string(),
                        prompt_preview: text.chars().take(600).collect(),
                        char_count: text.chars().count(),
                    });
                }
            }
            for role in ["chat", "pair", "curate", "work", "watch"] {
                if let Some(text) = prompts.get(role).and_then(|v| v.as_str()) {
                    let preview: String = text.chars().take(600).collect();
                    compiled_roles.push(CompiledRolePromptRow {
                        role: role.to_string(),
                        prompt_preview: preview,
                        char_count: text.len(),
                    });
                }
            }
        }
    }

    // Layer 2: standing notes (durable prompt-memory blocks). Bear-wide
    // blocks come back for every stance query, so dedupe by id. Session
    // notes are transient working state and are only counted here.
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut standing_notes: Vec<PromptMemoryAdminRow> = Vec::new();
    let mut session_note_count: usize = 0;
    let mut inactive_note_count: usize = 0;
    for role in ["chat", "pair", "curate", "work", "watch"] {
        let Some(blocks) = read_result(
            list_prompt_memory_blocks_for_bear_profile(state.sqlx_pool(), id, role).await,
            &format!("Standing notes ({role})"),
            &mut inspection_errors,
        ) else {
            continue;
        };
        for block in blocks {
            if !seen_ids.insert(block.id.clone()) {
                continue;
            }
            if block.scope == PromptMemoryBlockScope::Session {
                session_note_count += 1;
                continue;
            }
            if block.state != PromptMemoryBlockState::Active {
                inactive_note_count += 1;
                continue;
            }
            let applies_to = match block.scope {
                PromptMemoryBlockScope::BearWide => "every stance".to_string(),
                PromptMemoryBlockScope::RoleLocal => {
                    format!("{} stance", block.role.as_deref().unwrap_or("one"))
                }
                PromptMemoryBlockScope::WorkSurface => format!(
                    "work surface {}",
                    block.work_surface.as_deref().unwrap_or("(unnamed)")
                ),
                PromptMemoryBlockScope::Session => unreachable!(),
            };
            let body_preview: String = block.body.chars().take(200).collect();
            standing_notes.push(PromptMemoryAdminRow {
                block_id: block.id,
                scope: applies_to,
                block_type: block.block_type.as_str().to_string(),
                state: String::new(),
                title: block.title,
                body_preview,
            });
        }
    }

    // Layer 4: recall availability shapes what assembly can include.
    let recall_configured = state.config.qdrant_url.is_some();

    // The most recent turn's budget report: per-component attribution of the
    // assembled context, persisted per conversation on every turn.
    let latest_budget_result = sqlx::query!(
        "SELECT latest_context_budget_json AS \"latest_context_budget_json!: serde_json::Value\", \
                id, current_title, \
                to_char(latest_context_budget_updated_at, 'YYYY-MM-DD HH24:MI') AS \"updated_at!: String\" \
         FROM conversations \
         WHERE bear_id = $1 AND latest_context_budget_json IS NOT NULL \
         ORDER BY latest_context_budget_updated_at DESC NULLS LAST LIMIT 1",
        id
    )
    .fetch_optional(state.sqlx_pool())
    .await
    .map_err(|err| CustomError::Database(format!("latest bear context budget: {err}")));
    let latest_budget_row = read_result(
        latest_budget_result,
        "Latest recorded turn budget",
        &mut inspection_errors,
    )
    .flatten()
    .map(|row| {
        (
            row.latest_context_budget_json,
            row.id,
            row.current_title,
            row.updated_at,
        )
    });
    let latest_budget = latest_budget_row.and_then(|(value, conv_id, title, at)| {
        let report: ContextBudgetReport = read_result(
            serde_json::from_value(value),
            "Turn budget snapshot decode",
            &mut inspection_errors,
        )?;
        let denominator: u32 = report.estimated_input_tokens.max(1);
        let mut components: Vec<serde_json::Value> = report
            .components
            .iter()
            .filter(|c| c.estimated_tokens > 0)
            .map(|c| {
                let pct_exact = f64::from(c.estimated_tokens) / f64::from(denominator) * 100.0;
                // Bucket to tens for the CSS bar width classes; keep the
                // exact value for display.
                let pct_bucket = ((pct_exact / 10.0).ceil() as i64).clamp(0, 10) * 10;
                json!({
                    "label": c.label,
                    "tokens": c.estimated_tokens,
                    "pct_display": format!("{pct_exact:.0}"),
                    "pct": pct_bucket,
                })
            })
            .collect();
        components.sort_by(|a, b| {
            b["tokens"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&a["tokens"].as_u64().unwrap_or(0))
        });
        Some(json!({
            "model": report.model,
            "context_window": report.context_window,
            "estimated_input_tokens": report.estimated_input_tokens,
            "reserved_output_tokens": report.reserved_output_tokens,
            "near_budget": report.near_budget,
            "over_budget": report.over_budget,
            "components": components,
            "conversation_id": conv_id.to_string(),
            "conversation_title": title.unwrap_or_else(|| "Untitled conversation".to_string()),
            "updated_at": at,
        }))
    });

    web::render_template(
        &state,
        "bear/settings/context.html",
        auth_session,
        context! {
            inspection_errors,
            stored_snapshots => true,
            context_profile_enabled,
            template_id,
            compiled,
            compiled_bound_prompts,
            compiled_roles,
            standing_notes,
            session_note_count,
            inactive_note_count,
            recall_configured,
            latest_budget,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "context"),
        },
    )
    .await
}

async fn policy_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let id = bear.id;
    let hats_configured = !hats::list_hats(state.sqlx_pool(), BearId::new(id))
        .await?
        .is_empty();
    let web_sources: Vec<BearWebSourceRow> = bear_web_sources(state.sqlx_pool(), id).await?;
    let web_approvals: Vec<BearWebApprovalRow> = bear_web_approvals(state.sqlx_pool(), id).await?;
    let web_fetches: Vec<BearWebFetchRow> = if can_manage_bear {
        bear_web_fetches(state.sqlx_pool(), id).await?
    } else {
        Vec::new()
    };
    let plan_mode_rows: Vec<BearPlanModeRow> = if can_manage_bear {
        bear_plan_mode_rows(state.sqlx_pool(), id).await?
    } else {
        Vec::new()
    };
    let repositories =
        den_service::work_surfaces::list_surfaces_for_bears(state.sqlx_pool(), &[id]).await?;
    web::render_template(
        &state,
        "bear/settings/policy.html",
        auth_session,
        context! {
            web_sources,
            web_approvals,
            web_fetches,
            hats_configured,
            repositories,
            plan_mode_rows,
            message => query.message,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "resources"),
        },
    )
    .await
}

async fn advanced_view(
    Path(slug): Path<String>,
    Query(query): Query<DomainQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let can_manage_bear = true;
    let stats = memory_stats_for_bear(&state, bear.id).await?;
    web::render_template(
        &state,
        "bear/settings/advanced.html",
        auth_session,
        context! {
            stats,
            message => query.message,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "advanced"),
        },
    )
    .await
}

async fn live_reflection_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<LiveReflectionForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let enabled = matches!(form.enabled.as_str(), "1" | "true" | "on" | "yes");
    bears_db::update_live_reflection_settings(
        state.sqlx_pool(),
        bear.id,
        enabled,
        form.stale_after_minutes,
        form.activity_threshold,
        form.sweep_limit,
    )
    .await?;
    let message = if enabled {
        "Live reflection settings saved."
    } else {
        "Live reflection disabled; sweep settings saved."
    };
    Ok(Redirect::to(&format!(
        "/bear/{}/advanced?message={}",
        bear.slug,
        urlencoding::encode(message)
    ))
    .into_response())
}

async fn reflect_conversations_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<ReflectConversationsForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let selected_ids = match form.bulk_action.as_str() {
        "selected" | "" => form.conversation_ids,
        _ => Vec::new(),
    };
    if selected_ids.is_empty() {
        return Ok(Redirect::to(&format!(
            "/bear/{}/conversations?message={}",
            bear.slug,
            urlencoding::encode("No conversations matched that reflection action.")
        ))
        .into_response());
    }
    let mut processed = 0usize;
    let mut compaction_applied = 0usize;
    let mut compaction_skipped = 0usize;
    let mut proposals_created = 0usize;
    let mut reflection_skipped = 0usize;
    let mut failures = Vec::new();
    for conversation_id in selected_ids.into_iter().take(25) {
        let conv = match conversation_persistence::get_conversation_by_id(
            state.sqlx_pool(),
            conversation_id,
        )
        .await?
        {
            Some(conv) if conv.bear_id == bear.id => conv,
            _ => continue,
        };
        let result = reflect_persisted_conversation(
            &state,
            auth_session.user.as_ref().map(|u| u.id),
            &bear,
            &conv,
            "manual_bulk",
        )
        .await?;
        if result.error.is_some() {
            failures.push(format!(
                "{}: {}",
                conv.id,
                manual_reflection::summary(&result)
            ));
        } else {
            processed += 1;
        }
        compaction_applied += usize::from(result.compaction_applied);
        compaction_skipped += usize::from(result.compaction_skipped);
        proposals_created += result.proposals_created;
        reflection_skipped += usize::from(result.skipped_reason.is_some());
    }
    let summary = format!("Manual reflection: {processed} conversation(s) completed; {compaction_applied} checkpoint(s) created; {compaction_skipped} compaction check(s) skipped; {proposals_created} known proposal(s) created; {reflection_skipped} reflection run(s) skipped.");
    let (key, feedback) = if failures.is_empty() {
        ("message", summary)
    } else {
        (
            "error",
            format!(
                "{summary} {} failure(s): {}",
                failures.len(),
                failures.join(" ")
            ),
        )
    };
    Ok(Redirect::to(&format!(
        "/bear/{}/conversations?{key}={}",
        bear.slug,
        urlencoding::encode(&feedback)
    ))
    .into_response())
}

async fn reflect_conversation_post(
    Path((slug, conversation_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let conv = conversation_persistence::get_conversation_by_id(state.sqlx_pool(), conversation_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("conversation not found".to_string()))?;
    if conv.bear_id != bear.id {
        return Err(CustomError::NotFound("conversation not found".to_string()));
    }
    let result = reflect_persisted_conversation(
        &state,
        auth_session.user.as_ref().map(|u| u.id),
        &bear,
        &conv,
        "manual",
    )
    .await?;
    let feedback_key = if result.error.is_some() {
        "error"
    } else {
        "message"
    };
    Ok(Redirect::to(&format!(
        "/bear/{}/conversations/{}?{feedback_key}={}",
        bear.slug,
        conversation_id,
        urlencoding::encode(&manual_reflection::summary(&result))
    ))
    .into_response())
}

async fn reconsider_conversation_post(
    Path((slug, conversation_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let conv = conversation_persistence::get_conversation_by_id(state.sqlx_pool(), conversation_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("conversation not found".to_string()))?;
    if conv.bear_id != bear.id {
        return Err(CustomError::NotFound("conversation not found".to_string()));
    }
    let result = reflect_persisted_conversation(
        &state,
        auth_session.user.as_ref().map(|u| u.id),
        &bear,
        &conv,
        "manual_reconsider",
    )
    .await?;
    if result.error.is_some() {
        return Ok(Redirect::to(&format!(
            "/bear/{}/conversations/{}?error={}",
            bear.slug,
            conversation_id,
            urlencoding::encode(&manual_reflection::summary(&result))
        ))
        .into_response());
    }
    web::render_template(
        &state,
        "bear/settings/reconsider_result.html",
        auth_session,
        context! {
            conv,
            result,
            can_manage_bear => true,
            native_runtime => true,
            ..bear_nav_context(&bear, "activity"),
        },
    )
    .await
}

async fn add_web_source_action(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<AddWebSourceForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let id = bear.id;
    let scope_kind = form.scope_kind.trim();
    let policy = form.policy.trim();
    if !matches!(scope_kind, "host" | "url")
        || !matches!(policy, "preferred" | "allowed" | "blocked")
    {
        return Ok(Redirect::to(&format!(
            "/bear/{}/resources?message={}",
            bear.slug,
            urlencoding::encode("Invalid web source policy form.")
        ))
        .into_response());
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return Ok(Redirect::to(&format!(
                "/bear/{}/resources?message={}",
                bear.slug,
                urlencoding::encode(&err.to_string())
            ))
            .into_response());
        }
    };
    if policy == "allowed"
        && !hats::list_hats(state.sqlx_pool(), BearId::new(bear.id))
            .await?
            .is_empty()
    {
        return Err(CustomError::ValidationError(
            "Bear-wide web allows cannot be created after configuring hats; grant hosts on a hat"
                .into(),
        ));
    }
    sqlx::query!(
        r#"
        INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, label, policy, priority)
        VALUES ($1, $2, $3, NULLIF($4, ''), $5, $6)
        ON CONFLICT (bear_id, scope_kind, scope_value)
        DO UPDATE SET label = EXCLUDED.label,
                      policy = EXCLUDED.policy,
                      priority = EXCLUDED.priority,
                      updated_at = now()
        "#,
        id,
        scope_kind,
        scope_value,
        form.label.trim(),
        policy,
        form.priority.unwrap_or(0)
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/resources?message={}",
        bear.slug,
        urlencoding::encode("Web source saved.")
    ))
    .into_response())
}

async fn delete_web_source_action(
    Path((slug, source_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    sqlx::query!(
        "DELETE FROM bear_web_sources WHERE bear_id = $1 AND id = $2",
        bear.id,
        source_id
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/resources?message={}",
        bear.slug,
        urlencoding::encode("Web source deleted.")
    ))
    .into_response())
}

async fn add_web_approval_action(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<AddWebApprovalForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    let scope_kind = form.scope_kind.trim();
    if !matches!(scope_kind, "host" | "url") {
        return Ok(Redirect::to(&format!(
            "/bear/{}/resources?message={}",
            bear.slug,
            urlencoding::encode("Invalid web approval scope.")
        ))
        .into_response());
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return Ok(Redirect::to(&format!(
                "/bear/{}/resources?message={}",
                bear.slug,
                urlencoding::encode(&err.to_string())
            ))
            .into_response());
        }
    };
    // "admin" is the human-via-web-UI source; the bear_web_approvals source
    // CHECK constraint only admits ('acp', 'web', 'admin').
    web_policy::record_web_approval(
        state.sqlx_pool(),
        bear.id,
        scope_kind,
        &scope_value,
        auth_session.user.as_ref().map(|u| u.id),
        "admin",
        None,
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/resources?message={}",
        bear.slug,
        urlencoding::encode("Web approval added.")
    ))
    .into_response())
}

async fn revoke_web_approval_action(
    Path((slug, approval_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(b) => b,
        Err(r) => return Ok(r.into_response()),
    };
    sqlx::query!(
        "UPDATE bear_web_approvals SET revoked_at = now() WHERE bear_id = $1 AND id = $2",
        bear.id,
        approval_id
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/resources?message={}",
        bear.slug,
        urlencoding::encode("Web approval revoked.")
    ))
    .into_response())
}

#[cfg(test)]
mod tests;
