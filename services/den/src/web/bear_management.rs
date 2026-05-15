//! Member-facing bear lifecycle: create bears (you become admin), details, membership, edit/delete for bear admins (or site operators).
//! When changing routes, update `src/web/ROUTES.md`.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use minijinja::context;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::{AuthSession, SessionUser},
    config::Config,
    core::{
        acp_sessions, acp_tokens,
        acp_tools::{acp_tool_policy_json_for_provider, AcpToolName},
        archived_conversations,
        bears::{
            compute_letta_drift_with_expected_tool_ids, db as bears_db,
            db::{role_is_bear_admin, BearMemberRow, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
            provision, sync, Bear, BearAgent, BearAgentRole,
        },
        letta::{load_agent_conversations, AgentSummary, LettaAgentDiagnostics},
        memory_manager_head::{
            delete_memfs_role_memory_entries, fetch_memfs_role_memory_file,
            fetch_memfs_role_memory_status, fetch_memfs_role_memory_tree,
            fetch_memfs_role_view_health, fetch_memory_manager_repository_files,
            fetch_memory_manager_repository_status, search_memfs_role_memory,
        },
        memory_proposals::{self, CreateMemoryProposal},
        pair_reflection, user,
        user::db as user_db,
    },
    errors::CustomError,
    web::{
        bear_create_support::{
            bear_configuration_page_context, bear_new_form_context,
            ensure_stored_model_in_options_for_handle, insert_new_bear_row,
            validate_default_model_for_letta, BearConfigurationEditForm, BearOverviewEditForm,
            BearPromptEditForm, NewBearForm,
        },
        render_template, AppState,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bears/new", get(new_bear_get).post(new_bear_post))
        .route_with_tsr("/bear/{slug}/details", get(bear_details_get))
        .route_with_tsr(
            "/bear/{slug}/details/resync-letta",
            post(bear_resync_letta_post),
        )
        .route_with_tsr("/bear/{slug}/details/edit", get(bear_edit_redirect_get))
        .route_with_tsr(
            "/bear/{slug}/details/edit/overview",
            get(bear_edit_overview_get).post(bear_edit_overview_post),
        )
        .route_with_tsr(
            "/bear/{slug}/details/edit/prompt",
            get(bear_edit_prompt_get).post(bear_edit_prompt_post),
        )
        .route_with_tsr(
            "/bear/{slug}/details/edit/configuration",
            get(bear_edit_configuration_get).post(bear_edit_configuration_post),
        )
        .route_with_tsr("/bear/{slug}/details/access", get(bear_access_get))
        .route_with_tsr(
            "/bear/{slug}/details/code-token",
            get(bear_code_token_get).post(bear_code_token_post),
        )
        .route_with_tsr(
            "/bear/{slug}/details/conversations",
            get(bear_conversations_get),
        )
        .route_with_tsr("/bear/{slug}/details/roles/{role}", get(bear_role_get))
        .route_with_tsr(
            "/bear/{slug}/details/memory",
            get(bear_memory_get).post(bear_memory_delete_post),
        )
        .route_with_tsr(
            "/bear/{slug}/details/memory/runtime-blocks",
            get(bear_runtime_blocks_get),
        )
        .route_with_tsr(
            "/bear/{slug}/details/memory/proposals/{proposal_id}",
            get(bear_memory_proposal_get).post(bear_memory_proposal_post),
        )
        .route_with_tsr("/bear/{slug}/details/delete", post(bear_delete_post))
        .route_with_tsr("/bear/{slug}/details/members/add", post(member_add_post))
        .route_with_tsr(
            "/bear/{slug}/details/members/remove",
            post(member_remove_post),
        )
}

async fn email_verify_redirect(
    pool: &sqlx::PgPool,
    user_id: i32,
) -> Result<Option<Redirect>, CustomError> {
    let u = user::user_by_id(pool, user_id).await?;
    if !u.email_verified.unwrap_or(false) {
        return Ok(Some(Redirect::to("/settings/email/verify")));
    }
    Ok(None)
}

async fn load_bear_member(
    pool: &sqlx::PgPool,
    user_id: i32,
    slug: &str,
) -> Result<Bear, CustomError> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    bears_db::bear_for_user_by_slug(pool, user_id, slug)
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("Bear not found or you do not have access.".to_string())
        })
}

async fn viewer_is_bear_admin(
    pool: &sqlx::PgPool,
    user_id: i32,
    bear_id: Uuid,
) -> Result<bool, CustomError> {
    let role = bears_db::membership_role_for_user(pool, user_id, bear_id).await?;
    Ok(match role {
        None => false,
        Some(inner) => role_is_bear_admin(inner.as_deref()),
    })
}

/// Edit bear settings, resync, access, membership, delete: bear admins or site operators (`users.is_admin`).
async fn viewer_can_manage_bear(
    pool: &sqlx::PgPool,
    user: &SessionUser,
    bear_id: Uuid,
) -> Result<bool, CustomError> {
    if user.is_admin {
        return Ok(true);
    }
    viewer_is_bear_admin(pool, user.id, bear_id).await
}

#[derive(Debug, Deserialize)]
struct BearDetailsQuery {
    letta_resync: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BearMemoryQuery {
    role: Option<String>,
    q: Option<String>,
    path: Option<String>,
    deleted: Option<usize>,
    review_requested: Option<usize>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BearMemoryProposalResolutionForm {
    status: String,
    #[serde(default)]
    review_notes: Option<String>,
    #[serde(default)]
    decision_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BearMemoryDeleteForm {
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

#[derive(Serialize)]
struct BearMemoryRoleRow {
    role: String,
    label: String,
    description: String,
    runtime_family: String,
    letta_agent_id: Option<String>,
    provisioning_status: String,
    selected: bool,
    status_state: Option<String>,
    status_label: String,
    file_count: usize,
    registered_view_count: usize,
    canonical_tip: Option<String>,
    allowed_prefixes: Vec<String>,
    entry_counts: Vec<BearMemoryEntryCountRow>,
    recent_activity: Vec<BearMemoryActivityRow>,
    error: Option<String>,
}

#[derive(Serialize)]
struct BearMemoryActivityRow {
    event: String,
    detail: String,
}

#[derive(Serialize)]
struct BearMemoryEntryCountRow {
    kind: String,
    count: String,
}

#[derive(Serialize)]
struct RoleDetailView {
    role: String,
    label: String,
    plain_name: &'static str,
    description: String,
    surfaces: Vec<&'static str>,
    capabilities: Vec<&'static str>,
    memory: Vec<&'static str>,
    actions: Vec<RoleActionLink>,
    runtime_family: String,
    letta_agent_id: Option<String>,
    provisioning_status: String,
    last_synced_at: Option<String>,
    last_provisioning_error: Option<String>,
    memfs_view_state: Option<String>,
    memfs_view_quarantined: bool,
    memfs_view_diagnostic: Option<String>,
    memory_status_label: String,
    memory_file_count: usize,
    memory_entry_counts: Vec<BearMemoryEntryCountRow>,
    memory_allowed_prefixes: Vec<String>,
    memory_recent_activity: Vec<BearMemoryActivityRow>,
    role_contract: String,
    composed_prompt: String,
}

#[derive(Serialize)]
struct RoleActionLink {
    label: &'static str,
    href: String,
}

#[derive(Serialize)]
struct RuntimeBlockRoleRow {
    role: String,
    label: String,
    letta_agent_id: Option<String>,
    block_count: usize,
    diagnostics: Option<LettaAgentDiagnostics>,
    error: Option<String>,
}

#[derive(Serialize)]
struct AcpToolDetailRow {
    name: &'static str,
    title: &'static str,
    kind: &'static str,
    risk: &'static str,
    approval_label: &'static str,
    scope_label: &'static str,
    policy_summary: Vec<String>,
    parameter_summary: Vec<&'static str>,
    usage_hint: &'static str,
    highlighted: bool,
}

fn role_memory_label(role: BearAgentRole) -> &'static str {
    match role {
        BearAgentRole::Talk => "Conversation memory",
        BearAgentRole::Pair => "Pairing memory",
        BearAgentRole::Curate => "Review memory",
        BearAgentRole::Work => "Work memory",
        BearAgentRole::Watch => "Watch memory",
    }
}

fn role_memory_description(role: BearAgentRole) -> &'static str {
    match role {
        BearAgentRole::Talk => "Notes and local memory from chat-like conversations.",
        BearAgentRole::Pair => "Coding collaboration notes, logs, decisions, and summaries.",
        BearAgentRole::Curate => "Review, reflection, and memory integration work.",
        BearAgentRole::Work => "Task execution logs, decisions, and summaries.",
        BearAgentRole::Watch => "Event/subscription logs and summaries.",
    }
}

fn role_plain_name(role: BearAgentRole) -> &'static str {
    match role {
        BearAgentRole::Talk => "Conversational front door",
        BearAgentRole::Pair => "Collaborative tool/IDE partner",
        BearAgentRole::Curate => "Memory and integration reviewer",
        BearAgentRole::Work => "Approved outbound executor",
        BearAgentRole::Watch => "Inbound observer",
    }
}

fn role_surfaces(role: BearAgentRole) -> Vec<&'static str> {
    match role {
        BearAgentRole::Talk => vec!["Web chat", "Future chat surfaces"],
        BearAgentRole::Pair => vec!["ACP clients", "IDEs", "Future design/productivity tools"],
        BearAgentRole::Curate => vec!["Internal review and integration"],
        BearAgentRole::Work => vec!["Den task dispatch", "Schedules", "Approved background jobs"],
        BearAgentRole::Watch => vec![
            "Webhooks",
            "Polling",
            "Queues",
            "Subscriptions",
            "Event streams",
        ],
    }
}

