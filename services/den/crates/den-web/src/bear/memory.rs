//! Bear memory + entity admin views at `/bear/{slug}/memory…` and `/bear/{slug}/entities…`.
//!
//! Members see curated shared and Bear-hat memory; Bear admins can explicitly inspect
//! all scopes, review evidence, and derived entities. Canonical memory lives in per-Bear
//! SQLite (ADR-0031). Qdrant passages are not an authorization source.

use axum::{
    extract::{Multipart, Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use bearwire_protocol::wire::BearWireEvent;
use minijinja::context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path as FsPath, PathBuf},
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    errors::CustomError,
    web::{self, AppState},
};

use super::settings::{bear_nav_context, load_session_bear, session_user};
use den_core::ids::BearId;
use den_memory::library::{self, CuratedMemoryGrant};
use den_memory::{
    self as store, bear_memory_admin_stats, count_memory_proposals, count_records_by_kind,
    count_records_by_profile, get_memory_proposal as get_sqlite_memory_proposal,
    get_memory_record_detail, head_entry_count, import_legacy_memory_bundle,
    list_memory_proposals as list_sqlite_memory_proposals, list_path_summaries,
    list_recent_memory_records, list_relations_for_entity, list_relations_for_source,
    list_reviewable_memory_proposals, resolve_memory_proposal as resolve_sqlite_memory_proposal,
    search_memory_records, LegacyMemoryImportOptions, MemoryRecordRow, MemoryStoreManager,
    PathSummary, SqliteMemoryProposal,
};
use den_service::bears::{db as bears_db, hats, BearProfile};
use den_service::recall::{registry as recall_registry, semantic_search_for_bear};
use den_service::{
    memory_proposals::{self, CreateMemoryProposal},
    pair_reflection,
};

use super::member::{email_verify_redirect, load_bear_member, viewer_can_manage_bear};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/memory", get(dashboard_view))
        .route_with_tsr(
            "/bear/{slug}/memory/review-queue/clear",
            post(clear_review_queue_post),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/import-legacy",
            post(import_legacy_memory_post),
        )
        .route_with_tsr("/bear/{slug}/memory/recent", get(recent_view))
        .route_with_tsr("/bear/{slug}/memory/search", get(search_view))
        .route_with_tsr(
            "/bear/{slug}/memory/browse",
            get(browse_view).post(browse_delete_post),
        )
        .route_with_tsr("/bear/{slug}/memory/records/{memory_id}", get(record_view))
        .route_with_tsr(
            "/bear/{slug}/memory/reflection/{run_id}",
            get(reflection_run_get),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/reflection/{run_id}/evidence",
            get(reflection_evidence_get),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/proposals/{proposal_id}",
            get(proposal_get).post(proposal_post),
        )
        .route_with_tsr("/bear/{slug}/entities", get(entities_view))
        .route_with_tsr("/bear/{slug}/entities/{entity_id}", get(entity_detail_view))
}

