//! Member-facing bear lifecycle: create bears (you become admin), details, membership, edit/delete for bear admins (or site operators).
//! When changing routes, update `src/web/ROUTES.md`.
// TODO(den-web extraction): several detail/edit/member handlers here were superseded
// by `bear::settings` + legacy redirects and are now dead. Remove during the den-web
// move rather than blind-deleting now. `allow(dead_code)` keeps the clippy gate green.
#![allow(dead_code)]

use axum::{
    extract::{Path, State},
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
    auth_backend::AuthSession,
    core::{armature_tokens, user::db as user_db},
    errors::CustomError,
    web::{
        bear::create_support::{
            bear_configuration_page_context, bear_new_form_context, canonical_default_model_handle,
            BearConfigurationEditForm, BearOverviewEditForm, BearPromptEditForm, NewBearForm,
        },
        render_template, AppState,
    },
};
use den_core::client_tools::{client_tool_policy_json_for_provider, ClientToolName};
use den_memory::tools::sqlite_collect_role_logical_paths;
use den_service::bears::{
    db as bears_db,
    db::{role_is_bear_admin, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
    provision, Bear, RuntimeContextLabel,
};
use den_service::client_sessions;

pub(crate) use super::member::{email_verify_redirect, load_bear_member, viewer_can_manage_bear};
use super::{hats, settings};

pub fn router() -> Router<AppState> {
    settings::router()
        .merge(hats::router())
        .route_with_tsr("/bears/new", get(new_bear_get).post(new_bear_post))
        .route_with_tsr("/bear/{slug}/details", get(legacy_details_redirect))
        .route(
            "/bear/{slug}/details/{*rest}",
            get(legacy_details_path_redirect),
        )
        .route_with_tsr("/bear/{slug}/edit", get(bear_edit_redirect_get))
        .route_with_tsr(
            "/bear/{slug}/edit/overview",
            get(bear_edit_overview_get).post(bear_edit_overview_post),
        )
        .route_with_tsr(
            "/bear/{slug}/edit/prompt",
            get(bear_edit_prompt_get).post(bear_edit_prompt_post),
        )
        .route_with_tsr(
            "/bear/{slug}/edit/configuration",
            get(bear_edit_configuration_get).post(bear_edit_configuration_post),
        )
        .route_with_tsr(
            "/bear/{slug}/code-token",
            get(bear_code_token_get).post(bear_code_token_post),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/browse/runtime-blocks",
            get(runtime_blocks_redirect),
        )
        .route_with_tsr(
            "/bear/{slug}/memory/browse/proposals/{proposal_id}",
            get(memory_proposal_legacy_redirect),
        )
        .route_with_tsr(
            "/bear/{slug}/delete",
            get(super::deletion::get).post(super::deletion::post),
        )
        .route_with_tsr("/bear/{slug}/members/add", post(member_add_post))
        .route_with_tsr("/bear/{slug}/members/remove", post(member_remove_post))
}

async fn legacy_details_redirect(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/bear/{}/overview", slug.trim()))
}

async fn legacy_details_path_redirect(Path((slug, rest)): Path<(String, String)>) -> Redirect {
    let slug = slug.trim();
    let path = rest.trim_start_matches('/');
    let target = match path {
        "" | "details" => format!("/bear/{slug}/overview"),
        "access" => format!("/bear/{slug}/access"),
        "conversations" => format!("/bear/{slug}/conversations"),
        "code-token" => format!("/bear/{slug}/code-token"),
        "edit" | "edit/overview" => format!("/bear/{slug}/edit/overview"),
        "edit/prompt" => format!("/bear/{slug}/edit/prompt"),
        "edit/configuration" => format!("/bear/{slug}/edit/configuration"),
        "memory" => format!("/bear/{slug}/memory"),
        p if p.starts_with("memory/") => {
            let rest = p.trim_start_matches("memory/");
            if rest.starts_with("browse/") {
                let sub = rest.trim_start_matches("browse/");
                if sub.starts_with("proposals/") {
                    format!("/bear/{slug}/memory/{sub}")
                } else if sub == "runtime-blocks" {
                    format!(
                        "/bear/{slug}/advanced?message={}",
                        urlencoding::encode("Runtime memory blocks view is deprecated.")
                    )
                } else {
                    format!("/bear/{slug}/memory?{}", sub.replace('/', "&"))
                }
            } else {
                format!("/bear/{slug}/memory/{rest}")
            }
        }
        p if p.starts_with("roles/") => {
            let profile = p.trim_start_matches("roles/");
            format!("/bear/{slug}/profiles/{profile}")
        }
        p if p.starts_with("profiles/") => format!("/bear/{slug}/{p}"),
        other => format!(
            "/bear/{slug}/overview?legacy={}",
            urlencoding::encode(other)
        ),
    };
    Redirect::permanent(&target)
}

async fn runtime_blocks_redirect(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!(
        "/bear/{}/advanced?message={}",
        slug.trim(),
        urlencoding::encode("Runtime memory blocks view is deprecated.")
    ))
}