fn role_capabilities(role: BearAgentRole) -> Vec<&'static str> {
    match role {
        BearAgentRole::Talk => vec![
            "Synchronous conversation",
            "Task intent capture",
            "Channel-safe tools",
            "Work plan updates",
        ],
        BearAgentRole::Pair => vec![
            "Client-mediated tool use",
            "Code/workspace context",
            "User-gated actions",
            "File/document collaboration",
        ],
        BearAgentRole::Curate => vec![
            "Memory review",
            "Task intent review",
            "Observation review",
            "Skill proposal review",
            "Shared memory promotion",
        ],
        BearAgentRole::Work => vec![
            "Approved API calls",
            "Scheduled tasks",
            "Event-triggered work",
            "Run-status reporting",
        ],
        BearAgentRole::Watch => vec![
            "Inbound event parsing",
            "Observation creation",
            "Subscription monitoring",
            "Event summarization",
        ],
    }
}

fn acp_tool_detail_rows() -> Vec<AcpToolDetailRow> {
    AcpToolName::all()
        .iter()
        .filter(|tool| {
            matches!(
                tool,
                AcpToolName::TerminalRunCommand
                    | AcpToolName::ProcessRun
                    | AcpToolName::ReadTextFile
                    | AcpToolName::ListDirectory
                    | AcpToolName::SearchFiles
                    | AcpToolName::EditFile
                    | AcpToolName::CreateTextFile
                    | AcpToolName::DeletePath
            )
        })
        .map(|tool| {
            let descriptor = tool.descriptor();
            let policy = acp_tool_policy_json_for_provider(descriptor.provider_name);
            let mut policy_summary = Vec::new();
            if policy
                .get("approval_required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                policy_summary.push("User approval required before execution".to_string());
            }
            if let Some(timeout) = policy.get("total_timeout_ms").and_then(|v| v.as_u64()) {
                policy_summary.push(format!("Timeout: {}s", timeout / 1000));
            }
            if let Some(max_bytes) = policy.get("max_bytes").and_then(|v| v.as_u64()) {
                policy_summary.push(format!("Output/input cap: {} KiB", max_bytes / 1024));
            }
            if let Some(path_policy) = policy.get("path_containment").and_then(|v| v.as_str()) {
                policy_summary.push(path_policy.replace('_', " "));
            }
            AcpToolDetailRow {
                name: descriptor.provider_name,
                title: descriptor.title,
                kind: descriptor.kind,
                risk: descriptor.risk,
                approval_label: acp_tool_approval_label(descriptor.provider_name),
                scope_label: acp_tool_scope_label(descriptor.provider_name),
                policy_summary,
                parameter_summary: acp_tool_parameter_summary(descriptor.provider_name),
                usage_hint: acp_tool_usage_hint(descriptor.provider_name),
                highlighted: descriptor.provider_name == "terminal_run_command",
            }
        })
        .collect()
}

fn acp_tool_approval_label(provider_name: &str) -> &'static str {
    match provider_name {
        "terminal_run_command" => "Only once, by command in workspace, by workspace, or globally",
        "process_run" => "Only once, by command in workspace, by workspace, or globally",
        "fs_read_text_file" | "fs_list_directory" | "fs_search_files" => {
            "Only once, by directory, by workspace, or globally"
        }
        "fs_edit_file" | "fs_create_text_file" => {
            "Only once, by directory, by workspace, or globally"
        }
        "fs_delete_path" => "Only once, by directory, by workspace, or globally",
        _ => "Tool-family approval through ACP client",
    }
}

fn acp_tool_scope_label(provider_name: &str) -> &'static str {
    match provider_name {
        "terminal_run_command" => "Workspace cwd + allowlisted build/test commands",
        "process_run" => "Workspace cwd + adapter process policy",
        "fs_read_text_file" | "fs_list_directory" | "fs_search_files" => {
            "Absolute paths under ACP workspace roots"
        }
        "fs_edit_file" | "fs_create_text_file" | "fs_delete_path" => {
            "Absolute paths under ACP workspace roots; hidden/sensitive paths restricted"
        }
        _ => "ACP session workspace/policy scope",
    }
}

fn acp_tool_parameter_summary(provider_name: &str) -> Vec<&'static str> {
    match provider_name {
        "terminal_run_command" | "process_run" => vec![
            "command",
            "args[]",
            "cwd",
            "timeout_ms",
            "max_output_bytes",
            "env",
        ],
        "fs_read_text_file" => vec!["path", "line", "limit"],
        "fs_list_directory" => vec!["path", "recursive", "limit", "include_hidden"],
        "fs_search_files" => vec!["path", "query", "pattern", "extensions", "case_sensitive"],
        "fs_edit_file" => vec!["path", "old_text", "new_text", "expected_replacements"],
        "fs_create_text_file" => vec!["path", "content", "create_parent_dirs"],
        "fs_delete_path" => vec!["path", "recursive", "expected_kind"],
        _ => Vec::new(),
    }
}

fn acp_tool_usage_hint(provider_name: &str) -> &'static str {
    match provider_name {
        "terminal_run_command" => "Use for build/test commands that should run in Zed's client terminal and wait for actual process exit, including Cargo file-lock waits.",
        "process_run" => "Legacy bounded adapter-local process execution; prefer terminal_run_command for visible build/test workflows.",
        "fs_read_text_file" => "Read bounded text from a workspace file.",
        "fs_list_directory" => "Discover workspace files and directories without reading file contents.",
        "fs_search_files" => "Search text or path patterns under a workspace directory.",
        "fs_edit_file" => "Safely edit an existing file by replacing exact text with preview and stale-file revalidation.",
        "fs_create_text_file" => "Create a new UTF-8 text file without overwriting existing content.",
        "fs_delete_path" => "Delete files or empty directories; recursive deletion requires explicit opt-in.",
        _ => "ACP local tool.",
    }
}

fn role_memory_rules(role: BearAgentRole) -> Vec<&'static str> {
    match role {
        BearAgentRole::Talk => vec![
            "Reads core/",
            "Reads and writes talk/",
            "Does not directly promote to core/",
        ],
        BearAgentRole::Pair => vec![
            "Reads core/",
            "Reads and writes pair/",
            "Does not directly promote to core/",
        ],
        BearAgentRole::Curate => vec![
            "Reads across role branches, subject to policy",
            "Writes curate/",
            "Promotes durable knowledge into core/",
            "Does not write directly to other role branches",
        ],
        BearAgentRole::Work => vec![
            "Reads core/",
            "Reads task definition/run context",
            "Reads and writes work/",
            "Does not read raw talk/, pair/, or watch/ directly",
        ],
        BearAgentRole::Watch => vec![
            "Reads core/",
            "Reads delivered event payloads",
            "Reads and writes watch/",
            "Does not write core/ directly",
            "Does not trigger outbound action directly",
        ],
    }
}

fn value_object_count_rows(value: &serde_json::Value) -> Vec<BearMemoryEntryCountRow> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let mut rows = map
        .iter()
        .map(|(kind, count)| BearMemoryEntryCountRow {
            kind: kind.clone(),
            count: count
                .as_u64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| count.to_string()),
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.kind.cmp(&b.kind));
    rows
}