// ---------------------------------------------------------------------------
// Query / form types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DashboardQuery {
    #[serde(default)]
    import_notice: Option<String>,
    #[serde(default)]
    import_error: Option<String>,
    #[serde(default)]
    review_notice: Option<String>,
    #[serde(default)]
    review_error: Option<String>,
    #[serde(default)]
    reflection_lane: Option<String>,
    #[serde(default)]
    reflection_status: Option<String>,
    #[serde(default)]
    reflection_attention: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BrowseQuery {
    #[serde(default)]
    deleted: Option<usize>,
    #[serde(default)]
    review_requested: Option<usize>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EntitiesQuery {
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryDeleteForm {
    role: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    confirm: String,
    #[serde(default)]
    review_title: Option<String>,
    #[serde(default)]
    review_summary: Option<String>,
    #[serde(default)]
    review_rationale: Option<String>,
    #[serde(default)]
    suggested_action: Option<String>,
    #[serde(default)]
    sensitivity: Option<String>,
    #[serde(default)]
    requires_human: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryProposalResolutionForm {
    status: String,
    #[serde(default)]
    review_notes: Option<String>,
    #[serde(default)]
    decision_summary: Option<String>,
    #[serde(default)]
    after_save: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClearReviewQueueForm {
    #[serde(default)]
    confirm: String,
}

// ---------------------------------------------------------------------------
// View-model rows
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RecordListItem {
    memory_id: String,
    kind: String,
    scope_label: String,
    logical_path: Option<String>,
    created_at: String,
    sequence_no: i64,
    snippet: String,
    score: Option<f32>,
}

#[derive(Debug, Serialize)]
struct PathGroup {
    label: String,
    paths: Vec<PathSummary>,
}

#[derive(Debug, Serialize)]
struct HistoryItem {
    memory_id: String,
    sequence_no: i64,
    kind: String,
    created_at: String,
    is_current: bool,
}

#[derive(Debug, Serialize)]
struct LinkedEntity {
    entity_id: String,
    display_name: String,
    entity_type: String,
    relation_label: String,
    class_label: String,
}

#[derive(Debug, Serialize)]
struct EntityListItem {
    entity_id: String,
    display_name: String,
    entity_type: String,
    resolution: String,
    trust: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct LinkedRecord {
    src_memory_id: String,
    relation_label: String,
    class_label: String,
    author_profile: String,
    created_at: String,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionRunView {
    id: String,
    short_id: String,
    lane: String,
    trigger: String,
    status: String,
    status_label: String,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    conversation_id: Option<String>,
    conversation_key: Option<String>,
    conversation_date: Option<String>,
    queue_wait_label: String,
    duration_label: String,
    queued_age_label: String,
    proposal_count: Option<usize>,
    scanned_artifacts: Option<i64>,
    candidate_count: Option<i64>,
    discarded_count: Option<i64>,
    no_candidate_count: Option<i64>,
    output_summary: Value,
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionRunSummary {
    total: usize,
    queued: usize,
    running: usize,
    completed: usize,
    failed: usize,
    avg_completed_duration_label: String,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionRunFilterView {
    selected_lane: String,
    selected_status: String,
    selected_attention: String,
    lane_options: Vec<String>,
    status_options: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionLaneRuntimeView {
    lane: String,
    completed_runs: i64,
    p50_duration_label: String,
    p95_duration_label: String,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionPerformanceSloView {
    oldest_queued_label: String,
    oldest_queued_attention: bool,
    runs_24h: i64,
    failed_24h: i64,
    failure_rate_24h_label: String,
    failure_attention: bool,
    lane_runtimes: Vec<ReflectionLaneRuntimeView>,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionRunDetailView {
    run: ReflectionRunView,
    input_summary: Value,
    input_summary_pretty: String,
    output_summary_pretty: String,
    proposal_ids: Vec<String>,
    linked_proposals: Vec<MemoryProposalView>,
    item_count: i64,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionEvidenceConversationView {
    id: String,
    external_conversation_id: Option<String>,
    current_title: Option<String>,
    href: String,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionEvidenceArtifactView {
    id: String,
    conversation_id: String,
    external_conversation_id: Option<String>,
    conversation_href: String,
    artifact_kind: String,
    policy_version: String,
    trigger: String,
    source_message_start_seq: i64,
    source_message_end_seq: i64,
    source_group_start: Option<i32>,
    source_group_end: Option<i32>,
    superseded_by: Option<String>,
    created_at: String,
    evidence_source: String,
    artifact_json_pretty: String,
}

#[derive(Debug, Serialize, Clone)]
struct ReflectionEvidenceView {
    detail: ReflectionRunDetailView,
    conversations: Vec<ReflectionEvidenceConversationView>,
    artifacts: Vec<ReflectionEvidenceArtifactView>,
    artifact_ref_count: usize,
    conversation_ref_count: usize,
}

#[derive(Debug, Serialize, Clone)]
struct MemoryProposalView {
    id: String,
    store: String,
    source_profile: String,
    source_agent_id: Option<String>,
    source_paths: Vec<String>,
    source_refs: Value,
    suggested_action: String,
    target_ref: Option<String>,
    title: String,
    summary: String,
    rationale: String,
    proposed_content: Option<String>,
    proposed_patch: Option<String>,
    refs: Value,
    sensitivity: String,
    requires_human: bool,
    status: String,
    reviewer_profile: Option<String>,
    reviewer_agent_id: Option<String>,
    review_notes: Option<String>,
    decision_summary: Option<String>,
    created_at: String,
    reviewed_at: Option<String>,
    extraction: MemoryProposalExtractionView,
}

#[derive(Debug, Serialize, Clone, Default)]
struct MemoryProposalExtractionView {
    is_memory_extraction: bool,
    source_kind: Option<String>,
    source_ref: Option<String>,
    candidate_kind: Option<String>,
    source_message_count: usize,
    source_artifact_count: usize,
    confidence_label: Option<String>,
    detector: Option<String>,
    discarded_count: usize,
    discarded_preview: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EntityDetail {
    entity_id: String,
    sequence_no: i64,
    entity_type: String,
    display_name: String,
    resolution: String,
    trust: String,
    canonical_ref: Option<String>,
    superseded_by_entity_id: Option<String>,
    metadata_json: Value,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct HandleItem {
    handle_type: String,
    handle_value: String,
    source: Option<String>,
    trust: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct CountBucket {
    label: String,
    count: i64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn snippet(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let head: String = collapsed.chars().take(max).collect();
        format!("{head}…")
    }
}

fn scope_label(scope_type: store::MemoryScopeType, scope_profile: Option<&str>) -> String {
    match scope_type {
        store::MemoryScopeType::Shared => "core".to_string(),
        store::MemoryScopeType::Hat => "hat".to_string(),
        store::MemoryScopeType::SourceLocal => "session".to_string(),
        store::MemoryScopeType::ProfileLocal => {
            scope_profile.unwrap_or("legacy profile").to_string()
        }
    }
}

/// Derived from authenticated Bear membership, never from a path, record ID, or request parameter.
/// Only the explicit admin variant can inspect raw, legacy, or review data.
enum MemoryLibraryViewer {
    AdminInspection,
    Curated(CuratedMemoryGrant),
}

impl MemoryLibraryViewer {
    async fn resolve(
        state: &AppState,
        bear_id: Uuid,
        can_manage_bear: bool,
    ) -> Result<Self, CustomError> {
        if can_manage_bear {
            return Ok(Self::AdminInspection);
        }
        // Hats belong to the Bear, not an individual session. An absent/revoked hat
        // never becomes visible merely because SQLite still contains its records.
        let hats = hats::list_hats(state.sqlx_pool(), BearId::new(bear_id)).await?;
        Ok(Self::Curated(CuratedMemoryGrant::new(
            hats.into_iter().map(|hat| hat.id).collect(),
        )))
    }

    fn require_admin_review(&self) -> Result<(), CustomError> {
        match self {
            Self::AdminInspection => Ok(()),
            Self::Curated(_) => Err(CustomError::Authorization(
                "bear admin role required".to_string(),
            )),
        }
    }

    async fn recent(
        &self,
        manager: &MemoryStoreManager,
        bear_id: Uuid,
        limit: i64,
    ) -> Result<Vec<MemoryRecordRow>, CustomError> {
        Ok(match self {
            Self::AdminInspection => list_recent_memory_records(manager, bear_id, limit).await?,
            Self::Curated(grant) => {
                library::recent(&manager.store_for_bear(bear_id).await?, grant, limit).await?
            }
        })
    }

    async fn search(
        &self,
        manager: &MemoryStoreManager,
        bear_id: Uuid,
        query: &str,
        limit: i64,
    ) -> Result<Vec<MemoryRecordRow>, CustomError> {
        Ok(match self {
            Self::AdminInspection => search_memory_records(manager, bear_id, query, limit).await?,
            Self::Curated(grant) => {
                library::search(&manager.store_for_bear(bear_id).await?, grant, query, limit)
                    .await?
            }
        })
    }

    async fn browse(
        &self,
        manager: &MemoryStoreManager,
        bear_id: Uuid,
    ) -> Result<Vec<PathSummary>, CustomError> {
        Ok(match self {
            Self::AdminInspection => list_path_summaries(manager, bear_id).await?,
            Self::Curated(grant) => {
                library::browse(&manager.store_for_bear(bear_id).await?, grant).await?
            }
        })
    }

    async fn detail(
        &self,
        manager: &MemoryStoreManager,
        bear_id: Uuid,
        memory_id: &str,
    ) -> Result<Option<store::MemoryRecordDetail>, CustomError> {
        Ok(match self {
            Self::AdminInspection => get_memory_record_detail(manager, bear_id, memory_id).await?,
            Self::Curated(grant) => {
                library::detail(&manager.store_for_bear(bear_id).await?, grant, memory_id).await?
            }
        })
    }

    async fn history(
        &self,
        store: &store::BearMemoryStore,
        record: &store::MemoryRecordDetail,
        limit: i64,
    ) -> Result<Vec<MemoryRecordRow>, CustomError> {
        Ok(match self {
            Self::AdminInspection => match record.logical_path.as_deref() {
                Some(path) => {
                    store::list_record_history_for_logical_path(store, path, limit).await?
                }
                None => vec![],
            },
            Self::Curated(grant) => {
                library::history(store, grant, &record.memory_id, limit).await?
            }
        })
    }
}

/// Strip the `den.memory.relation.` prefix to a short, human label.
fn relation_label(relation: &str) -> String {
    relation
        .rsplit('.')
        .next()
        .unwrap_or(relation)
        .replace('_', " ")
}

fn class_label(class: &store::RelationClass) -> &'static str {
    match class {
        store::RelationClass::AccessBearing => "access-bearing",
        store::RelationClass::Descriptive => "descriptive",
    }
}

fn duration_label(milliseconds: Option<i128>) -> String {
    let Some(milliseconds) = milliseconds.filter(|value| *value >= 0) else {
        return "—".to_string();
    };
    let seconds = milliseconds / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes}m")
}

fn elapsed_ms(start: Option<OffsetDateTime>, end: Option<OffsetDateTime>) -> Option<i128> {
    let start = start?;
    let end = end.unwrap_or_else(OffsetDateTime::now_utc);
    Some((end - start).whole_milliseconds())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn valid_reflection_lane(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if matches!(
        value,
        "memory_curate" | "archive_harvest" | "recall_index" | "context_compact"
    ) {
        Some(value.to_string())
    } else {
        None
    }
}

fn valid_reflection_status(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if matches!(
        value,
        "queued"
            | "running"
            | "completed"
            | "failed"
            | "cancelled"
            | "skipped"
            | "needs_human_review"
    ) {
        Some(value.to_string())
    } else {
        None
    }
}

fn valid_reflection_attention(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if matches!(value, "failed_today" | "queued_over_10m" | "slow_completed") {
        Some(value.to_string())
    } else {
        None
    }
}

fn reflection_run_filter_view(
    lane: Option<&str>,
    status: Option<&str>,
    attention: Option<&str>,
) -> ReflectionRunFilterView {
    ReflectionRunFilterView {
        selected_lane: lane.unwrap_or("all").to_string(),
        selected_status: status.unwrap_or("all").to_string(),
        selected_attention: attention.unwrap_or("all").to_string(),
        lane_options: [
            "memory_curate",
            "archive_harvest",
            "recall_index",
            "context_compact",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        status_options: [
            "queued",
            "running",
            "completed",
            "failed",
            "cancelled",
            "skipped",
            "needs_human_review",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    }
}

fn proposal_ids_from_summary(summary: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for key in [
        "proposal_ids",
        "created_proposal_ids",
        "resolved_proposal_ids",
    ] {
        if let Some(values) = summary.get(key).and_then(Value::as_array) {
            for value in values {
                if let Some(id) = value.as_str() {
                    if !ids.iter().any(|existing| existing == id) {
                        ids.push(id.to_string());
                    }
                }
            }
        }
    }
    if let Some(outcomes) = summary.get("outcomes").and_then(Value::as_array) {
        for outcome in outcomes {
            if let Some(id) = outcome.get("proposal_id").and_then(Value::as_str) {
                if !ids.iter().any(|existing| existing == id) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids
}

fn proposal_count_from_summary(output: &Value) -> Option<usize> {
    let proposal_count = proposal_ids_from_summary(output).len();
    if proposal_count > 0 {
        Some(proposal_count)
    } else {
        output
            .get("outcomes")
            .and_then(Value::as_array)
            .map(Vec::len)
    }
}

fn scanned_artifacts_from_summary(output: &Value) -> Option<i64> {
    output.get("scanned_artifacts").and_then(Value::as_i64)
}

fn i64_field_from_summary(output: &Value, key: &str) -> Option<i64> {
    output.get(key).and_then(Value::as_i64)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn array_len_field(value: &Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn proposal_extraction_view(source_refs: &Value, refs: &Value) -> MemoryProposalExtractionView {
    let quality = refs.get("quality").unwrap_or(&Value::Null);
    let discarded = refs
        .get("discarded")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let discarded_preview = discarded
        .iter()
        .filter_map(|value| string_field(value, "reason"))
        .take(3)
        .collect();
    let confidence_label = quality
        .get("confidence")
        .and_then(Value::as_f64)
        .map(|value| format!("{value:.2}"));
    let is_memory_extraction = source_refs.get("source").and_then(Value::as_str)
        == Some("memory_extraction")
        || refs
            .get("memory_extraction")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    MemoryProposalExtractionView {
        is_memory_extraction,
        source_kind: string_field(source_refs, "source_kind"),
        source_ref: string_field(source_refs, "source_ref"),
        candidate_kind: string_field(source_refs, "candidate_kind"),
        source_message_count: array_len_field(source_refs, "source_message_ids"),
        source_artifact_count: array_len_field(source_refs, "source_artifact_ids"),
        confidence_label,
        detector: string_field(quality, "detector"),
        discarded_count: discarded.len(),
        discarded_preview,
    }
}

fn reflection_summary(runs: &[ReflectionRunView]) -> ReflectionRunSummary {
    let mut completed_durations = Vec::new();
    for run in runs {
        if run.status == "completed" {
            if let (Some(started), Some(completed)) = (&run.started_at, &run.completed_at) {
                if let (Ok(started), Ok(completed)) = (
                    OffsetDateTime::parse(started, &time::format_description::well_known::Rfc3339),
                    OffsetDateTime::parse(
                        completed,
                        &time::format_description::well_known::Rfc3339,
                    ),
                ) {
                    completed_durations.push((completed - started).whole_milliseconds());
                }
            }
        }
    }
    let avg = if completed_durations.is_empty() {
        None
    } else {
        Some(
            completed_durations.iter().sum::<i128>()
                / i128::try_from(completed_durations.len()).unwrap_or(1),
        )
    };
    ReflectionRunSummary {
        total: runs.len(),
        queued: runs.iter().filter(|run| run.status == "queued").count(),
        running: runs
            .iter()
            .filter(|run| matches!(run.status.as_str(), "running" | "started"))
            .count(),
        completed: runs.iter().filter(|run| run.status == "completed").count(),
        failed: runs.iter().filter(|run| run.status == "failed").count(),
        avg_completed_duration_label: duration_label(avg),
    }
}

async fn list_recent_reflection_runs(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    limit: i64,
    lane_filter: Option<&str>,
    status_filter: Option<&str>,
    attention_filter: Option<&str>,
) -> Vec<ReflectionRunView> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            serde_json::Value,
            Option<String>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
            OffsetDateTime,
            Option<String>,
            Option<String>,
            Option<time::Date>,
        ),
    >(
        r"
        SELECT id, lane, trigger, status, output_summary, error, started_at, completed_at, created_at,
               conversation_id, conversation_key, conversation_date
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane IN ('memory_curate', 'archive_harvest', 'recall_index', 'context_compact')
          AND ($3::text IS NULL OR lane = $3)
          AND ($4::text IS NULL OR status = $4)
          AND (
              $5::text IS NULL
              OR ($5 = 'failed_today' AND status = 'failed' AND created_at >= date_trunc('day', NOW()))
              OR ($5 = 'queued_over_10m' AND status = 'queued' AND created_at <= NOW() - INTERVAL '10 minutes')
              OR (
                  $5 = 'slow_completed'
                  AND status = 'completed'
                  AND started_at IS NOT NULL
                  AND completed_at IS NOT NULL
                  AND completed_at - started_at >= INTERVAL '5 minutes'
              )
          )
        ORDER BY created_at DESC
        LIMIT $2
        ",
    )
    .bind(bear_id)
    .bind(limit.clamp(1, 200))
    .bind(lane_filter)
    .bind(status_filter)
    .bind(attention_filter)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(
            |(
                id,
                lane,
                trigger,
                status,
                output_summary,
                error,
                started_at,
                completed_at,
                created_at,
                conversation_id,
                conversation_key,
                conversation_date,
            )| {
                let queue_wait_ms = elapsed_ms(Some(created_at), started_at.or(completed_at));
                let duration_ms = elapsed_ms(started_at, completed_at);
                let queued_age_ms = if status == "queued" {
                    elapsed_ms(Some(created_at), None)
                } else {
                    None
                };
                let id = id.to_string();
                ReflectionRunView {
                    short_id: short_id(&id),
                    id,
                    lane,
                    trigger,
                    status_label: match status.as_str() {
                        "queued" => "queued".to_string(),
                        "running" | "started" => "running".to_string(),
                        "completed" => "completed".to_string(),
                        "failed" => "failed".to_string(),
                        other => other.to_string(),
                    },
                    status,
                    created_at: created_at.to_string(),
                    started_at: started_at.map(|value| value.to_string()),
                    completed_at: completed_at.map(|value| value.to_string()),
                    conversation_id,
                    conversation_key,
                    conversation_date: conversation_date.map(|value| value.to_string()),
                    queue_wait_label: duration_label(queue_wait_ms),
                    duration_label: duration_label(duration_ms),
                    queued_age_label: duration_label(queued_age_ms),
                    proposal_count: proposal_count_from_summary(&output_summary),
                    scanned_artifacts: scanned_artifacts_from_summary(&output_summary),
                    candidate_count: i64_field_from_summary(&output_summary, "candidate_count"),
                    discarded_count: i64_field_from_summary(&output_summary, "discarded_count"),
                    no_candidate_count: i64_field_from_summary(
                        &output_summary,
                        "no_candidate_count",
                    ),
                    output_summary,
                    error,
                }
            },
        )
        .collect()
}

async fn reflection_performance_slo(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> ReflectionPerformanceSloView {
    let (oldest_queued_at, runs_24h, failed_24h) = sqlx::query_as::<
        _,
        (Option<OffsetDateTime>, i64, i64),
    >(
        r"
        SELECT
            MIN(created_at) FILTER (WHERE status = 'queued') AS oldest_queued_at,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '24 hours')::bigint AS runs_24h,
            COUNT(*) FILTER (WHERE created_at >= NOW() - INTERVAL '24 hours' AND status = 'failed')::bigint AS failed_24h
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane IN ('memory_curate', 'archive_harvest', 'recall_index', 'context_compact')
        ",
    )
    .bind(bear_id)
    .fetch_one(pool)
    .await
    .unwrap_or((None, 0, 0));

    let oldest_queued_ms = oldest_queued_at
        .map(|created_at| (OffsetDateTime::now_utc() - created_at).whole_milliseconds());
    let failure_rate = if runs_24h > 0 {
        (failed_24h as f64 / runs_24h as f64) * 100.0
    } else {
        0.0
    };

    let lane_rows = sqlx::query_as::<_, (String, i64, Option<f64>, Option<f64>)>(
        r"
        SELECT lane,
               COUNT(*)::bigint AS completed_runs,
               percentile_cont(0.50) WITHIN GROUP (
                   ORDER BY EXTRACT(EPOCH FROM (completed_at - started_at)) * 1000
               ) AS p50_ms,
               percentile_cont(0.95) WITHIN GROUP (
                   ORDER BY EXTRACT(EPOCH FROM (completed_at - started_at)) * 1000
               ) AS p95_ms
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND lane IN ('memory_curate', 'archive_harvest', 'recall_index', 'context_compact')
          AND status = 'completed'
          AND started_at IS NOT NULL
          AND completed_at IS NOT NULL
          AND completed_at >= NOW() - INTERVAL '7 days'
        GROUP BY lane
        ORDER BY lane ASC
        ",
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    ReflectionPerformanceSloView {
        oldest_queued_label: duration_label(oldest_queued_ms),
        oldest_queued_attention: oldest_queued_ms
            .map(|ms| ms >= Duration::minutes(10).whole_milliseconds())
            .unwrap_or(false),
        runs_24h,
        failed_24h,
        failure_rate_24h_label: format!("{failure_rate:.0}%"),
        failure_attention: failed_24h > 0,
        lane_runtimes: lane_rows
            .into_iter()
            .map(
                |(lane, completed_runs, p50_ms, p95_ms)| ReflectionLaneRuntimeView {
                    lane,
                    completed_runs,
                    p50_duration_label: duration_label(p50_ms.map(|value| value.round() as i128)),
                    p95_duration_label: duration_label(p95_ms.map(|value| value.round() as i128)),
                },
            )
            .collect(),
    }
}

async fn get_reflection_run_detail(
    pool: &sqlx::PgPool,
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    run_id: Uuid,
) -> Result<Option<ReflectionRunDetailView>, CustomError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            serde_json::Value,
            serde_json::Value,
            Option<String>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
            OffsetDateTime,
            Option<String>,
            Option<String>,
            Option<time::Date>,
        ),
    >(
        r"
        SELECT id, lane, trigger, status, input_summary, output_summary, error,
               started_at, completed_at, created_at, conversation_id, conversation_key, conversation_date
        FROM bear_reflection_runs
        WHERE bear_id = $1
          AND id = $2
          AND lane IN ('memory_curate', 'archive_harvest', 'recall_index', 'context_compact')
        ",
    )
    .bind(bear_id)
    .bind(run_id)
    .fetch_optional(pool)
    .await?;

    let Some((
        id,
        lane,
        trigger,
        status,
        input_summary,
        output_summary,
        error,
        started_at,
        completed_at,
        created_at,
        conversation_id,
        conversation_key,
        conversation_date,
    )) = row
    else {
        return Ok(None);
    };

    let queue_wait_ms = elapsed_ms(Some(created_at), started_at.or(completed_at));
    let duration_ms = elapsed_ms(started_at, completed_at);
    let queued_age_ms = if status == "queued" {
        elapsed_ms(Some(created_at), None)
    } else {
        None
    };
    let output_proposal_ids = proposal_ids_from_summary(&output_summary);
    let input_proposal_ids = proposal_ids_from_summary(&input_summary);
    let mut proposal_ids = input_proposal_ids;
    for id in output_proposal_ids {
        if !proposal_ids.iter().any(|existing| existing == &id) {
            proposal_ids.push(id);
        }
    }

    let mut linked_proposals = Vec::new();
    for proposal_id in &proposal_ids {
        if let Ok(uuid) = Uuid::parse_str(proposal_id) {
            if let Some(row) = memory_proposals::get_for_bear(pool, bear_id, uuid).await? {
                linked_proposals.push(proposal_view_from_postgres(row));
                continue;
            }
        }
        if let Ok(store) = manager.store_for_bear(bear_id).await {
            if let Some(row) = get_sqlite_memory_proposal(&store, proposal_id).await? {
                linked_proposals.push(proposal_view_from_sqlite(row));
            }
        }
    }

    let item_count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*)::bigint AS "count!: i64"
        FROM bear_reflection_run_items
        WHERE run_id = $1
        "#,
        run_id,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let id = id.to_string();
    Ok(Some(ReflectionRunDetailView {
        run: ReflectionRunView {
            short_id: short_id(&id),
            id,
            lane,
            trigger,
            status_label: match status.as_str() {
                "queued" => "queued".to_string(),
                "running" | "started" => "running".to_string(),
                "completed" => "completed".to_string(),
                "failed" => "failed".to_string(),
                other => other.to_string(),
            },
            status,
            created_at: created_at.to_string(),
            started_at: started_at.map(|value| value.to_string()),
            completed_at: completed_at.map(|value| value.to_string()),
            conversation_id,
            conversation_key,
            conversation_date: conversation_date.map(|value| value.to_string()),
            queue_wait_label: duration_label(queue_wait_ms),
            duration_label: duration_label(duration_ms),
            queued_age_label: duration_label(queued_age_ms),
            proposal_count: proposal_count_from_summary(&output_summary),
            scanned_artifacts: scanned_artifacts_from_summary(&output_summary),
            candidate_count: i64_field_from_summary(&output_summary, "candidate_count"),
            discarded_count: i64_field_from_summary(&output_summary, "discarded_count"),
            no_candidate_count: i64_field_from_summary(&output_summary, "no_candidate_count"),
            output_summary: output_summary.clone(),
            error,
        },
        input_summary_pretty: serde_json::to_string_pretty(&input_summary)
            .unwrap_or_else(|_| "{}".to_string()),
        output_summary_pretty: serde_json::to_string_pretty(&output_summary)
            .unwrap_or_else(|_| "{}".to_string()),
        input_summary,
        proposal_ids,
        linked_proposals,
        item_count,
    }))
}

fn collect_uuid(value: &Value, key: &str, out: &mut BTreeSet<Uuid>) {
    match value {
        Value::Object(map) => {
            for (field, nested) in map {
                if field == key {
                    if let Some(id) = nested
                        .as_str()
                        .and_then(|value| Uuid::parse_str(value).ok())
                    {
                        out.insert(id);
                    } else if let Some(values) = nested.as_array() {
                        for value in values {
                            if let Some(id) =
                                value.as_str().and_then(|value| Uuid::parse_str(value).ok())
                            {
                                out.insert(id);
                            }
                        }
                    }
                }
                collect_uuid(nested, key, out);
            }
        }
        Value::Array(values) => {
            for nested in values {
                collect_uuid(nested, key, out);
            }
        }
        _ => {}
    }
}

fn collect_string_field(value: &Value, key: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (field, nested) in map {
                if field == key {
                    if let Some(text) = nested
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        out.insert(text.to_string());
                    }
                }
                collect_string_field(nested, key, out);
            }
        }
        Value::Array(values) => {
            for nested in values {
                collect_string_field(nested, key, out);
            }
        }
        _ => {}
    }
}

fn collect_evidence_refs(
    detail: &ReflectionRunDetailView,
) -> (BTreeSet<Uuid>, BTreeSet<Uuid>, BTreeSet<String>) {
    let mut artifact_ids = BTreeSet::new();
    let mut conversation_uuids = BTreeSet::new();
    let mut external_conversation_ids = BTreeSet::new();

    for value in [&detail.input_summary, &detail.run.output_summary] {
        collect_uuid(value, "artifact_id", &mut artifact_ids);
        collect_uuid(value, "artifact_ids", &mut artifact_ids);
        collect_uuid(value, "conversation_uuid", &mut conversation_uuids);
        collect_string_field(value, "conversation_id", &mut external_conversation_ids);
    }
    if let Some(conversation_id) = detail.run.conversation_id.as_deref() {
        if let Ok(id) = Uuid::parse_str(conversation_id) {
            conversation_uuids.insert(id);
        } else if !conversation_id.trim().is_empty() {
            external_conversation_ids.insert(conversation_id.to_string());
        }
    }
    for proposal in &detail.linked_proposals {
        for value in [&proposal.source_refs, &proposal.refs] {
            collect_uuid(value, "artifact_id", &mut artifact_ids);
            collect_uuid(value, "artifact_ids", &mut artifact_ids);
            collect_uuid(value, "conversation_uuid", &mut conversation_uuids);
            collect_string_field(value, "conversation_id", &mut external_conversation_ids);
        }
    }

    (artifact_ids, conversation_uuids, external_conversation_ids)
}

async fn load_evidence_conversations(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    conversation_uuids: &BTreeSet<Uuid>,
    external_conversation_ids: &BTreeSet<String>,
    slug: &str,
) -> Result<Vec<ReflectionEvidenceConversationView>, CustomError> {
    let uuid_values: Vec<Uuid> = conversation_uuids.iter().copied().collect();
    let external_values: Vec<String> = external_conversation_ids.iter().cloned().collect();
    let rows = sqlx::query_as::<_, (Uuid, Option<String>, Option<String>)>(
        r"
        SELECT id, external_conversation_id, current_title
        FROM conversations
        WHERE bear_id = $1
          AND (
              id = ANY($2::uuid[])
              OR external_conversation_id = ANY($3::text[])
          )
        ORDER BY updated_at DESC
        LIMIT 50
        ",
    )
    .bind(bear_id)
    .bind(&uuid_values)
    .bind(&external_values)
    .fetch_all(pool)
    .await
    .map_err(|err| CustomError::Database(format!("load evidence conversations: {err}")))?;

    Ok(rows
        .into_iter()
        .map(
            |(id, external_conversation_id, current_title)| ReflectionEvidenceConversationView {
                id: id.to_string(),
                external_conversation_id,
                current_title,
                href: format!("/bear/{slug}/conversations/{id}"),
            },
        )
        .collect())
}

async fn load_evidence_artifacts(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    artifact_ids: &BTreeSet<Uuid>,
    conversations: &[ReflectionEvidenceConversationView],
    slug: &str,
) -> Result<Vec<ReflectionEvidenceArtifactView>, CustomError> {
    let artifact_values: Vec<Uuid> = artifact_ids.iter().copied().collect();
    let conversation_values: Vec<Uuid> = conversations
        .iter()
        .filter_map(|conversation| Uuid::parse_str(&conversation.id).ok())
        .collect();
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Option<String>,
            String,
            String,
            String,
            i64,
            i64,
            Option<i32>,
            Option<i32>,
            serde_json::Value,
            Option<Uuid>,
            OffsetDateTime,
        ),
    >(
        r"
        SELECT a.id,
               a.conversation_id,
               c.external_conversation_id,
               a.artifact_kind,
               a.policy_version,
               a.trigger,
               a.source_message_start_seq,
               a.source_message_end_seq,
               a.source_group_start,
               a.source_group_end,
               a.artifact_json,
               a.superseded_by,
               a.created_at
        FROM conversation_compaction_artifacts a
        JOIN conversations c ON c.id = a.conversation_id
        WHERE c.bear_id = $1
          AND (
              a.id = ANY($2::uuid[])
              OR a.conversation_id = ANY($3::uuid[])
          )
        ORDER BY a.created_at DESC
        LIMIT 100
        ",
    )
    .bind(bear_id)
    .bind(&artifact_values)
    .bind(&conversation_values)
    .fetch_all(pool)
    .await
    .map_err(|err| CustomError::Database(format!("load evidence artifacts: {err}")))?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                conversation_id,
                external_conversation_id,
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
            )| ReflectionEvidenceArtifactView {
                evidence_source: if artifact_ids.contains(&id) {
                    "linked artifact".to_string()
                } else {
                    "conversation artifact".to_string()
                },
                id: id.to_string(),
                conversation_id: conversation_id.to_string(),
                external_conversation_id,
                conversation_href: format!("/bear/{slug}/conversations/{conversation_id}"),
                artifact_kind,
                policy_version,
                trigger,
                source_message_start_seq,
                source_message_end_seq,
                source_group_start,
                source_group_end,
                superseded_by: superseded_by.map(|id| id.to_string()),
                created_at: created_at.to_string(),
                artifact_json_pretty: serde_json::to_string_pretty(&artifact_json)
                    .unwrap_or_else(|_| "{}".to_string()),
            },
        )
        .collect())
}

async fn get_reflection_evidence(
    pool: &sqlx::PgPool,
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    slug: &str,
    run_id: Uuid,
) -> Result<Option<ReflectionEvidenceView>, CustomError> {
    let Some(detail) = get_reflection_run_detail(pool, manager, bear_id, run_id).await? else {
        return Ok(None);
    };
    let (artifact_ids, conversation_uuids, external_conversation_ids) =
        collect_evidence_refs(&detail);
    let conversations = load_evidence_conversations(
        pool,
        bear_id,
        &conversation_uuids,
        &external_conversation_ids,
        slug,
    )
    .await?;
    let artifacts =
        load_evidence_artifacts(pool, bear_id, &artifact_ids, &conversations, slug).await?;
    Ok(Some(ReflectionEvidenceView {
        detail,
        conversations,
        artifacts,
        artifact_ref_count: artifact_ids.len(),
        conversation_ref_count: conversation_uuids.len() + external_conversation_ids.len(),
    }))
}

fn proposal_view_from_postgres(row: memory_proposals::MemoryProposalRow) -> MemoryProposalView {
    let extraction = proposal_extraction_view(&row.source_refs, &row.refs);
    MemoryProposalView {
        id: row.id.to_string(),
        store: "postgres".to_string(),
        source_profile: row.source_profile,
        source_agent_id: row.source_agent_id,
        source_paths: row.source_paths,
        source_refs: row.source_refs,
        suggested_action: row.suggested_action,
        target_ref: row.target_ref,
        title: row.title,
        summary: row.summary,
        rationale: row.rationale,
        proposed_content: row.proposed_content,
        proposed_patch: row.proposed_patch,
        refs: row.refs,
        sensitivity: row.sensitivity,
        requires_human: row.requires_human,
        status: row.status,
        reviewer_profile: row.reviewer_profile,
        reviewer_agent_id: row.reviewer_agent_id,
        review_notes: row.review_notes,
        decision_summary: row.decision_summary,
        created_at: row.created_at.to_string(),
        reviewed_at: row.reviewed_at.map(|value| value.to_string()),
        extraction,
    }
}

fn proposal_view_from_sqlite(row: SqliteMemoryProposal) -> MemoryProposalView {
    let payload = row.payload_json;
    let source_refs = payload
        .get("source_refs")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let refs = payload.get("refs").cloned().unwrap_or_else(|| json!({}));
    let extraction = proposal_extraction_view(&source_refs, &refs);
    let string_field = |key: &str, default: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let source_paths = payload
        .get("source_paths")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    MemoryProposalView {
        id: row.proposal_id,
        store: "sqlite".to_string(),
        source_profile: string_field("source_profile", "curate"),
        source_agent_id: payload
            .get("source_agent_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_paths,
        source_refs,
        suggested_action: string_field("suggested_action", "unspecified"),
        target_ref: payload
            .get("target_ref")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: string_field("title", "Memory proposal"),
        summary: string_field("summary", ""),
        rationale: string_field("rationale", ""),
        proposed_content: payload
            .get("proposed_content")
            .and_then(Value::as_str)
            .map(str::to_string),
        proposed_patch: payload
            .get("proposed_patch")
            .and_then(Value::as_str)
            .map(str::to_string),
        refs,
        sensitivity: string_field("sensitivity", "normal"),
        requires_human: payload
            .get("requires_human")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: row.status,
        reviewer_profile: payload
            .get("reviewer_profile")
            .and_then(Value::as_str)
            .map(str::to_string),
        reviewer_agent_id: payload
            .get("reviewer_agent_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        review_notes: payload
            .get("review_notes")
            .and_then(Value::as_str)
            .map(str::to_string),
        decision_summary: payload
            .get("decision_summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        created_at: row.created_at,
        reviewed_at: None,
        extraction,
    }
}

async fn list_dashboard_proposals(
    state: &AppState,
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Vec<MemoryProposalView> {
    let mut proposals: Vec<MemoryProposalView> =
        memory_proposals::list_for_bear(state.sqlx_pool(), bear_id, status, limit)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(proposal_view_from_postgres)
            .collect();

    if let Ok(store) = manager.store_for_bear(bear_id).await {
        proposals.extend(
            list_sqlite_memory_proposals(&store, status, limit)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(proposal_view_from_sqlite),
        );
    }
    proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    proposals.truncate(limit.clamp(1, 200) as usize);
    proposals
}

async fn count_dashboard_proposals(
    state: &AppState,
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    status: &str,
) -> i64 {
    let postgres_count =
        memory_proposals::count_for_bear_status(state.sqlx_pool(), bear_id, status)
            .await
            .unwrap_or(0);
    let sqlite_count = if let Ok(store) = manager.store_for_bear(bear_id).await {
        count_memory_proposals(&store, Some(status))
            .await
            .unwrap_or(0)
    } else {
        0
    };
    postgres_count + sqlite_count
}

fn record_list_item(row: MemoryRecordRow, score: Option<f32>) -> RecordListItem {
    RecordListItem {
        scope_label: scope_label(row.scope_type, row.scope_profile.as_deref()),
        snippet: snippet(&row.content_text, 240),
        memory_id: row.memory_id,
        kind: row.kind,
        logical_path: row.logical_path,
        created_at: row.created_at,
        sequence_no: row.sequence_no,
        score,
    }
}

/// First path segment (`core`, `pair`, `work`, …) used to group the library browse view.
fn path_group_label(logical_path: &str) -> String {
    logical_path
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("other")
        .to_string()
}

// ---------------------------------------------------------------------------
// Dashboard — "how much memory"
// ---------------------------------------------------------------------------

const LEGACY_IMPORT_MAX_UPLOAD_BYTES: usize = 128 * 1024 * 1024;

fn dashboard_redirect_with_query(slug: &str, key: &str, message: &str) -> Response {
    let encoded = urlencoding::encode(message);
    Redirect::to(&format!("/bear/{slug}/memory?{key}={encoded}")).into_response()
}

fn collect_review_source_refs(
    source_refs: &Value,
    session_ids: &mut BTreeSet<String>,
    conversation_ids: &mut BTreeSet<String>,
) {
    for key in ["session_id", "client_session_id"] {
        if let Some(value) = source_refs
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            session_ids.insert(value.to_string());
        }
    }
    for key in ["conversation_id", "conversation_uuid"] {
        if let Some(value) = source_refs
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            conversation_ids.insert(value.to_string());
        }
    }
}

async fn requeue_source_reflection_sessions(
    state: &AppState,
    bear_id: Uuid,
    session_ids: &BTreeSet<String>,
    conversation_ids: &BTreeSet<String>,
) -> Result<usize, CustomError> {
    if session_ids.is_empty() && conversation_ids.is_empty() {
        return Ok(0);
    }
    let session_ids = session_ids.iter().cloned().collect::<Vec<_>>();
    let conversation_ids = conversation_ids.iter().cloned().collect::<Vec<_>>();
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT
            client_session_id AS "client_session_id!: String",
            user_id AS "user_id!: i32"
        FROM client_sessions
        WHERE bear_id = $1
          AND (
              client_session_id = ANY($2::text[])
              OR conversation_id = ANY($3::text[])
              OR resolved_conversation_id = ANY($3::text[])
          )
        "#,
        bear_id,
        &session_ids,
        &conversation_ids,
    )
    .fetch_all(state.sqlx_pool())
    .await?;

    let mut requeued = 0;
    for row in rows {
        let session_id = row.client_session_id;
        let user_id = row.user_id;
        let mut event = BearWireEvent::ephemeral(
            "session.reflection_requeued",
            json!({
                "reason": "memory_review_queue_cleared",
                "source_session_ids": &session_ids,
                "source_conversation_ids": &conversation_ids,
            }),
        );
        event.bear_id = Some(bear_id.to_string());
        event.human_id = Some(user_id.to_string());
        event.session_id = Some(session_id.clone());
        den_runtime::bearwire_events::append_bearwire_event(
            state.sqlx_pool(),
            &session_id,
            Some(bear_id),
            Some(user_id),
            event,
        )
        .await?;
        requeued += 1;
    }
    Ok(requeued)
}

async fn clear_review_queue_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
    Form(form): Form<ClearReviewQueueForm>,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }
    if form.confirm.trim() != "clear review queue" {
        return Ok(dashboard_redirect_with_query(
            &bear.slug,
            "review_error",
            "Type “clear review queue” to confirm clearing reviewable memory proposals.",
        ));
    }

    let mut cleared = 0usize;
    let mut session_ids = BTreeSet::new();
    let mut conversation_ids = BTreeSet::new();
    for proposal in memory_proposals::list_reviewable_for_bear(state.sqlx_pool(), bear.id).await? {
        collect_review_source_refs(
            &proposal.source_refs,
            &mut session_ids,
            &mut conversation_ids,
        );
        memory_proposals::resolve_for_bear(
            state.sqlx_pool(),
            memory_proposals::ProposalResolutionParams {
                bear_id: bear.id,
                proposal_id: proposal.id,
                reviewer_profile: BearProfile::Curate,
                reviewer_agent_id: None,
                status: "rejected",
                review_notes: Some("Cleared from the review queue; source session marked for reflection retry when identifiable."),
                decision_summary: Some("Queue cleared without accepting proposal."),
                result_path: None,
                result_commit: None,
                project_to_conversation: false,
            },
        )
        .await?;
        cleared += 1;
    }

    let manager = state.memory_stores.clone();
    if let Ok(store) = manager.store_for_bear(bear.id).await {
        for proposal in list_reviewable_memory_proposals(&store).await? {
            let source_refs = proposal
                .payload_json
                .get("source_refs")
                .cloned()
                .unwrap_or_else(|| json!({}));
            collect_review_source_refs(&source_refs, &mut session_ids, &mut conversation_ids);
            resolve_sqlite_memory_proposal(
                &store,
                &proposal.proposal_id,
                "rejected",
                &json!({
                    "reviewer_profile": BearProfile::Curate.as_str(),
                    "review_notes": "Cleared from the review queue; source session marked for reflection retry when identifiable.",
                    "decision_summary": "Queue cleared without accepting proposal.",
                }),
            )
            .await?;
            cleared += 1;
        }
    }

    let requeued =
        requeue_source_reflection_sessions(&state, bear.id, &session_ids, &conversation_ids)
            .await?;
    Ok(dashboard_redirect_with_query(
        &bear.slug,
        "review_notice",
        &format!(
            "Cleared {cleared} reviewable proposal{} and marked {requeued} source session{} for reflection retry.",
            if cleared == 1 { "" } else { "s" },
            if requeued == 1 { "" } else { "s" },
        ),
    ))
}

fn looks_like_git_bundle(bytes: &[u8]) -> bool {
    let first_line = bytes.split(|b| *b == b'\n').next().unwrap_or(&[]);
    if first_line.is_empty() {
        return false;
    }
    String::from_utf8_lossy(first_line)
        .to_ascii_lowercase()
        .contains("git bundle")
}

fn import_dir_for_bear(config: &den_core::config::Config, bear_id: Uuid) -> PathBuf {
    FsPath::new(&config.bear_sqlite_data_dir)
        .join("imports")
        .join(bear_id.to_string())
}

async fn import_staged_bundle(
    state: &AppState,
    bear_id: Uuid,
    bundle_path: &FsPath,
) -> Result<den_memory::LegacyMemoryImportReport, CustomError> {
    let stores = state.memory_stores.clone();
    let store = stores.store_for_bear(bear_id).await?;
    let record_count: i64 =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM memory_records WHERE bear_id = ?")
            .bind(bear_id.to_string())
            .fetch_one(store.pool())
            .await?;
    if record_count > 0 {
        return Err(CustomError::ValidationError(
            "Legacy memory import is disabled for Bears that already have memory records."
                .to_string(),
        ));
    }

    let report = import_legacy_memory_bundle(
        &store,
        bundle_path,
        &LegacyMemoryImportOptions {
            dry_run: false,
            include_workflow_artifacts: false,
            import_history: false,
        },
    )
    .await?;

    let report_path = bundle_path.with_extension("report.json");
    if let Ok(report_json) = serde_json::to_string_pretty(&report) {
        if let Err(err) = std::fs::write(&report_path, report_json) {
            tracing::warn!(bear_id = %bear_id, error = %err, path = %report_path.display(), "failed to write legacy import report next to staged bundle");
        }
    }

    Ok(report)
}

async fn dashboard_view(
    Path(slug): Path<String>,
    Query(query): Query<DashboardQuery>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let id = bear.id;
    let viewer = MemoryLibraryViewer::resolve(&state, id, can_manage_bear).await?;
    let config = state.config.as_ref();
    let manager = state.memory_stores.clone();

    if let MemoryLibraryViewer::Curated(ref grant) = viewer {
        let recent: Vec<_> = viewer
            .recent(&manager, id, 8)
            .await?
            .into_iter()
            .map(|r| record_list_item(r, None))
            .collect();
        let head_count =
            library::count_current_entries(&manager.store_for_bear(id).await?, grant).await?;
        return web::render_template(
            &state,
            "bear/memory/member_dashboard.html",
            auth_session,
            context! {
                recent,
                head_count,
                can_manage_bear,
                native_runtime => true,
                ..bear_nav_context(&bear, "memory"),
            },
        )
        .await;
    }

    let stats = bear_memory_admin_stats(&manager, config, id).await.ok();
    let legacy_import_locked = stats.as_ref().map(|s| s.record_count > 0).unwrap_or(true);
    let head_count = head_entry_count(&manager, id).await.unwrap_or(0);
    let by_kind = count_records_by_kind(&manager, id)
        .await
        .unwrap_or_default();
    let by_profile = count_records_by_profile(&manager, id)
        .await
        .unwrap_or_default();

    // Derived recall coverage (Postgres registry). Only meaningful when recall is configured.
    let recall = if config.qdrant_url.is_some() {
        let (passages, memories) =
            recall_registry::passage_stats(state.sqlx_pool(), id, &config.embedding_standard)
                .await
                .unwrap_or((0, 0));
        // Recall consistency watermark (ADR-0038 §8): lag + last-run failure summary.
        let watermark = match den_service::recall::recall_watermark_for_bear(
            state.sqlx_pool(),
            config,
            &manager,
            id,
        )
        .await
        {
            Ok(watermark) => den_service::recall::recall_status_json(watermark.as_ref()),
            Err(err) => {
                json!({ "available": false, "reason": format!("watermark unavailable: {err}") })
            }
        };
        Some(json!({
            "enabled": true,
            "passages": passages,
            "memories": memories,
            "watermark": watermark,
        }))
    } else {
        None
    };

    // Entity layer summary (ADR-0042). Populated as the Bear resolves entities (Phase 6+);
    // empty for most Bears today.
    let entity_summary = entity_summary(&manager, id)
        .await
        .unwrap_or_else(|_| json!({ "total": 0, "by_type": [], "by_resolution": [] }));

    let recent: Vec<RecordListItem> = list_recent_memory_records(&manager, id, 8)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| record_list_item(r, None))
        .collect();

    let proposals = list_dashboard_proposals(&state, &manager, id, None, 10).await;
    let pending_review_count = count_dashboard_proposals(&state, &manager, id, "pending").await;
    let needs_human_review_count =
        count_dashboard_proposals(&state, &manager, id, "needs_human_review").await;
    let reviewable_proposal_count = pending_review_count + needs_human_review_count;
    let mut pending_proposals =
        list_dashboard_proposals(&state, &manager, id, Some("pending"), 10).await;
    pending_proposals.extend(
        list_dashboard_proposals(&state, &manager, id, Some("needs_human_review"), 10).await,
    );
    pending_proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    pending_proposals.truncate(10);
    let pair_reflection_runs = pair_reflection::list_recent_for_bear(state.sqlx_pool(), id, 8)
        .await
        .unwrap_or_default();
    let reflection_lane = valid_reflection_lane(query.reflection_lane.as_deref());
    let reflection_status = valid_reflection_status(query.reflection_status.as_deref());
    let reflection_attention = valid_reflection_attention(query.reflection_attention.as_deref());
    let reflection_runs = list_recent_reflection_runs(
        state.sqlx_pool(),
        id,
        25,
        reflection_lane.as_deref(),
        reflection_status.as_deref(),
        reflection_attention.as_deref(),
    )
    .await;
    let reflection_run_summary = reflection_summary(
        &list_recent_reflection_runs(state.sqlx_pool(), id, 100, None, None, None).await,
    );
    let reflection_run_filters = reflection_run_filter_view(
        reflection_lane.as_deref(),
        reflection_status.as_deref(),
        reflection_attention.as_deref(),
    );
    let reflection_slo = reflection_performance_slo(state.sqlx_pool(), id).await;

    web::render_template(
        &state,
        "bear/memory/dashboard.html",
        auth_session,
        context! {
            stats,
            head_count,
            by_kind,
            by_profile,
            recall,
            entity_summary,
            recent,
            proposals,
            pending_proposals,
            pending_review_count,
            needs_human_review_count,
            reviewable_proposal_count,
            pair_reflection_runs,
            reflection_runs,
            reflection_run_summary,
            reflection_run_filters,
            reflection_slo,
            import_notice => query.import_notice.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            import_error => query.import_error.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            review_notice => query.review_notice.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            review_error => query.review_error.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            legacy_import_locked,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

async fn entity_summary(manager: &MemoryStoreManager, bear_id: Uuid) -> Result<Value, CustomError> {
    let store = manager.store_for_bear(bear_id).await?;
    let entities = store::list_entities(&store, None, 500).await?;
    let total = entities.len();
    let mut by_type: std::collections::BTreeMap<String, i64> = Default::default();
    let mut by_resolution: std::collections::BTreeMap<String, i64> = Default::default();
    for e in &entities {
        *by_type.entry(e.entity_type.clone()).or_default() += 1;
        *by_resolution
            .entry(e.resolution.as_str().to_string())
            .or_default() += 1;
    }
    let to_buckets = |m: std::collections::BTreeMap<String, i64>| -> Vec<CountBucket> {
        let mut v: Vec<CountBucket> = m
            .into_iter()
            .map(|(label, count)| CountBucket { label, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.label.cmp(&b.label)));
        v
    };
    Ok(json!({
        "total": total,
        "by_type": to_buckets(by_type),
        "by_resolution": to_buckets(by_resolution),
    }))
}

// ---------------------------------------------------------------------------
// Recent additions feed
// ---------------------------------------------------------------------------

async fn recent_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let manager = state.memory_stores.clone();
    let viewer = MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear).await?;
    let records: Vec<RecordListItem> = viewer
        .recent(&manager, bear.id, 50)
        .await?
        .into_iter()
        .map(|r| record_list_item(r, None))
        .collect();
    web::render_template(
        &state,
        "bear/memory/recent.html",
        auth_session,
        context! {
            records,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Search (keyword always; semantic when recall is configured)
// ---------------------------------------------------------------------------

async fn search_view(
    Path(slug): Path<String>,
    Query(query): Query<SearchQuery>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let config = state.config.as_ref();
    let manager = state.memory_stores.clone();
    let viewer = MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear).await?;
    let q = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    // Qdrant stores broad, potentially stale passage text. Members must only search
    // the curated canonical SQLite library until semantic candidates are rechecked.
    let semantic_available =
        can_manage_bear && config.qdrant_url.is_some() && !config.llm_api_url.trim().is_empty();
    let want_semantic = query.mode.as_deref() == Some("semantic") && semantic_available;

    let mut mode_used = "keyword";
    let mut notice: Option<String> = (query.mode.as_deref() == Some("semantic") && !can_manage_bear)
        .then(|| "Semantic search is available only in Bear admin inspection; showing curated keyword results.".to_string());
    let mut results: Vec<RecordListItem> = Vec::new();

    if let Some(q) = q {
        if want_semantic {
            match semantic_search_for_bear(config, bear.id, q, 25).await {
                Ok(projection) if !projection.passages.is_empty() => {
                    mode_used = "semantic";
                    results = projection
                        .passages
                        .into_iter()
                        .map(|p| RecordListItem {
                            memory_id: p.memory_id,
                            kind: p.kind.unwrap_or_else(|| "memory".to_string()),
                            scope_label: String::new(),
                            logical_path: p.logical_path,
                            created_at: String::new(),
                            sequence_no: 0,
                            snippet: snippet(&p.text, 240),
                            score: Some(p.score),
                        })
                        .collect();
                }
                Ok(_) => {
                    notice = Some(
                        "Semantic search returned no matches; showing keyword results.".to_string(),
                    );
                }
                Err(err) => {
                    tracing::warn!(bear_id = %bear.id, error = %err, "semantic search failed; keyword fallback");
                    notice = Some(
                        "Semantic search is unavailable; showing keyword results.".to_string(),
                    );
                }
            }
        }
        if results.is_empty() {
            results = viewer
                .search(&manager, bear.id, q, 50)
                .await?
                .into_iter()
                .map(|r| record_list_item(r, None))
                .collect();
        }
    }

    let result_count = results.len();
    web::render_template(
        &state,
        "bear/memory/search.html",
        auth_session,
        context! {
            q => q.unwrap_or(""),
            results,
            result_count,
            mode_used,
            semantic_available,
            notice,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Browse the library
// ---------------------------------------------------------------------------

async fn browse_view(
    Path(slug): Path<String>,
    Query(query): Query<BrowseQuery>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let manager = state.memory_stores.clone();
    let viewer = MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear).await?;
    let summaries = viewer.browse(&manager, bear.id).await?;

    // Group by first path segment, preserving a stable, meaningful order.
    let mut groups: Vec<PathGroup> = Vec::new();
    for summary in summaries {
        let label = path_group_label(&summary.logical_path);
        if let Some(group) = groups.iter_mut().find(|g| g.label == label) {
            group.paths.push(summary);
        } else {
            groups.push(PathGroup {
                label,
                paths: vec![summary],
            });
        }
    }
    groups.sort_by(|a, b| {
        group_rank(&a.label)
            .cmp(&group_rank(&b.label))
            .then(a.label.cmp(&b.label))
    });

    web::render_template(
        &state,
        "bear/memory/browse.html",
        auth_session,
        context! {
            groups,
            delete_notice => query.deleted,
            review_notice => query.review_requested,
            delete_error => query.error.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

/// Canonical-first ordering for the browse groups (`core` shared memory before role branches).
fn group_rank(label: &str) -> usize {
    match label {
        "core" => 0,
        "pair" => 1,
        "work" => 2,
        "curate" => 3,
        "talk" => 4,
        "watch" => 5,
        _ => 9,
    }
}

async fn import_legacy_memory_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
    mut multipart: Multipart,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }

    let mut bundle_bytes: Option<Vec<u8>> = None;
    while let Some(mut field) = match multipart.next_field().await {
        Ok(field) => field,
        Err(err) => {
            tracing::warn!(bear_id = %bear.id, error = %err, "invalid legacy bundle multipart upload");
            return Ok(dashboard_redirect_with_query(
                &bear.slug,
                "import_error",
                "Invalid multipart upload.",
            ));
        }
    } {
        if field.name() != Some("bundle") {
            continue;
        }
        let mut data = Vec::new();
        while let Some(chunk) = match field.chunk().await {
            Ok(chunk) => chunk,
            Err(err) => {
                tracing::warn!(bear_id = %bear.id, error = %err, "failed reading legacy bundle upload field");
                return Ok(dashboard_redirect_with_query(
                    &bear.slug,
                    "import_error",
                    "Failed reading uploaded bundle.",
                ));
            }
        } {
            if data.len() + chunk.len() > LEGACY_IMPORT_MAX_UPLOAD_BYTES {
                return Ok(dashboard_redirect_with_query(
                    &bear.slug,
                    "import_error",
                    "Bundle exceeds the 128 MiB upload limit.",
                ));
            }
            data.extend_from_slice(&chunk);
        }
        bundle_bytes = Some(data);
        break;
    }

    let bundle_bytes = match bundle_bytes {
        Some(bytes) if !bytes.is_empty() => bytes,
        _ => {
            return Ok(dashboard_redirect_with_query(
                &bear.slug,
                "import_error",
                "Please select a bundle file.",
            ));
        }
    };

    if !looks_like_git_bundle(&bundle_bytes) {
        return Ok(dashboard_redirect_with_query(
            &bear.slug,
            "import_error",
            "Upload must be a git bundle.",
        ));
    }

    let stores = state.memory_stores.clone();
    let store = stores.store_for_bear(bear.id).await?;
    let record_count: i64 =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM memory_records WHERE bear_id = ?")
            .bind(bear.id.to_string())
            .fetch_one(store.pool())
            .await?;
    if record_count > 0 {
        return Ok(dashboard_redirect_with_query(
            &bear.slug,
            "import_error",
            "Legacy memory import is disabled for Bears that already have memory records.",
        ));
    }

    let import_dir = import_dir_for_bear(state.config.as_ref(), bear.id);
    std::fs::create_dir_all(&import_dir).map_err(|err| {
        CustomError::System(format!("failed to create legacy import directory: {err}"))
    })?;

    let file_path = import_dir.join(format!("legacy-memory-{}.bundle", Uuid::new_v4()));
    std::fs::write(&file_path, &bundle_bytes)
        .map_err(|err| CustomError::System(format!("failed to stage legacy bundle: {err}")))?;

    let report = match import_staged_bundle(&state, bear.id, &file_path).await {
        Ok(report) => report,
        Err(err) => {
            tracing::warn!(bear_id = %bear.id, error = %err, path = %file_path.display(), "legacy bundle import failed after staging");
            if let Err(delete_err) = std::fs::remove_file(&file_path) {
                tracing::warn!(bear_id = %bear.id, error = %delete_err, path = %file_path.display(), "failed to discard staged legacy bundle after import failure");
            }
            return Ok(dashboard_redirect_with_query(
                &bear.slug,
                "import_error",
                "Bundle upload succeeded, but importing into SQLite failed. The failed upload was discarded; check Den logs for the importer error.",
            ));
        }
    };

    Ok(dashboard_redirect_with_query(
        &bear.slug,
        "import_notice",
        &format!(
            "Imported {} memory paths (skipped {}, quarantined {}) from the uploaded legacy bundle.",
            report.imported_count, report.skipped_count, report.quarantined_count
        ),
    ))
}

async fn browse_delete_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
    Form(form): Form<MemoryDeleteForm>,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }
    let role = form
        .role
        .parse::<BearProfile>()
        .map_err(CustomError::ValidationError)?;
    let action = form.action.as_deref().unwrap_or("delete").trim();
    let confirm = form.confirm.trim();
    let mut paths = form
        .paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let browse_base = format!("/bear/{}/memory/browse", bear.slug);
    if paths.is_empty() {
        return Ok(Redirect::to(&format!(
            "{browse_base}?error={}",
            urlencoding::encode("Select at least one memory path.")
        ))
        .into_response());
    }
    if action == "request_review" {
        let title = form
            .review_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Review selected memory");
        let summary = form
            .review_summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Selected memory paths were marked for Reflection/curate review from the Bear memory UI.");
        let proposal = memory_proposals::create(
            state.sqlx_pool(),
            CreateMemoryProposal {
                bear_id: bear.id,
                source_profile: role,
                source_agent_id: bears_db::profile_binding_id(state.sqlx_pool(), bear.id, role)
                    .await?,
                source_paths: paths,
                source_refs: serde_json::json!([]),
                suggested_action: form
                    .suggested_action
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("unspecified"),
                target_ref: None,
                title,
                summary,
                rationale: form
                    .review_rationale
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or(""),
                proposed_content: None,
                proposed_patch: None,
                refs: serde_json::json!({}),
                sensitivity: form
                    .sensitivity
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("normal"),
                requires_human: form.requires_human.as_deref() == Some("on"),
                project_to_conversation: true,
            },
        )
        .await?;
        return Ok(Redirect::to(&format!(
            "{browse_base}?review_requested=1&path={}",
            urlencoding::encode(
                proposal
                    .source_paths
                    .first()
                    .map(String::as_str)
                    .unwrap_or("")
            )
        ))
        .into_response());
    }
    if confirm != role.as_str() && confirm != bear.slug {
        return Ok(Redirect::to(&format!(
            "{browse_base}?error={}",
            urlencoding::encode("Type the profile name or Bear slug to confirm deletion.")
        ))
        .into_response());
    }
    let manager = state.memory_stores.clone();
    let store = manager.store_for_bear(bear.id).await?;
    let mut deleted = 0usize;
    for path in &paths {
        // Per-Bear canonical memory is SQLite; the workspace SQLx prepare URL is
        // Postgres, so this SQLite statement remains typed at the store boundary.
        let result = sqlx::query(
            "DELETE FROM memory_records WHERE bear_id = ? AND scope_profile = ? AND logical_path = ?",
        )
        .bind(bear.id.to_string())
        .bind(role.as_str())
        .bind(path)
        .execute(store.pool())
        .await
        .map_err(|err| CustomError::System(format!("delete memory records failed: {err}")))?;
        if result.rows_affected() > 0 {
            deleted += 1;
        }
    }
    Ok(Redirect::to(&format!("{browse_base}?deleted={deleted}")).into_response())
}

// ---------------------------------------------------------------------------
// Single memory entry: content + history + referenced entities + recall status
// ---------------------------------------------------------------------------

async fn record_view(
    Path((slug, memory_id)): Path<(String, String)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let config = state.config.as_ref();
    let manager = state.memory_stores.clone();
    let viewer = MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear).await?;
    let record = viewer
        .detail(&manager, bear.id, &memory_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("memory record not found".to_string()))?;

    let store = manager.store_for_bear(bear.id).await?;

    // For members, each version must share the authorized record's canonical scope;
    // a colliding logical path cannot turn a private record into history.
    let versions = viewer.history(&store, &record, 50).await?;
    let history: Vec<HistoryItem> = versions
        .iter()
        .enumerate()
        .map(|(idx, v)| HistoryItem {
            is_current: idx == 0,
            memory_id: v.memory_id.clone(),
            sequence_no: v.sequence_no,
            kind: v.kind.clone(),
            created_at: v.created_at.clone(),
        })
        .collect();
    let is_head = history
        .first()
        .map(|h| h.memory_id == memory_id)
        .unwrap_or(true);

    if !can_manage_bear {
        return web::render_template(
            &state,
            "bear/memory/member_record.html",
            auth_session,
            context! {
                record,
                history,
                is_head,
                can_manage_bear,
                native_runtime => true,
                ..bear_nav_context(&bear, "memory"),
            },
        )
        .await;
    }

    // Referenced entities (descriptive + access-bearing) via the relation view.
    let mut entities: Vec<LinkedEntity> = Vec::new();
    for rel in list_relations_for_source(&store, &memory_id, 50)
        .await
        .unwrap_or_default()
    {
        let (display_name, entity_type) = match store::get_entity(&store, &rel.entity_id).await {
            Ok(Some(e)) => (
                e.display_name.unwrap_or_else(|| e.entity_id.clone()),
                e.entity_type,
            ),
            _ => (rel.entity_id.clone(), "unknown".to_string()),
        };
        entities.push(LinkedEntity {
            entity_id: rel.entity_id,
            display_name,
            entity_type,
            relation_label: relation_label(&rel.relation),
            class_label: class_label(&rel.class).to_string(),
        });
    }

    // Recall coverage for this record.
    let recall = if config.qdrant_url.is_some() {
        let passages = recall_registry::list_passages(
            state.sqlx_pool(),
            bear.id,
            &memory_id,
            &config.embedding_standard,
        )
        .await
        .map(|p| p.len())
        .unwrap_or(0);
        Some(json!({ "enabled": true, "passages": passages }))
    } else {
        None
    };

    web::render_template(
        &state,
        "bear/memory/record.html",
        auth_session,
        context! {
            record,
            is_head,
            history,
            entities,
            recall,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

async fn entities_view(
    Path(slug): Path<String>,
    Query(query): Query<EntitiesQuery>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear)
        .await?
        .require_admin_review()?;
    let manager = state.memory_stores.clone();
    let store = manager.store_for_bear(bear.id).await?;
    let type_filter = query
        .r#type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let rows = store::list_entities(&store, type_filter, 200)
        .await
        .unwrap_or_default();

    // Distinct types present, for the filter chips.
    let mut types: Vec<String> = store::list_entities(&store, None, 500)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.entity_type)
        .collect();
    types.sort();
    types.dedup();

    let entities: Vec<EntityListItem> = rows
        .into_iter()
        .map(|e| EntityListItem {
            display_name: e.display_name.unwrap_or_else(|| e.entity_id.clone()),
            entity_id: e.entity_id,
            entity_type: e.entity_type,
            resolution: e.resolution.as_str().to_string(),
            trust: e.trust.as_str().to_string(),
            created_at: e.created_at,
        })
        .collect();

    web::render_template(
        &state,
        "bear/memory/entities.html",
        auth_session,
        context! {
            entities,
            types,
            type_filter => type_filter.unwrap_or(""),
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

async fn entity_detail_view(
    Path((slug, entity_id)): Path<(String, String)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear)
        .await?
        .require_admin_review()?;
    let manager = state.memory_stores.clone();
    let store = manager.store_for_bear(bear.id).await?;
    let row = store::get_entity(&store, &entity_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("entity not found".to_string()))?;
    let entity = EntityDetail {
        display_name: row
            .display_name
            .clone()
            .unwrap_or_else(|| row.entity_id.clone()),
        entity_id: row.entity_id,
        sequence_no: row.sequence_no,
        entity_type: row.entity_type,
        resolution: row.resolution.as_str().to_string(),
        trust: row.trust.as_str().to_string(),
        canonical_ref: row.canonical_ref,
        superseded_by_entity_id: row.superseded_by_entity_id,
        metadata_json: row.metadata_json,
        created_at: row.created_at,
    };
    let handles: Vec<HandleItem> = store::list_handles(&store, &entity_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|h| HandleItem {
            handle_type: h.handle_type,
            handle_value: h.handle_value,
            source: h.source,
            trust: h.trust.as_str().to_string(),
            state: h.state,
        })
        .collect();

    let related: Vec<LinkedRecord> = list_relations_for_entity(&store, &entity_id, 100)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|rel| LinkedRecord {
            src_memory_id: rel.src_memory_id,
            relation_label: relation_label(&rel.relation),
            class_label: class_label(&rel.class).to_string(),
            author_profile: rel.author_profile,
            created_at: rel.created_at,
        })
        .collect();

    web::render_template(
        &state,
        "bear/memory/entity.html",
        auth_session,
        context! {
            entity,
            handles,
            related,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Reflection runs (performance + review trace)
// ---------------------------------------------------------------------------

async fn reflection_run_get(
    Path((slug, run_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear)
        .await?
        .require_admin_review()?;
    let manager = state.memory_stores.clone();
    let detail = get_reflection_run_detail(state.sqlx_pool(), &manager, bear.id, run_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("reflection run not found".to_string()))?;

    web::render_template(
        &state,
        "bear/memory/reflection_run.html",
        auth_session,
        context! {
            bear,
            detail,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

async fn reflection_evidence_get(
    Path((slug, run_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear)
        .await?
        .require_admin_review()?;
    let manager = state.memory_stores.clone();
    let evidence =
        get_reflection_evidence(state.sqlx_pool(), &manager, bear.id, &bear.slug, run_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("reflection run not found".to_string()))?;

    web::render_template(
        &state,
        "bear/memory/reflection_evidence.html",
        auth_session,
        context! {
            bear,
            evidence,
            can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Memory proposals (review/governance)
// ---------------------------------------------------------------------------

async fn proposal_get(
    Path((slug, proposal_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    MemoryLibraryViewer::resolve(&state, bear.id, can_manage_bear)
        .await?
        .require_admin_review()?;
    let proposal = if let Some(proposal) =
        memory_proposals::get_for_bear(state.sqlx_pool(), bear.id, proposal_id).await?
    {
        proposal_view_from_postgres(proposal)
    } else {
        let manager = state.memory_stores.clone();
        let store = manager.store_for_bear(bear.id).await?;
        get_sqlite_memory_proposal(&store, &proposal_id.to_string())
            .await?
            .map(proposal_view_from_sqlite)
            .ok_or_else(|| CustomError::NotFound("memory proposal not found".to_string()))?
    };
    web::render_template(
        &state,
        "bear/memory_proposal.html",
        auth_session,
        context! {
            bear,
            proposal,
            can_manage_bear,
            errors => None::<String>,
            ..bear_nav_context(&bear, "memory"),
        },
    )
    .await
}

async fn proposal_post(
    Path((slug, proposal_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: crate::auth_backend::AuthSession,
    Form(form): Form<MemoryProposalResolutionForm>,
) -> Result<Response, CustomError> {
    let user = session_user(&auth_session).await?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }
    let status = form.status.trim();
    if !matches!(
        status,
        "rejected" | "retained_local" | "deferred" | "superseded" | "needs_human_review"
    ) {
        return Err(CustomError::ValidationError(
            "invalid memory proposal status".to_string(),
        ));
    }
    if memory_proposals::get_for_bear(state.sqlx_pool(), bear.id, proposal_id)
        .await?
        .is_some()
    {
        memory_proposals::resolve_for_bear(
            state.sqlx_pool(),
            memory_proposals::ProposalResolutionParams {
                bear_id: bear.id,
                proposal_id,
                reviewer_profile: BearProfile::Curate,
                reviewer_agent_id: None,
                status,
                review_notes: form.review_notes.as_deref(),
                decision_summary: form.decision_summary.as_deref(),
                result_path: None,
                result_commit: None,
                project_to_conversation: true,
            },
        )
        .await?;
    } else {
        let manager = state.memory_stores.clone();
        let store = manager.store_for_bear(bear.id).await?;
        let review_payload = json!({
            "reviewer_profile": BearProfile::Curate.as_str(),
            "reviewer_agent_id": Value::Null,
            "review_notes": form.review_notes,
            "decision_summary": form.decision_summary,
            "result_path": Value::Null,
            "result_commit": Value::Null,
        });
        resolve_sqlite_memory_proposal(&store, &proposal_id.to_string(), status, &review_payload)
            .await?;
    }
    if form.after_save.as_deref() == Some("next") {
        let manager = state.memory_stores.clone();
        if let Some(next) = next_review_proposal(&state, &manager, bear.id, proposal_id).await {
            return Ok(
                Redirect::to(&format!("/bear/{}/memory/proposals/{}", bear.slug, next.id))
                    .into_response(),
            );
        }
    }
    Ok(Redirect::to(&format!("/bear/{}/memory", bear.slug)).into_response())
}

async fn next_review_proposal(
    state: &AppState,
    manager: &MemoryStoreManager,
    bear_id: Uuid,
    current_id: Uuid,
) -> Option<MemoryProposalView> {
    let current_id = current_id.to_string();
    // ponytail: scans the first 200 reviewable proposals; upgrade to a cursor query if queues grow.
    let mut proposals =
        list_dashboard_proposals(state, manager, bear_id, Some("pending"), 200).await;
    proposals.extend(
        list_dashboard_proposals(state, manager, bear_id, Some("needs_human_review"), 200).await,
    );
    proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    proposals
        .into_iter()
        .find(|proposal| proposal.id != current_id)
}

#[cfg(test)]
mod tests;
