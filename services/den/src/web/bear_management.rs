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
    core::{
        acp_sessions, acp_tokens, archived_conversations,
        bears::{
            compute_letta_drift_with_expected_tool_ids, db as bears_db,
            db::{role_is_bear_admin, BearMemberRow, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
            provision, sync, Bear, BearAgent, BearAgentRole,
        },
        letta::{load_agent_conversations, AgentSummary, LettaAgentDiagnostics},
        memory_manager_head::{
            fetch_memfs_role_memory_file, fetch_memfs_role_memory_status,
            fetch_memfs_role_memory_tree, fetch_memfs_role_view_health,
            fetch_memory_manager_repository_files, fetch_memory_manager_repository_status,
            search_memfs_role_memory,
        },
        user,
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
        .route_with_tsr("/bear/{slug}/details/memory", get(bear_memory_get))
        .route_with_tsr(
            "/bear/{slug}/details/memory/runtime-blocks",
            get(bear_runtime_blocks_get),
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
struct RuntimeBlockRoleRow {
    role: String,
    label: String,
    letta_agent_id: Option<String>,
    block_count: usize,
    diagnostics: Option<LettaAgentDiagnostics>,
    error: Option<String>,
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
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_drift,
            letta_tool_ids_display,
            conversation_rows,
            archived_conversation_count,
            letta_resync_notice,
            mem_private_files,
            mem_private_error,
            mem_private_skipped,
            mem_private_no_repo,
            mem_health,
            mem_health_error,
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
            match fetch_memfs_role_memory_status(state.letta.http(), memfs_url, bear.id, role.as_str()).await {
                Ok(Some(status)) => {
                    row.status_state = status.canonical_tip.as_ref().map(|_| "ok".to_string());
                    row.status_label = if status.ok { "Available" } else { "Unavailable" }.to_string();
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
            memfs_configured => !memfs_url.is_empty(),
        },
    )
    .await
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