fn memory_activity_rows(value: &serde_json::Value) -> Vec<BearMemoryActivityRow> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .rev()
        .take(10)
        .filter_map(|item| {
            let obj = item.as_object()?;
            let event = obj.get("event").and_then(|v| v.as_str()).unwrap_or("event");
            let detail = obj
                .get("path")
                .or_else(|| obj.get("status"))
                .or_else(|| obj.get("state"))
                .or_else(|| obj.get("reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(BearMemoryActivityRow {
                event: event.to_string(),
                detail: detail.to_string(),
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct CodeTokenForm {
    name: String,
}

#[derive(Serialize)]
struct DetailsConvRow {
    id: String,
    title: String,
    last_message_at: Option<String>,
    channel_label: &'static str,
    web_href: String,
    archived: bool,
}

#[derive(Serialize)]
struct BearRoleViewRow {
    role: String,
    runtime_family: String,
    letta_agent_id: Option<String>,
    provisioning_status: String,
    last_synced_at: Option<String>,
    memfs_view_state: Option<String>,
    memfs_view_quarantined: bool,
    memfs_view_diagnostic: Option<String>,
}

#[derive(Debug, Serialize)]
struct BearWebSourceRow {
    id: Uuid,
    scope_kind: String,
    scope_value: String,
    label: Option<String>,
    policy: String,
    priority: i32,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct BearWebApprovalRow {
    id: Uuid,
    scope_kind: String,
    scope_value: String,
    source: String,
    approved_by_user_label: Option<String>,
    created_at: String,
    expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct BearWebFetchRow {
    url: String,
    final_url: Option<String>,
    host: String,
    execution_location: String,
    approval_kind: String,
    http_status: Option<i32>,
    content_type: Option<String>,
    bytes: Option<i64>,
    fetched_at: String,
}

#[derive(Debug, Serialize)]
struct BearPlanModeRow {
    id: Uuid,
    user_id: i32,
    username: Option<String>,
    acp_session_id: String,
    state: String,
    reason: String,
    plan_artifact_path: Option<String>,
    plan_title: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct BearWorkSurfaceRow {
    slug: String,
    display_name: String,
    summary: Option<String>,
    glossary_present: bool,
    pair_current_understanding_present: bool,
    work_current_understanding_present: bool,
    role_local_presence_count: usize,
    active_workplace_count: usize,
    known_in_workplace_count: usize,
    workplace_labels: Vec<String>,
    canonical_path_count: usize,
    anchor_status: String,
}

impl BearRoleViewRow {
    fn from_agent(agent: BearAgent, role: BearAgentRole) -> Self {
        Self {
            role: role.as_str().to_string(),
            runtime_family: role.runtime_family().to_string(),
            letta_agent_id: agent.letta_agent_id,
            provisioning_status: agent.provisioning_status,
            last_synced_at: agent.last_synced_at.map(|t| t.to_string()),
            memfs_view_state: None,
            memfs_view_quarantined: false,
            memfs_view_diagnostic: None,
        }
    }
}

fn memfs_http_client(context: &str) -> Result<reqwest::Client, CustomError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CustomError::System(format!("{context}: {e}")))
}

fn parse_work_surface_display_name(index_content: &str, slug: &str) -> String {
    for line in index_content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }
    slug.to_string()
}

fn first_nonempty_markdown_paragraph(content: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("- ") {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        lines.push(trimmed);
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

async fn bear_work_surface_rows(
    config: &Config,
    bear_id: Uuid,
) -> Result<Vec<BearWorkSurfaceRow>, CustomError> {
    let http = memfs_http_client("MemFS work-surface detail client build failed")?;
    let mut rows = Vec::new();
    let core_tree = fetch_memfs_role_memory_tree(
        &http,
        &config.letta_memfs_service_url,
        bear_id,
        BearAgentRole::Pair.as_str(),
    )
    .await?;
    let Some(core_tree) = core_tree else {
        return Ok(rows);
    };
    let root = core_tree.files.as_array().cloned().unwrap_or_default();
    let work_surfaces_dir = root.iter().find(|node| {
        node.get("path") == Some(&serde_json::Value::String("core/work_surfaces".to_string()))
    });
    let Some(work_surfaces_dir) = work_surfaces_dir else {
        return Ok(rows);
    };
    let children = work_surfaces_dir
        .get("children")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    for child in children {
        let slug = match child.get("name").and_then(|v| v.as_str()) {
            Some("index.md") | None => continue,
            Some(value) if !value.trim().is_empty() => value.trim().to_string(),
            _ => continue,
        };
        let slug_path_prefix = format!("core/work_surfaces/{slug}/");
        let child_nodes = child
            .get("children")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let child_paths = child_nodes
            .iter()
            .filter_map(|node| node.get("path").and_then(|v| v.as_str()).map(ToString::to_string))
            .collect::<Vec<_>>();
        let canonical_path_count = child_paths.len();
        let index_path = format!("{slug_path_prefix}index.md");
        let overview_path = format!("{slug_path_prefix}overview.md");
        let glossary_path = format!("{slug_path_prefix}glossary.md");
        let index_file = fetch_memfs_role_memory_file(
            &http,
            &config.letta_memfs_service_url,
            bear_id,
            BearAgentRole::Pair.as_str(),
            &index_path,
        )
        .await?;
        let overview_file = fetch_memfs_role_memory_file(
            &http,
            &config.letta_memfs_service_url,
            bear_id,
            BearAgentRole::Pair.as_str(),
            &overview_path,
        )
        .await?;
        let display_name = index_file
            .as_ref()
            .map(|file| parse_work_surface_display_name(&file.content, &slug))
            .unwrap_or_else(|| slug.clone());
        let summary = overview_file
            .as_ref()
            .and_then(|file| first_nonempty_markdown_paragraph(&file.content));
        let glossary_present = child_paths.iter().any(|path| path == &glossary_path);
        let pair_current_understanding_present = fetch_memfs_role_memory_file(
            &http,
            &config.letta_memfs_service_url,
            bear_id,
            BearAgentRole::Pair.as_str(),
            &format!("pair/work_surfaces/{slug}/current-understanding.md"),
        )
        .await?
        .is_some();
        let work_current_understanding_present = fetch_memfs_role_memory_file(
            &http,
            &config.letta_memfs_service_url,
            bear_id,
            BearAgentRole::Work.as_str(),
            &format!("work/work_surfaces/{slug}/current-understanding.md"),
        )
        .await?
        .is_some();
        let workplace_labels = [
            (BearAgentRole::Pair, pair_current_understanding_present),
            (BearAgentRole::Work, work_current_understanding_present),
        ]
        .into_iter()
        .filter_map(|(role, present)| present.then(|| role.as_str().to_string()))
        .collect::<Vec<_>>();
        let role_local_presence_count = workplace_labels.len();
        let active_workplace_count = workplace_labels.len();
        let known_in_workplace_count = canonical_path_count + role_local_presence_count;
        let anchor_status = if canonical_path_count == 0 {
            "missing_canonical".to_string()
        } else if role_local_presence_count == 0 {
            "canonical_only".to_string()
        } else {
            "active".to_string()
        };
        rows.push(BearWorkSurfaceRow {
            slug,
            display_name,
            summary,
            glossary_present,
            pair_current_understanding_present,
            work_current_understanding_present,
            role_local_presence_count,
            active_workplace_count,
            known_in_workplace_count,
            workplace_labels,
            canonical_path_count,
            anchor_status,
        });
    }
    rows.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(rows)
}

async fn bear_web_sources(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebSourceRow>, CustomError> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            String,
            i32,
            time::OffsetDateTime,
        ),
    >(
        r#"
        SELECT id, scope_kind, scope_value, label, policy, priority, created_at
        FROM bear_web_sources
        WHERE bear_id = $1
        ORDER BY policy ASC, priority DESC, scope_kind ASC, scope_value ASC
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, scope_kind, scope_value, label, policy, priority, created_at)| BearWebSourceRow {
                id,
                scope_kind,
                scope_value,
                label,
                policy,
                priority,
                created_at: created_at.to_string(),
            },
        )
        .collect())
}

async fn bear_web_approvals(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebApprovalRow>, CustomError> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            time::OffsetDateTime,
            Option<time::OffsetDateTime>,
        ),
    >(
        r#"
        SELECT a.id,
               a.scope_kind,
               a.scope_value,
               a.source,
               u.username,
               NULLIF(u.display_name, '') AS display_name,
               a.created_at,
               a.expires_at
        FROM bear_web_approvals a
        LEFT JOIN users u ON u.id = a.approved_by_user_id
        WHERE a.bear_id = $1 AND a.revoked_at IS NULL
        ORDER BY a.created_at DESC
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                scope_kind,
                scope_value,
                source,
                username,
                display_name,
                created_at,
                expires_at,
            )| BearWebApprovalRow {
                id,
                scope_kind,
                scope_value,
                source,
                approved_by_user_label: match (display_name, username) {
                    (Some(display_name), Some(username)) => {
                        Some(format!("{display_name} (@{username})"))
                    }
                    (Some(display_name), None) => Some(display_name),
                    (None, Some(username)) => Some(format!("@{username}")),
                    (None, None) => None,
                },
                created_at: created_at.to_string(),
                expires_at: expires_at.map(|t| t.to_string()),
            },
        )
        .collect())
}