async fn memory_proposal_legacy_redirect(
    Path((slug, proposal_id)): Path<(String, Uuid)>,
) -> Redirect {
    Redirect::permanent(&format!(
        "/bear/{}/memory/proposals/{proposal_id}",
        slug.trim()
    ))
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

#[derive(Debug, Deserialize)]
struct CodeTokenForm {
    name: String,
}

fn acp_tool_detail_rows() -> Vec<AcpToolDetailRow> {
    ClientToolName::all()
        .iter()
        .filter(|tool| {
            matches!(
                tool,
                ClientToolName::TerminalRunCommand
                    | ClientToolName::RunCommand
                    | ClientToolName::ProcessRun
                    | ClientToolName::ReadTextFile
                    | ClientToolName::ListDirectory
                    | ClientToolName::SearchFiles
                    | ClientToolName::EditFile
                    | ClientToolName::CreateTextFile
                    | ClientToolName::DeletePath
            )
        })
        .map(|tool| {
            let descriptor = tool.descriptor();
            let policy = client_tool_policy_json_for_provider(descriptor.provider_name);
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
                highlighted: descriptor.provider_name == "run_command",
            }
        })
        .collect()
}

fn acp_tool_approval_label(provider_name: &str) -> &'static str {
    match provider_name {
        "run_command" => "Only once, by command in workspace, by workspace, or globally",
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
        "run_command" => "Workspace cwd + routed command execution policy",
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
        "run_command" | "terminal_run_command" | "process_run" => vec![
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
        "run_command" => "Unified command tool. The adapter routes dedicated-tool redirects first, then chooses process or terminal execution based on command policy and visibility needs.",
        "terminal_run_command" => "Compatibility terminal command tool; unified run_command should be preferred for new model-facing flows.",
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

#[derive(Serialize)]
struct DetailsConvRow {
    id: String,
    title: String,
    last_message_at: Option<String>,
    channel_label: &'static str,
    web_href: String,
    archived: bool,
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
    profile_local_presence_count: usize,
    active_workplace_count: usize,
    known_in_workplace_count: usize,
    workplace_labels: Vec<String>,
    canonical_path_count: usize,
    anchor_status: String,
}

async fn read_native_memory_content(
    store: &den_memory::BearMemoryStore,
    logical_path: &str,
) -> Result<Option<String>, CustomError> {
    let value = den_memory::tools::sqlite_memory_read(store, logical_path).await?;
    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        return Ok(None);
    }
    Ok(value
        .get("content")
        .and_then(|v| v.as_str())
        .map(ToString::to_string))
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
    stores: &den_memory::MemoryStoreManager,
    bear_id: Uuid,
) -> Result<Vec<BearWorkSurfaceRow>, CustomError> {
    let mut rows = Vec::new();
    let store = stores.store_for_bear(bear_id).await?;

    let core_paths = sqlite_collect_role_logical_paths(
        &store,
        RuntimeContextLabel::ArmatureConversation.as_str(),
    )
    .await?;
    let pair_paths = &core_paths;
    let work_paths =
        sqlite_collect_role_logical_paths(&store, RuntimeContextLabel::JobRun.as_str()).await?;

    // Work surfaces are canonical core memory under `core/work_surfaces/{slug}/...`.
    let mut slugs: Vec<String> = core_paths
        .iter()
        .filter_map(|path| {
            let rest = path.strip_prefix("core/work_surfaces/")?;
            let slug = rest.split('/').next()?.trim();
            if slug.is_empty() || slug == "index.md" {
                None
            } else {
                Some(slug.to_string())
            }
        })
        .collect();
    slugs.sort();
    slugs.dedup();

    for slug in slugs {
        let slug_path_prefix = format!("core/work_surfaces/{slug}/");
        let child_paths = core_paths
            .iter()
            .filter(|path| path.starts_with(&slug_path_prefix))
            .cloned()
            .collect::<Vec<_>>();
        let canonical_path_count = child_paths.len();
        let index_path = format!("{slug_path_prefix}index.md");
        let overview_path = format!("{slug_path_prefix}overview.md");
        let glossary_path = format!("{slug_path_prefix}glossary.md");
        let index_content = read_native_memory_content(&store, &index_path).await?;
        let overview_content = read_native_memory_content(&store, &overview_path).await?;
        let display_name = index_content
            .as_deref()
            .map(|content| parse_work_surface_display_name(content, &slug))
            .unwrap_or_else(|| slug.clone());
        let summary = overview_content
            .as_deref()
            .and_then(first_nonempty_markdown_paragraph);
        let glossary_present = child_paths.iter().any(|path| path == &glossary_path);
        let pair_understanding_path = format!("pair/work_surfaces/{slug}/current-understanding.md");
        let work_understanding_path = format!("work/work_surfaces/{slug}/current-understanding.md");
        let pair_current_understanding_present = pair_paths
            .iter()
            .any(|path| path == &pair_understanding_path);
        let work_current_understanding_present = work_paths
            .iter()
            .any(|path| path == &work_understanding_path);
        let workplace_labels = [
            (
                RuntimeContextLabel::ArmatureConversation,
                pair_current_understanding_present,
            ),
            (
                RuntimeContextLabel::JobRun,
                work_current_understanding_present,
            ),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(role, _)| role.as_str().to_string())
        .collect::<Vec<_>>();
        let profile_local_presence_count = workplace_labels.len();
        let active_workplace_count = workplace_labels.len();
        let known_in_workplace_count = canonical_path_count + profile_local_presence_count;
        let anchor_status = if canonical_path_count == 0 {
            "missing_canonical".to_string()
        } else if profile_local_presence_count == 0 {
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
            profile_local_presence_count,
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
        r"
        SELECT id, scope_kind, scope_value, label, policy, priority, created_at
        FROM bear_web_sources
        WHERE bear_id = $1
        ORDER BY policy ASC, priority DESC, scope_kind ASC, scope_value ASC
        ",
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
        r"
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
        ",
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
        r"
        SELECT url, final_url, host, execution_location, approval_kind, http_status, content_type, bytes, fetched_at
        FROM bear_web_fetches
        WHERE bear_id = $1
        ORDER BY fetched_at DESC
        LIMIT 25
        ",
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
        r"
        SELECT s.id,
               s.user_id,
               u.username,
               s.client_session_id,
               s.state,
               s.reason,
               s.plan_artifact_path,
               s.plan_title,
               s.created_at,
               s.updated_at
        FROM client_plan_mode_sessions s
        LEFT JOIN users u ON u.id = s.user_id
        WHERE s.bear_id = $1
        ORDER BY s.updated_at DESC
        LIMIT 10
        ",
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
        client_sessions::resolved_conversation_ids_for_bear(pool, &bear.slug)
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
    let can_manage_bear = viewer_can_manage_bear(
        state.sqlx_pool(),
        super::settings::session_user(&auth_session).await?,
        bear.id,
    )
    .await?;
    render_template(
        &state,
        "bear/code_token.html",
        auth_session,
        context! {
            bear,
            bear_nav_active => "connections",
            can_manage_bear,
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
    let can_manage_bear = viewer_can_manage_bear(
        state.sqlx_pool(),
        super::settings::session_user(&auth_session).await?,
        bear.id,
    )
    .await?;
    let token_name = form.name.trim();
    let created =
        armature_tokens::create_for_bear(state.sqlx_pool(), user_id, bear.id, token_name).await?;

    render_template(
        &state,
        "bear/code_token.html",
        auth_session,
        context! {
            bear,
            bear_nav_active => "connections",
            can_manage_bear,
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

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    if bears_db::bear_slug_exists(state.sqlx_pool(), form.slug.trim()).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let id = match super::model_setup::create_with_validated_model(&state, &form).await {
            Ok(id) => id,
            Err(mut failure) => {
                if let Some(id) = failure.saved_bear {
                    // Expose a refused compensation to the creating human, without deleting work.
                    if bears_db::grant_membership(
                        state.sqlx_pool(),
                        user_id,
                        id.as_uuid(),
                        Some(BEAR_ROLE_ADMIN),
                    )
                    .await
                    .is_err()
                    {
                        failure.message.push_str(
                            " Ask a Den operator to repair your access to this saved Bear.",
                        );
                    }
                }
                let page = bear_new_form_context(&state, &form).await;
                let mut response = render_template(
                    &state,
                    "bear/new.html",
                    auth_session,
                    context! {
                        form, errors => validation_errors, provision_error => failure.message,
                        saved_bear_slug => failure.saved_bear.map(|_| form.slug.trim()),
                        ..page
                    },
                )
                .await?;
                *response.status_mut() = failure.status;
                return Ok(response);
            }
        };

        bears_db::grant_membership(state.sqlx_pool(), user_id, id, Some(BEAR_ROLE_ADMIN)).await?;

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await
        {
            tracing::warn!(%id, "Bear initialization failed: {e}");
            let page = bear_new_form_context(&state, &form).await;
            return render_template(
                &state,
                "bear/new.html",
                auth_session,
                context! {
                    form => form,
                    provision_error => e.to_string(),
                    saved_bear_slug => form.slug.trim(),
                    ..page
                },
            )
            .await;
        }

        let bear = bears_db::get_bear(state.sqlx_pool(), id)
            .await?
            .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
        return Ok(Redirect::to(&format!("/bear/{}/overview", bear.slug)).into_response());
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
    Ok(Redirect::to(&format!("/bear/{}/edit/overview", slug.trim())).into_response())
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
            "bear admin role required".to_string(),
        ));
    }
    let form = BearOverviewEditForm::from(&bear);
    render_template(
        &state,
        "bear/edit_overview.html",
        auth_session,
        context! {
            can_manage_bear => true,
            bear,
            bear_nav_active => "identity",
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
            "bear admin role required".to_string(),
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
            BearParams {
                slug: form.slug.trim(),
                name: form.name.trim(),
                description: form.description.trim(),
                system_prompt: bear.system_prompt.as_str(),
                default_model: bear.default_model.as_deref(),
                tools_enabled: None::<Json<serde_json::Value>>,
                context_profile: bear.context_profile.clone(),
            },
        )
        .await?;

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, bear.id)
                .await
        {
            tracing::warn!(bear_id = %bear.id, "Bear initialization after overview edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), bear.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            return render_template(
                &state,
                "bear/edit_overview.html",
                auth_session,
                context! {
                    can_manage_bear => true,
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    bear_nav_active => "identity",
                    provision_error => format!(
                        "Bear was saved in Den, but initialization failed: {e}"
                    ),
                },
            )
            .await;
        }

        let out_slug = form.slug.trim().to_string();
        return Ok(Redirect::to(&format!("/bear/{out_slug}/identity")).into_response());
    }

    render_template(
        &state,
        "bear/edit_overview.html",
        auth_session,
        context! {
            can_manage_bear => true,
            errors => validation_errors,
            form => form,
            bear,
            bear_nav_active => "identity",
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
            "bear admin role required".to_string(),
        ));
    }
    let form = BearPromptEditForm::from(&bear);
    render_template(
        &state,
        "bear/edit_prompt.html",
        auth_session,
        context! {
            can_manage_bear => true,
            bear,
            bear_nav_active => "identity",
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
            "bear admin role required".to_string(),
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
            BearParams {
                slug: bear.slug.as_str(),
                name: bear.name.as_str(),
                description: bear.description.as_str(),
                system_prompt: form.system_prompt.trim(),
                default_model: bear.default_model.as_deref(),
                tools_enabled: None::<Json<serde_json::Value>>,
                context_profile: bear.context_profile.clone(),
            },
        )
        .await?;

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, bear.id)
                .await
        {
            tracing::warn!(bear_id = %bear.id, "Bear initialization after prompt edit failed: {e}");
            return render_template(
                &state,
                "bear/edit_prompt.html",
                auth_session,
                context! {
                    can_manage_bear => true,
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    bear_nav_active => "identity",
                    provision_error => format!(
                        "Bear was saved in Den, but initialization failed: {e}"
                    ),
                },
            )
            .await;
        }

        return Ok(Redirect::to(&format!("/bear/{}/identity", bear.slug)).into_response());
    }

    render_template(
        &state,
        "bear/edit_prompt.html",
        auth_session,
        context! {
            can_manage_bear => true,
            errors => validation_errors,
            form => form,
            bear,
            bear_nav_active => "identity",
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
            "bear admin role required".to_string(),
        ));
    }
    // Keep legacy POSTs compatible, but send new edits to the canonical model UI.
    Ok(Redirect::to(&format!("/bear/{}/models", bear.slug)).into_response())
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
            "bear admin role required".to_string(),
        ));
    }

    let mut availability_status = None;
    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let default_model_trim = form.default_model.trim();
    let default_model_opt = canonical_default_model_handle(default_model_trim);
    if let Some(model) = default_model_opt.as_deref() {
        let result: Result<(), CustomError> = async {
            den_service::bears::model_configurations::validate_model_configuration(
                state.sqlx_pool(),
                model,
                None,
            )
            .await?;
            crate::model_availability::validate_model(
                &state,
                den_core::ids::BearId::new(bear.id),
                model,
            )
            .await
        }
        .await;
        if let Err(error) = result {
            let Some((status, message)) = crate::model_availability::form_failure(&error) else {
                return Err(error);
            };
            availability_status = Some(status);
            let mut error = ValidationError::new("model_availability");
            error.message = Some(message.into());
            validation_errors.add("default_model", error);
        }
    }

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            bear.id,
            BearParams {
                slug: bear.slug.as_str(),
                name: bear.name.as_str(),
                description: bear.description.as_str(),
                system_prompt: bear.system_prompt.as_str(),
                default_model: default_model_opt.as_deref(),
                tools_enabled: None::<Json<serde_json::Value>>,
                context_profile: bear.context_profile.clone(),
            },
        )
        .await?;

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, bear.id)
                .await
        {
            tracing::warn!(bear_id = %bear.id, "Bear initialization after configuration edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), bear.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            let page = bear_configuration_page_context(&state, &bear, &form).await;
            return render_template(
                &state,
                "bear/edit_configuration.html",
                auth_session,
                context! {
                    can_manage_bear => true,
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    bear_nav_active => "identity",
                    provision_error => format!(
                        "Bear was saved in Den, but initialization failed: {e}"
                    ),
                    ..page
                },
            )
            .await;
        }

        return Ok(Redirect::to(&format!("/bear/{}/identity", bear.slug)).into_response());
    }

    let page = bear_configuration_page_context(&state, &bear, &form).await;
    let mut response = render_template(
        &state,
        "bear/edit_configuration.html",
        auth_session,
        context! {
            can_manage_bear => true,
            errors => validation_errors,
            form => form,
            bear,
            bear_nav_active => "identity",
            ..page
        },
    )
    .await?;
    if let Some(status) = availability_status {
        *response.status_mut() = status;
    }
    Ok(response)
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
            "bear admin role required".to_string(),
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

    Ok(Redirect::to(&format!("/bear/{}/people", bear.slug)).into_response())
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
            "bear admin role required".to_string(),
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

    Ok(Redirect::to(&format!("/bear/{}/people", bear.slug)).into_response())
}