async fn bear_web_fetches(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebFetchRow>, CustomError> {
    let rows = sqlx::query_as::<_, (String, Option<String>, String, String, String, Option<i32>, Option<String>, Option<i64>, time::OffsetDateTime)>(
        r#"
        SELECT url, final_url, host, execution_location, approval_kind, http_status, content_type, bytes, fetched_at
        FROM bear_web_fetches
        WHERE bear_id = $1
        ORDER BY fetched_at DESC
        LIMIT 25
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                url,
                final_url,
                host,
                execution_location,
                approval_kind,
                http_status,
                content_type,
                bytes,
                fetched_at,
            )| BearWebFetchRow {
                url,
                final_url,
                host,
                execution_location,
                approval_kind,
                http_status,
                content_type,
                bytes,
                fetched_at: fetched_at.to_string(),
            },
        )
        .collect())
}

async fn bear_plan_mode_rows(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearPlanModeRow>, CustomError> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            i32,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            time::OffsetDateTime,
            time::OffsetDateTime,
        ),
    >(
        r#"
        SELECT s.id,
               s.user_id,
               u.username,
               s.acp_session_id,
               s.state,
               s.reason,
               s.plan_artifact_path,
               s.plan_title,
               s.created_at,
               s.updated_at
        FROM acp_plan_mode_sessions s
        LEFT JOIN users u ON u.id = s.user_id
        WHERE s.bear_id = $1
        ORDER BY s.updated_at DESC
        LIMIT 10
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                user_id,
                username,
                acp_session_id,
                state,
                reason,
                plan_artifact_path,
                plan_title,
                created_at,
                updated_at,
            )| BearPlanModeRow {
                id,
                user_id,
                username,
                acp_session_id,
                state,
                reason,
                plan_artifact_path,
                plan_title,
                created_at: created_at.to_string(),
                updated_at: updated_at.to_string(),
            },
        )
        .collect())
}

async fn build_role_detail_view(
    state: &AppState,
    bear: &Bear,
    role: BearAgentRole,
) -> Result<RoleDetailView, CustomError> {
    bears_db::ensure_bear_agent_rows(state.sqlx_pool(), bear.id).await?;
    let agent = bears_db::get_bear_agent(state.sqlx_pool(), bear.id, role)
        .await?
        .ok_or_else(|| CustomError::NotFound("role agent not found".to_string()))?;

    let memfs_url = state.config.letta_memfs_service_url.trim().to_string();
    let mut role_row = BearRoleViewRow::from_agent(agent.clone(), role);
    if !memfs_url.is_empty() {
        match fetch_memfs_role_view_health(state.letta.http(), &memfs_url, bear.id, role.as_str())
            .await
        {
            Ok(Some(view)) => {
                role_row.memfs_view_state = Some(view.state);
                role_row.memfs_view_quarantined = view.quarantined;
                role_row.memfs_view_diagnostic = view.diagnostic;
            }
            Ok(None) => {}
            Err(err) => {
                role_row.memfs_view_state = Some("error".to_string());
                role_row.memfs_view_diagnostic = Some(err.to_string());
            }
        }
    }

    let mut memory_status_label = if memfs_url.is_empty() {
        "MemFS not configured".to_string()
    } else {
        "Unavailable".to_string()
    };
    let mut memory_file_count = 0usize;
    let mut memory_entry_counts = Vec::new();
    let mut memory_allowed_prefixes = Vec::new();
    let mut memory_recent_activity = Vec::new();
    if !memfs_url.is_empty() {
        match fetch_memfs_role_memory_status(state.letta.http(), &memfs_url, bear.id, role.as_str())
            .await
        {
            Ok(Some(status)) => {
                memory_status_label = if status.ok {
                    "Available"
                } else {
                    "Unavailable"
                }
                .to_string();
                memory_file_count = status.file_count;
                memory_entry_counts = value_object_count_rows(&status.entry_count_by_kind);
                memory_allowed_prefixes = status.allowed_prefixes;
                memory_recent_activity = memory_activity_rows(&status.recent_activity);
            }
            Ok(None) => {
                memory_status_label = "MemFS not configured".to_string();
            }
            Err(err) => {
                memory_status_label = format!("Error: {err}");
            }
        }
    }

    let composed = crate::core::bears::compose_role_context(
        bear,
        role,
        Some("Runtime/conversation context is injected when this role handles a specific task."),
    )?;
    let role_contract = if composed.role_contract.trim().is_empty() {
        "Legacy/manual Bear prompt; no role-aware contract is stored yet.".to_string()
    } else {
        composed.role_contract
    };

    let mut actions = Vec::new();
    match role {
        BearAgentRole::Talk => {
            actions.push(RoleActionLink {
                label: "Open chat",
                href: format!("/bear/{}", bear.slug),
            });
            actions.push(RoleActionLink {
                label: "All conversations",
                href: format!("/bear/{}/details/conversations", bear.slug),
            });
        }
        BearAgentRole::Pair => {
            actions.push(RoleActionLink {
                label: "Code with this Bear",
                href: format!("/bear/{}/details/code-token", bear.slug),
            });
        }
        _ => {}
    }
    actions.push(RoleActionLink {
        label: "Role memory",
        href: format!("/bear/{}/details/memory?role={}", bear.slug, role.as_str()),
    });

    Ok(RoleDetailView {
        role: role.as_str().to_string(),
        label: role_memory_label(role).to_string(),
        plain_name: role_plain_name(role),
        description: role_memory_description(role).to_string(),
        surfaces: role_surfaces(role),
        capabilities: role_capabilities(role),
        memory: role_memory_rules(role),
        actions,
        runtime_family: role.runtime_family().to_string(),
        letta_agent_id: role_row.letta_agent_id,
        provisioning_status: role_row.provisioning_status,
        last_synced_at: role_row.last_synced_at,
        last_provisioning_error: agent.last_provisioning_error,
        memfs_view_state: role_row.memfs_view_state,
        memfs_view_quarantined: role_row.memfs_view_quarantined,
        memfs_view_diagnostic: role_row.memfs_view_diagnostic,
        memory_status_label,
        memory_file_count,
        memory_entry_counts,
        memory_allowed_prefixes,
        memory_recent_activity,
        role_contract,
        composed_prompt: composed.composed_prompt,
    })
}

async fn bear_role_rows(
    state: &AppState,
    bear_id: Uuid,
) -> Result<Vec<BearRoleViewRow>, CustomError> {
    bears_db::ensure_bear_agent_rows(state.sqlx_pool(), bear_id).await?;
    let memfs_url = state.config.letta_memfs_service_url.trim().to_string();
    let mut rows = Vec::new();
    for agent in bears_db::list_bear_agents(state.sqlx_pool(), bear_id).await? {
        let role = agent
            .parsed_role()
            .map_err(|err| CustomError::System(format!("invalid bear agent role in DB: {err}")))?;
        let mut row = BearRoleViewRow::from_agent(agent, role);
        if !memfs_url.is_empty() {
            match fetch_memfs_role_view_health(
                state.letta.http(),
                &memfs_url,
                bear_id,
                role.as_str(),
            )
            .await
            {
                Ok(Some(view)) => {
                    row.memfs_view_state = Some(view.state);
                    row.memfs_view_quarantined = view.quarantined;
                    row.memfs_view_diagnostic = view.diagnostic;
                }
                Ok(None) => {}
                Err(err) => {
                    row.memfs_view_state = Some("error".to_string());
                    row.memfs_view_diagnostic = Some(err.to_string());
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

async fn talk_agent_id_for_bear(
    pool: &sqlx::PgPool,
    bear: &Bear,
) -> Result<Option<String>, CustomError> {
    bears_db::role_agent_id(pool, bear.id, BearAgentRole::Talk)
        .await
        .map(|v| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

async fn pair_agent_id_for_bear(
    pool: &sqlx::PgPool,
    bear: &Bear,
) -> Result<Option<String>, CustomError> {
    bears_db::role_agent_id(pool, bear.id, BearAgentRole::Pair)
        .await
        .map(|v| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

fn web_href_for_conversation(slug: &str, conversation_id: &str) -> String {
    if conversation_id == "default" {
        format!("/bear/{slug}/")
    } else {
        format!(
            "/bear/{}/?conversation_id={}",
            slug,
            urlencoding::encode(conversation_id)
        )
    }
}

async fn acp_conversation_ids_for_bear(
    pool: &sqlx::PgPool,
    bear: &Bear,
) -> Result<std::collections::HashSet<String>, CustomError> {
    Ok(
        acp_sessions::resolved_conversation_ids_for_bear(pool, &bear.slug)
            .await?
            .into_iter()
            .collect(),
    )
}

async fn bear_code_token_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    render_template(
        &state,
        "bear/code_token.html",
        auth_session,
        context! {
            bear,
            token_name => format!("Zed - {}", bear.name),
            raw_token => None::<String>,
            api_server_url => state.config.api_server_url.clone(),
        },
    )
    .await
}

async fn bear_code_token_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<CodeTokenForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let token_name = form.name.trim();
    let created =
        acp_tokens::create_for_bear(state.sqlx_pool(), user_id, bear.id, token_name).await?;

    render_template(
        &state,
        "bear/code_token.html",
        auth_session,
        context! {
            bear,
            token_name => token_name,
            raw_token => created.raw_token,
            token_id => created.id.to_string(),
            api_server_url => state.config.api_server_url.clone(),
        },
    )
    .await
}

async fn new_bear_get(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let form = NewBearForm::default();
    let page = bear_new_form_context(&state, &form).await;
    render_template(
        &state,
        "bear/new.html",
        auth_session,
        context! {
            form,
            ..page
        },
    )
    .await
}

async fn new_bear_post(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let letta_fetch = if state.letta.is_enabled() {
        Some(state.letta.list_llm_models().await.map(|opts| {
            let model_trim = form.default_model.trim();
            let h = (!model_trim.is_empty()).then_some(model_trim);
            ensure_stored_model_in_options_for_handle(h, opts)
        }))
    } else {
        None
    };

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let letta_tool_ids: Vec<String> = form
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let letta_agent_type_db: Option<String> = {
        let t = form.letta_agent_type.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    };

    let default_model_trim = form.default_model.trim();
    validate_default_model_for_letta(&letta_fetch, default_model_trim, &mut validation_errors);

    let default_model_opt = if default_model_trim.is_empty() {
        None
    } else {
        Some(default_model_trim)
    };

    if bears_db::bear_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let id = insert_new_bear_row(
            state.sqlx_pool(),
            &form,
            letta_tool_ids.clone(),
            letta_agent_type_db.clone(),
            default_model_opt,
        )
        .await?;

        bears_db::grant_membership(state.sqlx_pool(), user_id, id, Some(BEAR_ROLE_ADMIN)).await?;

        if let Err(e) = provision::provision_bear_if_configured(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            id,
        )
        .await
        {
            if state.letta.is_enabled() {
                tracing::warn!(%id, "Letta provision failed: {e}");
                let page = bear_new_form_context(&state, &form).await;
                return render_template(
                    &state,
                    "bear/new.html",
                    auth_session,
                    context! {
                        form => form,
                        provision_error => e.to_string(),
                        ..page
                    },
                )
                .await;
            }
        }

        if state.letta.is_enabled() {
            let sync_summary = sync::sync_all_bear_roles_to_letta(
                state.sqlx_pool(),
                state.letta.as_ref(),
                state.bifrost.as_ref(),
                id,
            )
            .await?;
            if let Some(message) = sync_summary.diagnostic_message() {
                tracing::warn!(bear_id = %id, message = %message, "Letta role sync after member bear create had failures");
                let page = bear_new_form_context(&state, &form).await;
                return render_template(
                    &state,
                    "bear/new.html",
                    auth_session,
                    context! {
                        form => form,
                        letta_sync_error => format!(
                            "Bear was saved and provisioned, but one or more role agents rejected syncing fields: {message}"
                        ),
                        ..page
                    },
                )
                .await;
            }
        }

        let bear = bears_db::get_bear(state.sqlx_pool(), id)
            .await?
            .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
        return Ok(Redirect::to(&format!("/bear/{}/details", bear.slug)).into_response());
    }

    let page = bear_new_form_context(&state, &form).await;
    render_template(
        &state,
        "bear/new.html",
        auth_session,
        context! {
            errors => validation_errors,
            form => form,
            ..page
        },
    )
    .await
}

/// Renders [`bear/details.html`].
async fn render_bear_details_page(
    state: &AppState,
    auth_session: AuthSession,
    bear: Bear,
    members: Vec<BearMemberRow>,
    can_manage_bear: bool,
    letta_resync_query: Option<String>,
) -> Result<Response, CustomError> {
    let letta_configured = state.letta.is_enabled();
    let letta_api_base = state.config.letta_base_url.trim().to_string();
    let slug = bear.slug.clone();
    let talk_agent_id = talk_agent_id_for_bear(state.sqlx_pool(), &bear).await?;
    let pair_agent_id = pair_agent_id_for_bear(state.sqlx_pool(), &bear).await?;
    let role_rows = bear_role_rows(state, bear.id).await?;
    let mut role_details = Vec::new();
    for role in BearAgentRole::ALL {
        role_details.push(build_role_detail_view(state, &bear, role).await?);
    }

    let (letta_agent_summary, letta_agent_fetch_error, letta_drift) = if letta_configured {
        if let Some(agent_id) = talk_agent_id.as_deref() {
            match state.letta.fetch_agent(agent_id).await {
                Ok(v) => {
                    let summary = AgentSummary::from_letta_agent_state(&v);
                    let diagnostics = LettaAgentDiagnostics::from_agent_json(&v);
                    let expected_tool_ids = state
                        .letta
                        .filtered_tool_ids(&bear.letta_tool_ids.0)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!(bear_id = %bear.id, "Could not filter Letta tools for drift comparison: {e}");
                            bear.letta_tool_ids.0.clone()
                        });
                    let drift = compute_letta_drift_with_expected_tool_ids(
                        &bear,
                        Some(&summary),
                        Some(&diagnostics),
                        Some(&v),
                        Some(&expected_tool_ids),
                    );
                    (Some(summary), None, drift)
                }
                Err(e) => {
                    let msg = e.to_string();
                    (None, Some(msg), None)
                }
            }
        } else {
            (None, None, None)
        }
    } else {
        (None, None, None)
    };

    let (conversation_rows, archived_conversation_count) = if letta_configured {
        let archived_ids =
            archived_conversations::list_for_bear(state.sqlx_pool(), bear.id).await?;
        let acp_ids = acp_conversation_ids_for_bear(state.sqlx_pool(), &bear).await?;
        let mut rows = Vec::new();
        let mut archived_count = 0usize;
        if let Some(agent_id) = talk_agent_id.as_deref() {
            let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
            archived_count += snap
                .all
                .iter()
                .filter(|r| r.archived || archived_ids.contains(&r.id))
                .count();
            rows.extend(
                snap.all
                    .into_iter()
                    .filter(|r| !r.archived && !archived_ids.contains(&r.id))
                    .map(|r| DetailsConvRow {
                        web_href: web_href_for_conversation(&slug, &r.id),
                        id: r.id,
                        title: r.title,
                        last_message_at: r.last_message_at,
                        channel_label: "Web",
                        archived: false,
                    }),
            );
        }
        if let Some(agent_id) = pair_agent_id.as_deref() {
            let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
            archived_count += snap
                .all
                .iter()
                .filter(|r| r.archived || archived_ids.contains(&r.id))
                .count();
            rows.extend(
                snap.all
                    .into_iter()
                    .filter(|r| acp_ids.contains(&r.id))
                    .filter(|r| !r.archived && !archived_ids.contains(&r.id))
                    .map(|r| DetailsConvRow {
                        web_href: web_href_for_conversation(&slug, &r.id),
                        id: r.id,
                        title: r.title,
                        last_message_at: r.last_message_at,
                        channel_label: "ACP",
                        archived: false,
                    }),
            );
        }
        rows.sort_by(|a, b| b.last_message_at.cmp(&a.last_message_at));
        (rows, archived_count)
    } else {
        (Vec::new(), 0)
    };

    let context_profile = crate::core::bears::context_profile_from_json(&bear.context_profile)?;
    let user_steering = context_profile
        .as_ref()
        .map(|p| p.user_steering.trim().to_string())
        .filter(|s| !s.is_empty());
    let bear_context = context_profile
        .as_ref()
        .map(|p| p.bear_context.trim().to_string())
        .filter(|s| !s.is_empty());
    let template_label = context_profile.as_ref().and_then(|p| {
        p.template_id.as_deref().and_then(|id| {
            crate::core::bears::templates::first_bear_template(id)
                .map(|template| template.name.to_string())
                .or_else(|| Some(id.to_string()))
        })
    });
    let talk_composed_prompt = crate::core::bears::compose_role_context(
        &bear,
        BearAgentRole::Talk,
        Some("Runtime/conversation context is injected when this role handles a specific chat."),
    )?
    .composed_prompt;
    let pair_composed_prompt = crate::core::bears::compose_role_context(
        &bear,
        BearAgentRole::Pair,
        Some("Runtime/conversation context is injected when this role handles a specific ACP/client session."),
    )?
    .composed_prompt;

    let letta_tool_ids_display = if bear.letta_tool_ids.0.is_empty() {
        None
    } else {
        Some(bear.letta_tool_ids.0.join(", "))
    };

    let letta_resync_notice = match letta_resync_query.as_deref() {
        Some("ok") => Some("ok"),
        Some("error") => Some("error"),
        Some("drift") => Some("drift"),
        _ => None,
    };
    let acp_tool_details = acp_tool_detail_rows();
    let work_surface_rows = bear_work_surface_rows(&state.config, bear.id).await?;
    let web_sources = bear_web_sources(state.sqlx_pool(), bear.id).await?;
    let web_approvals = bear_web_approvals(state.sqlx_pool(), bear.id).await?;
    let web_fetches = bear_web_fetches(state.sqlx_pool(), bear.id).await?;
    let plan_mode_rows = bear_plan_mode_rows(state.sqlx_pool(), bear.id).await?;

    let memfs_url = state.config.letta_memfs_service_url.as_str();
    let (
        mem_private_files,
        mem_private_error,
        mem_private_skipped,
        mem_private_no_repo,
        mem_health,
        mem_health_error,
    ) = if !memfs_url.is_empty() && letta_configured {
        if let Some(agent_id) = talk_agent_id.as_deref() {
            let mem_health_result =
                fetch_memory_manager_repository_status(state.letta.http(), memfs_url, agent_id)
                    .await;
            let (mem_health, mem_health_error) = match mem_health_result {
                Ok(status) => (status, None),
                Err(e) => (None, Some(e.to_string())),
            };
            match fetch_memory_manager_repository_files(state.letta.http(), memfs_url, agent_id)
                .await
            {
                Ok(None) => (None, None, false, true, mem_health, mem_health_error),
                Ok(Some(files)) => (
                    Some(files),
                    None,
                    false,
                    false,
                    mem_health,
                    mem_health_error,
                ),
                Err(e) => (
                    None,
                    Some(e.to_string()),
                    false,
                    false,
                    mem_health,
                    mem_health_error,
                ),
            }
        } else {
            (None, None, true, false, None, None)
        }
    } else {
        (None, None, true, false, None, None)
    };

    render_template(
        state,
        "bear/details.html",
        auth_session,
        context! {
            bear,
            can_manage_bear,
            members,
            letta_configured,
            letta_api_base,
            talk_agent_id,
            role_rows,
            role_details,
            context_profile_enabled => bear.context_profile.is_some(),
            user_steering,
            bear_context,
            template_label,
            talk_composed_prompt,
            pair_composed_prompt,
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_drift,
            letta_tool_ids_display,
            conversation_rows,
            archived_conversation_count,
            letta_resync_notice,
            acp_tool_details,
            mem_private_files,
            mem_private_error,
            mem_private_skipped,
            mem_private_no_repo,
            mem_health,
            mem_health_error,
            work_surface_rows,
            web_sources,
            web_approvals,
            web_fetches,
            plan_mode_rows,
        },
    )
    .await
}

async fn bear_details_get(
    Path(slug): Path<String>,
    Query(q): Query<BearDetailsQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    let members = bears_db::list_members_for_bear(state.sqlx_pool(), bear.id).await?;

    render_bear_details_page(
        &state,
        auth_session,
        bear,
        members,
        can_manage_bear,
        q.letta_resync,
    )
    .await
}

async fn bear_resync_letta_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    let target = format!("/bear/{}/details", bear.slug);
    if !state.letta.is_enabled() {
        return Ok(Redirect::to(&format!("{target}?letta_resync=error")).into_response());
    }

    let sync_summary = sync::sync_all_bear_roles_to_letta(
        state.sqlx_pool(),
        state.letta.as_ref(),
        state.bifrost.as_ref(),
        bear.id,
    )
    .await?;
    if let Some(message) = sync_summary.diagnostic_message() {
        tracing::warn!(bear_id = %bear.id, message = %message, "Letta role resync from details had failures");
        return Ok(Redirect::to(&format!("{target}?letta_resync=error")).into_response());
    }

    let Some(agent_id) = talk_agent_id_for_bear(state.sqlx_pool(), &bear).await? else {
        return Ok(Redirect::to(&format!("{target}?letta_resync=error")).into_response());
    };

    let still_drifted = match state.letta.fetch_agent(&agent_id).await {
        Ok(v) => {
            let summary = AgentSummary::from_letta_agent_state(&v);
            let diagnostics = LettaAgentDiagnostics::from_agent_json(&v);
            let expected_tool_ids = state
                .letta
                .filtered_tool_ids(&bear.letta_tool_ids.0)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(bear_id = %bear.id, "Could not filter Letta tools after resync: {e}");
                    bear.letta_tool_ids.0.clone()
                });
            compute_letta_drift_with_expected_tool_ids(
                &bear,
                Some(&summary),
                Some(&diagnostics),
                Some(&v),
                Some(&expected_tool_ids),
            )
            .is_some_and(|flags| flags.drift_any)
        }
        Err(e) => {
            tracing::warn!(bear_id = %bear.id, "Could not verify Letta state after resync: {e}");
            true
        }
    };

    if still_drifted {
        Ok(Redirect::to(&format!("{target}?letta_resync=drift")).into_response())
    } else {
        Ok(Redirect::to(&format!("{target}?letta_resync=ok")).into_response())
    }
}

async fn bear_edit_redirect_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let _bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    Ok(Redirect::to(&format!("/bear/{}/details/edit/overview", slug.trim())).into_response())
}

async fn bear_edit_overview_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    let form = BearOverviewEditForm::from(&bear);
    render_template(
        &state,
        "bear/edit_overview.html",
        auth_session,
        context! {
            bear,
            form,
            errors => ValidationErrors::new(),
        },
    )
    .await
}

async fn bear_edit_overview_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearOverviewEditForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    if bears_db::bear_slug_exists_excluding(state.sqlx_pool(), form.slug.trim(), bear.id).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            bear.id,
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            bear.system_prompt.as_str(),
            bear.default_model.as_deref(),
            None::<Json<serde_json::Value>>,
            bear.letta_agent_type.as_deref(),
            Json(bear.letta_tool_ids.0.clone()),
        )
        .await?;

        if let Err(e) = sync::sync_bear_to_letta(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            bear.id,
        )
        .await
        {
            tracing::warn!(bear_id = %bear.id, "Letta sync after overview edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), bear.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            return render_template(
                &state,
                "bear/edit_overview.html",
                auth_session,
                context! {
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    letta_sync_error => format!(
                        "Bear was saved in Den, but Letta rejected the update: {e}"
                    ),
                },
            )
            .await;
        }

        let out_slug = form.slug.trim().to_string();
        return Ok(Redirect::to(&format!("/bear/{out_slug}/details")).into_response());
    }

    render_template(
        &state,
        "bear/edit_overview.html",
        auth_session,
        context! {
            errors => validation_errors,
            form => form,
            bear,
        },
    )
    .await
}

async fn bear_edit_prompt_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    let form = BearPromptEditForm::from(&bear);
    render_template(
        &state,
        "bear/edit_prompt.html",
        auth_session,
        context! {
            bear,
            form,
            errors => ValidationErrors::new(),
        },
    )
    .await
}

async fn bear_edit_prompt_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearPromptEditForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            bear.id,
            bear.slug.as_str(),
            bear.name.as_str(),
            bear.description.as_str(),
            form.system_prompt.trim(),
            bear.default_model.as_deref(),
            None::<Json<serde_json::Value>>,
            bear.letta_agent_type.as_deref(),
            Json(bear.letta_tool_ids.0.clone()),
        )
        .await?;

        if let Err(e) = sync::sync_bear_to_letta(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            bear.id,
        )
        .await
        {
            tracing::warn!(bear_id = %bear.id, "Letta sync after prompt edit failed: {e}");
            return render_template(
                &state,
                "bear/edit_prompt.html",
                auth_session,
                context! {
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    letta_sync_error => format!(
                        "Bear was saved in Den, but Letta rejected the update: {e}"
                    ),
                },
            )
            .await;
        }

        return Ok(Redirect::to(&format!("/bear/{}/details", bear.slug)).into_response());
    }

    render_template(
        &state,
        "bear/edit_prompt.html",
        auth_session,
        context! {
            errors => validation_errors,
            form => form,
            bear,
        },
    )
    .await
}

async fn bear_edit_configuration_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    let form = BearConfigurationEditForm::from(&bear);
    let page = bear_configuration_page_context(&state, &bear, &form).await;
    render_template(
        &state,
        "bear/edit_configuration.html",
        auth_session,
        context! {
            bear,
            form,
            errors => ValidationErrors::new(),
            ..page
        },
    )
    .await
}

async fn bear_edit_configuration_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearConfigurationEditForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    let letta_fetch = if state.letta.is_enabled() {
        Some(state.letta.list_llm_models().await.map(|opts| {
            let model_trim = form.default_model.trim();
            let h = (!model_trim.is_empty()).then_some(model_trim);
            ensure_stored_model_in_options_for_handle(h, opts)
        }))
    } else {
        None
    };

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let letta_tool_ids: Vec<String> = form
        .letta_tool_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let letta_agent_type_db: Option<String> = {
        let t = form.letta_agent_type.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    };

    let default_model_trim = form.default_model.trim();
    validate_default_model_for_letta(&letta_fetch, default_model_trim, &mut validation_errors);

    let default_model_opt = if default_model_trim.is_empty() {
        None
    } else {
        Some(default_model_trim)
    };

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            bear.id,
            bear.slug.as_str(),
            bear.name.as_str(),
            bear.description.as_str(),
            bear.system_prompt.as_str(),
            default_model_opt,
            None::<Json<serde_json::Value>>,
            letta_agent_type_db.as_deref(),
            Json(letta_tool_ids.clone()),
        )
        .await?;

        if let Err(e) = sync::sync_bear_to_letta(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            bear.id,
        )
        .await
        {
            tracing::warn!(bear_id = %bear.id, "Letta sync after configuration edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), bear.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            let page = bear_configuration_page_context(&state, &bear, &form).await;
            return render_template(
                &state,
                "bear/edit_configuration.html",
                auth_session,
                context! {
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    letta_sync_error => format!(
                        "Bear was saved in Den, but Letta rejected the update: {e}"
                    ),
                    ..page
                },
            )
            .await;
        }

        return Ok(Redirect::to(&format!("/bear/{}/details", bear.slug)).into_response());
    }

    let page = bear_configuration_page_context(&state, &bear, &form).await;
    render_template(
        &state,
        "bear/edit_configuration.html",
        auth_session,
        context! {
            errors => validation_errors,
            form => form,
            bear,
            ..page
        },
    )
    .await
}

async fn bear_role_get(
    Path((slug, role)): Path<(String, String)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let role = role
        .parse::<BearAgentRole>()
        .map_err(|err| CustomError::NotFound(err.to_string()))?;
    let role_detail = build_role_detail_view(&state, &bear, role).await?;
    let role_rows = bear_role_rows(&state, bear.id).await?;

    render_template(
        &state,
        "bear/role_detail.html",
        auth_session,
        context! {
            bear,
            role_detail,
            role_rows,
        },
    )
    .await
}

async fn bear_access_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    let members = bears_db::list_members_for_bear(state.sqlx_pool(), bear.id).await?;
    render_template(
        &state,
        "bear/access.html",
        auth_session,
        context! {
            bear,
            members,
        },
    )
    .await
}

async fn bear_conversations_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let letta_configured = state.letta.is_enabled();

    let talk_agent_id = talk_agent_id_for_bear(state.sqlx_pool(), &bear).await?;
    let pair_agent_id = pair_agent_id_for_bear(state.sqlx_pool(), &bear).await?;
    let (rows, list_error) = if letta_configured {
        let archived_ids =
            archived_conversations::list_for_bear(state.sqlx_pool(), bear.id).await?;
        let acp_ids = acp_conversation_ids_for_bear(state.sqlx_pool(), &bear).await?;
        let mut rows = Vec::new();
        if let Some(agent_id) = talk_agent_id.as_deref() {
            let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
            rows.extend(snap.all.into_iter().map(|mut r| {
                if archived_ids.contains(&r.id) {
                    r.archived = true;
                }
                DetailsConvRow {
                    web_href: web_href_for_conversation(&bear.slug, &r.id),
                    id: r.id,
                    title: r.title,
                    last_message_at: r.last_message_at,
                    channel_label: "Web",
                    archived: r.archived,
                }
            }));
        }
        if let Some(agent_id) = pair_agent_id.as_deref() {
            let snap = load_agent_conversations(state.letta.as_ref(), agent_id).await;
            rows.extend(
                snap.all
                    .into_iter()
                    .filter(|r| acp_ids.contains(&r.id))
                    .map(|mut r| {
                        if archived_ids.contains(&r.id) {
                            r.archived = true;
                        }
                        DetailsConvRow {
                            web_href: web_href_for_conversation(&bear.slug, &r.id),
                            id: r.id,
                            title: r.title,
                            last_message_at: r.last_message_at,
                            channel_label: "ACP",
                            archived: r.archived,
                        }
                    }),
            );
        }
        rows.sort_by(|a, b| b.last_message_at.cmp(&a.last_message_at));
        let list_error = if talk_agent_id.is_none() && pair_agent_id.is_none() {
            Some("No talk or pair role Letta agent is linked to this bear.".to_string())
        } else {
            None
        };
        (rows, list_error)
    } else {
        (Vec::new(), Some("Letta is not configured.".to_string()))
    };

    render_template(
        &state,
        "bear/conversations.html",
        auth_session,
        context! {
            bear,
            conversation_rows => rows,
            list_error,
        },
    )
    .await
}

async fn bear_memory_get(
    Path(slug): Path<String>,
    Query(q): Query<BearMemoryQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let letta_configured = state.letta.is_enabled();
    let memfs_url = state.config.letta_memfs_service_url.as_str();
    let requested_role = q.role.as_deref().unwrap_or("pair");
    let selected_role = requested_role
        .parse::<BearAgentRole>()
        .unwrap_or(BearAgentRole::Pair);
    let selected_role_name = selected_role.as_str().to_string();
    let search_query = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let selected_path = q.path.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let delete_notice = q.deleted;
    let review_notice = q.review_requested;
    let delete_error = q.error.as_deref().map(str::trim).filter(|s| !s.is_empty());

    bears_db::ensure_bear_agent_rows(state.sqlx_pool(), bear.id).await?;
    let agents = bears_db::list_bear_agents(state.sqlx_pool(), bear.id).await?;
    let mut role_rows = Vec::new();
    for agent in agents {
        let role = agent
            .parsed_role()
            .map_err(|err| CustomError::System(format!("invalid bear agent role in DB: {err}")))?;
        let mut row = BearMemoryRoleRow {
            role: role.as_str().to_string(),
            label: role_memory_label(role).to_string(),
            description: role_memory_description(role).to_string(),
            runtime_family: role.runtime_family().to_string(),
            letta_agent_id: agent.letta_agent_id,
            provisioning_status: agent.provisioning_status,
            selected: role == selected_role,
            status_state: None,
            status_label: "Unavailable".to_string(),
            file_count: 0,
            registered_view_count: 0,
            canonical_tip: None,
            allowed_prefixes: Vec::new(),
            entry_counts: Vec::new(),
            recent_activity: Vec::new(),
            error: None,
        };
        if !memfs_url.is_empty() {
            match fetch_memfs_role_memory_status(
                state.letta.http(),
                memfs_url,
                bear.id,
                role.as_str(),
            )
            .await
            {
                Ok(Some(status)) => {
                    row.status_state = status.canonical_tip.as_ref().map(|_| "ok".to_string());
                    row.status_label = if status.ok {
                        "Available"
                    } else {
                        "Unavailable"
                    }
                    .to_string();
                    row.file_count = status.file_count;
                    row.registered_view_count = status.registered_view_count;
                    row.canonical_tip = status.canonical_tip;
                    row.allowed_prefixes = status.allowed_prefixes;
                    row.entry_counts = value_object_count_rows(&status.entry_count_by_kind);
                    row.recent_activity = memory_activity_rows(&status.recent_activity);
                }
                Ok(None) => {
                    row.status_label = "MemFS not configured".to_string();
                }
                Err(err) => {
                    row.status_label = "Error".to_string();
                    row.error = Some(err.to_string());
                }
            }
        } else {
            row.status_label = "MemFS not configured".to_string();
        }
        role_rows.push(row);
    }

    let selected_tree = if !memfs_url.is_empty() {
        match fetch_memfs_role_memory_tree(
            state.letta.http(),
            memfs_url,
            bear.id,
            selected_role.as_str(),
        )
        .await
        {
            Ok(Some(tree)) => Some(tree),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(bear_id = %bear.id, role = selected_role.as_str(), "Could not load memory tree: {err}");
                None
            }
        }
    } else {
        None
    };

    let search_results = if !memfs_url.is_empty() {
        if let Some(query) = search_query {
            match search_memfs_role_memory(
                state.letta.http(),
                memfs_url,
                bear.id,
                selected_role.as_str(),
                query,
                Some(50),
            )
            .await
            {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(bear_id = %bear.id, role = selected_role.as_str(), "Could not search memory: {err}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let selected_file = if !memfs_url.is_empty() {
        if let Some(path) = selected_path {
            match fetch_memfs_role_memory_file(
                state.letta.http(),
                memfs_url,
                bear.id,
                selected_role.as_str(),
                path,
            )
            .await
            {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(bear_id = %bear.id, role = selected_role.as_str(), path = path, "Could not read memory file: {err}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let pair_reflection_runs =
        pair_reflection::list_recent_for_bear(state.sqlx_pool(), bear.id, 10)
            .await
            .unwrap_or_default();
    let memory_proposals = memory_proposals::list_for_bear(state.sqlx_pool(), bear.id, None, 25)
        .await
        .unwrap_or_default();

    let runtime_block_count = if letta_configured {
        let mut count = 0usize;
        for row in &role_rows {
            if let Some(agent_id) = row.letta_agent_id.as_deref() {
                if let Ok(v) = state.letta.fetch_agent(agent_id).await {
                    count += LettaAgentDiagnostics::from_agent_json(&v).blocks.len();
                }
            }
        }
        Some(count)
    } else {
        None
    };

    render_template(
        &state,
        "bear/memory.html",
        auth_session,
        context! {
            bear,
            letta_configured,
            role_rows,
            selected_role => selected_role_name,
            search_query => search_query.unwrap_or(""),
            selected_path => selected_path.unwrap_or(""),
            selected_tree,
            search_results,
            selected_file,
            runtime_block_count,
            pair_reflection_runs,
            memory_proposals,
            memfs_configured => !memfs_url.is_empty(),
            delete_notice,
            review_notice,
            delete_error,
        },
    )
    .await
}

async fn bear_memory_delete_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearMemoryDeleteForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    let role = form
        .role
        .parse::<BearAgentRole>()
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
    if paths.is_empty() {
        let target = format!(
            "/bear/{}/details/memory?role={}&error={}",
            bear.slug,
            role.as_str(),
            urlencoding::encode("Select at least one memory file.")
        );
        return Ok(Redirect::to(&target).into_response());
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
            .unwrap_or("Selected memory files were marked for Reflection/curate review from the Bear memory UI.");
        let proposal = memory_proposals::create(
            state.sqlx_pool(),
            CreateMemoryProposal {
                bear_id: bear.id,
                source_role: role,
                source_agent_id: bears_db::role_agent_id(state.sqlx_pool(), bear.id, role).await?,
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
            },
        )
        .await?;
        let target = format!(
            "/bear/{}/details/memory?role={}&review_requested=1&path={}",
            bear.slug,
            role.as_str(),
            urlencoding::encode(
                proposal
                    .source_paths
                    .first()
                    .map(String::as_str)
                    .unwrap_or("")
            )
        );
        return Ok(Redirect::to(&target).into_response());
    }
    if confirm != role.as_str() && confirm != bear.slug {
        let target = format!(
            "/bear/{}/details/memory?role={}&error={}",
            bear.slug,
            role.as_str(),
            urlencoding::encode("Type the role name or Bear slug to confirm deletion.")
        );
        return Ok(Redirect::to(&target).into_response());
    }
    let memfs_url = state.config.letta_memfs_service_url.as_str();
    let deleted = match delete_memfs_role_memory_entries(
        state.letta.http(),
        memfs_url,
        bear.id,
        role.as_str(),
        &paths,
    )
    .await
    {
        Ok(Some(response)) => response.deleted.len(),
        Ok(None) => {
            let target = format!(
                "/bear/{}/details/memory?role={}&error={}",
                bear.slug,
                role.as_str(),
                urlencoding::encode("MemFS Manager is not configured.")
            );
            return Ok(Redirect::to(&target).into_response());
        }
        Err(err) => {
            let target = format!(
                "/bear/{}/details/memory?role={}&error={}",
                bear.slug,
                role.as_str(),
                urlencoding::encode(&err.to_string())
            );
            return Ok(Redirect::to(&target).into_response());
        }
    };
    let target = format!(
        "/bear/{}/details/memory?role={}&deleted={}",
        bear.slug,
        role.as_str(),
        deleted
    );
    Ok(Redirect::to(&target).into_response())
}

async fn bear_memory_proposal_get(
    Path((slug, proposal_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    let can_manage_bear = viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await?;
    let proposal = memory_proposals::get_for_bear(state.sqlx_pool(), bear.id, proposal_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("memory proposal not found".to_string()))?;
    render_template(
        &state,
        "bear/memory_proposal.html",
        auth_session,
        context! {
            bear,
            proposal,
            can_manage_bear,
            errors => None::<String>,
        },
    )
    .await
}

async fn bear_memory_proposal_post(
    Path((slug, proposal_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearMemoryProposalResolutionForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user.id).await? {
        return Ok(r.into_response());
    }
    let bear = load_bear_member(state.sqlx_pool(), user.id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
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
    memory_proposals::resolve_for_bear(
        state.sqlx_pool(),
        bear.id,
        proposal_id,
        BearAgentRole::Curate,
        None,
        status,
        form.review_notes.as_deref(),
        form.decision_summary.as_deref(),
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/details/memory/proposals/{}",
        bear.slug, proposal_id
    ))
    .into_response())
}

async fn bear_runtime_blocks_get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    let letta_configured = state.letta.is_enabled();
    bears_db::ensure_bear_agent_rows(state.sqlx_pool(), bear.id).await?;
    let agents = bears_db::list_bear_agents(state.sqlx_pool(), bear.id).await?;
    let mut rows = Vec::new();
    for agent in agents {
        let role = agent
            .parsed_role()
            .map_err(|err| CustomError::System(format!("invalid bear agent role in DB: {err}")))?;
        let mut row = RuntimeBlockRoleRow {
            role: role.as_str().to_string(),
            label: role_memory_label(role).to_string(),
            letta_agent_id: agent.letta_agent_id.clone(),
            block_count: 0,
            diagnostics: None,
            error: None,
        };
        if letta_configured {
            if let Some(agent_id) = agent.letta_agent_id.as_deref() {
                match state.letta.fetch_agent(agent_id).await {
                    Ok(v) => {
                        let diagnostics = LettaAgentDiagnostics::from_agent_json(&v);
                        row.block_count = diagnostics.blocks.len();
                        row.diagnostics = Some(diagnostics);
                    }
                    Err(err) => row.error = Some(err.to_string()),
                }
            }
        }
        rows.push(row);
    }

    render_template(
        &state,
        "bear/runtime_blocks.html",
        auth_session,
        context! {
            bear,
            letta_configured,
            runtime_block_rows => rows,
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct BearDeleteForm {
    confirm_slug: String,
}

async fn bear_delete_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(body): Form<BearDeleteForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }
    if body.confirm_slug.trim() != bear.slug {
        return Err(CustomError::ValidationError(
            "confirmation slug does not match".to_string(),
        ));
    }
    bears_db::delete_bear(state.sqlx_pool(), bear.id).await?;
    Ok(Redirect::to("/").into_response())
}

#[derive(Debug, Deserialize, Validate)]
struct MemberAddForm {
    #[validate(length(min = 1, max = 120))]
    username: String,
    /// `admin` or `member`
    #[validate(length(min = 1, max = 32))]
    role: String,
}

async fn member_add_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<MemberAddForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    if let Err(e) = form.validate() {
        return Err(CustomError::ValidationError(format!("{e:?}")));
    }

    let uname = form.username.trim();
    let role_trim = form.role.trim().to_ascii_lowercase();
    let role_db = if role_trim == BEAR_ROLE_ADMIN {
        Some(BEAR_ROLE_ADMIN)
    } else if role_trim == BEAR_ROLE_MEMBER || role_trim.is_empty() {
        Some(BEAR_ROLE_MEMBER)
    } else {
        return Err(CustomError::ValidationError(
            "role must be admin or member".to_string(),
        ));
    };

    let target = user_db::get_user_by_username(state.sqlx_pool(), uname)
        .await?
        .ok_or_else(|| CustomError::NotFound("user not found".to_string()))?;

    bears_db::grant_membership(state.sqlx_pool(), target.id, bear.id, role_db).await?;

    Ok(Redirect::to(&format!("/bear/{}/details/access", bear.slug)).into_response())
}

#[derive(Debug, Deserialize)]
struct MemberRemoveForm {
    remove_user_id: i32,
}

async fn member_remove_post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(body): Form<MemberRemoveForm>,
) -> Result<Response, CustomError> {
    let user = auth_session
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    let user_id = user.id;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin or site admin role required".to_string(),
        ));
    }

    let target_role =
        bears_db::membership_role_for_user(state.sqlx_pool(), body.remove_user_id, bear.id)
            .await?
            .ok_or_else(|| {
                CustomError::NotFound("user is not a member of this bear".to_string())
            })?;

    if role_is_bear_admin(target_role.as_deref()) {
        let n = bears_db::count_bear_admins(state.sqlx_pool(), bear.id).await?;
        if n <= 1 {
            return Err(CustomError::ValidationError(
                "cannot remove the last bear admin; promote another admin first".to_string(),
            ));
        }
    }

    bears_db::revoke_membership(state.sqlx_pool(), body.remove_user_id, bear.id).await?;

    Ok(Redirect::to(&format!("/bear/{}/details/access", bear.slug)).into_response())
}
