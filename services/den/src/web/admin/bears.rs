// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
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
use std::collections::HashSet;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::{
        bears::{db as bears_db, db::BearParams, provision, sync, BearAgent, BearAgentRole},
        letta::{AgentSummary, LettaAgentListItem},
        memory_manager_head::fetch_memfs_role_view_health,
        web_policy,
    },
    errors::CustomError,
    web::{self, AppState},
};

use crate::web::bear_create_support::{
    bear_edit_page_context, bear_new_form_context, ensure_stored_model_in_options_for_handle,
    validate_default_model_for_letta, NewBearForm,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bears/", get(list_view))
        .route_with_tsr(
            "/bears/unlinked-letta-agents",
            get(unlinked_letta_agents_view),
        )
        .route_with_tsr(
            "/bears/register-memfs-views",
            post(register_memfs_views_action),
        )
        .route_with_tsr("/bears/new", get(new_view).post(new_action))
        .route_with_tsr("/bears/{id}/edit", get(edit_view).post(edit_action))
        .route_with_tsr("/bears/{id}/web-sources", post(add_web_source_action))
        .route_with_tsr(
            "/bears/{id}/web-sources/{source_id}/delete",
            post(delete_web_source_action),
        )
        .route_with_tsr("/bears/{id}/web-approvals", post(add_web_approval_action))
        .route_with_tsr(
            "/bears/{id}/web-approvals/{approval_id}/revoke",
            post(revoke_web_approval_action),
        )
        .route_with_tsr(
            "/bears/{id}/provision-missing-roles",
            post(provision_missing_roles_action),
        )
        .route_with_tsr("/bears/{id}/retry-letta", post(retry_letta_action))
        .route_with_tsr("/bears/{id}", get(detail_view))
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

#[derive(Debug, Deserialize)]
struct AddWebSourceForm {
    scope_kind: String,
    scope_value: String,
    policy: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct AddWebApprovalForm {
    scope_kind: String,
    scope_value: String,
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
struct BearAgentHealthRow {
    role: String,
    runtime_family: String,
    branch: String,
    letta_agent_id: Option<String>,
    provisioning_status: String,
    last_provisioned_version: i32,
    last_synced_at: Option<String>,
    health_status: String,
    health_label: String,
    health_detail: Option<String>,
    letta_name: Option<String>,
    letta_model: Option<String>,
    letta_agent_type: Option<String>,
    letta_tool_count: Option<usize>,
    letta_memory_block_count: Option<usize>,
    memfs_view_state: Option<String>,
    memfs_view_quarantined: bool,
    memfs_view_diagnostic: Option<String>,
}

impl BearAgentHealthRow {
    fn not_configured(agent: &BearAgent, role: BearAgentRole) -> Self {
        Self {
            role: role.as_str().to_string(),
            runtime_family: role.runtime_family().to_string(),
            branch: role.as_str().to_string(),
            letta_agent_id: agent.letta_agent_id.clone(),
            provisioning_status: agent.provisioning_status.clone(),
            last_provisioned_version: agent.last_provisioned_version,
            last_synced_at: agent.last_synced_at.map(|t| t.to_string()),
            health_status: "unknown".to_string(),
            health_label: "Not checked".to_string(),
            health_detail: Some("Letta is not configured on this Den instance.".to_string()),
            letta_name: None,
            letta_model: None,
            letta_agent_type: None,
            letta_tool_count: None,
            letta_memory_block_count: None,
            memfs_view_state: None,
            memfs_view_quarantined: false,
            memfs_view_diagnostic: None,
        }
    }

    fn missing(agent: &BearAgent, role: BearAgentRole) -> Self {
        Self {
            role: role.as_str().to_string(),
            runtime_family: role.runtime_family().to_string(),
            branch: role.as_str().to_string(),
            letta_agent_id: None,
            provisioning_status: agent.provisioning_status.clone(),
            last_provisioned_version: agent.last_provisioned_version,
            last_synced_at: agent.last_synced_at.map(|t| t.to_string()),
            health_status: "missing".to_string(),
            health_label: "No agent id".to_string(),
            health_detail: agent
                .last_provisioning_error
                .clone()
                .or_else(|| Some("No Letta agent id is recorded for this role.".to_string())),
            letta_name: None,
            letta_model: None,
            letta_agent_type: None,
            letta_tool_count: None,
            letta_memory_block_count: None,
            memfs_view_state: None,
            memfs_view_quarantined: false,
            memfs_view_diagnostic: None,
        }
    }

    fn ok(agent: &BearAgent, role: BearAgentRole, summary: AgentSummary) -> Self {
        Self {
            role: role.as_str().to_string(),
            runtime_family: role.runtime_family().to_string(),
            branch: role.as_str().to_string(),
            letta_agent_id: agent.letta_agent_id.clone(),
            provisioning_status: agent.provisioning_status.clone(),
            last_provisioned_version: agent.last_provisioned_version,
            last_synced_at: agent.last_synced_at.map(|t| t.to_string()),
            health_status: "ok".to_string(),
            health_label: "OK".to_string(),
            health_detail: None,
            letta_name: summary.name,
            letta_model: summary.model,
            letta_agent_type: summary.agent_type,
            letta_tool_count: summary.tool_count,
            letta_memory_block_count: summary.memory_block_count,
            memfs_view_state: None,
            memfs_view_quarantined: false,
            memfs_view_diagnostic: None,
        }
    }

    fn error(agent: &BearAgent, role: BearAgentRole, error: String) -> Self {
        Self {
            role: role.as_str().to_string(),
            runtime_family: role.runtime_family().to_string(),
            branch: role.as_str().to_string(),
            letta_agent_id: agent.letta_agent_id.clone(),
            provisioning_status: agent.provisioning_status.clone(),
            last_provisioned_version: agent.last_provisioned_version,
            last_synced_at: agent.last_synced_at.map(|t| t.to_string()),
            health_status: "error".to_string(),
            health_label: "Fetch failed".to_string(),
            health_detail: Some(error),
            letta_name: None,
            letta_model: None,
            letta_agent_type: None,
            letta_tool_count: None,
            letta_memory_block_count: None,
            memfs_view_state: None,
            memfs_view_quarantined: false,
            memfs_view_diagnostic: None,
        }
    }
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
        SELECT p.id, p.user_id, u.username, p.acp_session_id, p.state, p.reason,
               p.plan_artifact_path, p.plan_title, p.created_at, p.updated_at
        FROM acp_plan_mode_sessions p
        LEFT JOIN users u ON u.id = p.user_id
        WHERE p.bear_id = $1
        ORDER BY p.updated_at DESC
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

async fn bear_agent_health_rows(
    state: &AppState,
    bear_id: Uuid,
    letta_configured: bool,
) -> Result<Vec<BearAgentHealthRow>, CustomError> {
    bears_db::ensure_bear_agent_rows(state.sqlx_pool(), bear_id).await?;
    let agents = bears_db::list_bear_agents(state.sqlx_pool(), bear_id).await?;
    let memfs_url = state.config.letta_memfs_service_url.trim().to_string();
    let mut rows = Vec::with_capacity(agents.len());
    for agent in agents {
        let role = agent
            .parsed_role()
            .map_err(|err| CustomError::System(format!("invalid bear agent role in DB: {err}")))?;
        let mut row = if !letta_configured {
            BearAgentHealthRow::not_configured(&agent, role)
        } else if let Some(agent_id) = agent
            .letta_agent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match state.letta.fetch_agent(agent_id).await {
                Ok(v) => {
                    BearAgentHealthRow::ok(&agent, role, AgentSummary::from_letta_agent_state(&v))
                }
                Err(err) => BearAgentHealthRow::error(&agent, role, err.to_string()),
            }
        } else {
            BearAgentHealthRow::missing(&agent, role)
        };

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

async fn bear_detail_response(
    state: &AppState,
    auth_session: AuthSession,
    id: Uuid,
    letta_retry_message: Option<String>,
) -> Result<Response, CustomError> {
    let web_message = letta_retry_message;
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let member_count = bears_db::count_bear_members(state.sqlx_pool(), id).await?;

    let letta_api_base = state.config.letta_base_url.trim().to_string();
    let letta_configured = state.letta.is_enabled();

    let agent_health_rows = bear_agent_health_rows(state, id, letta_configured).await?;
    let web_sources = bear_web_sources(state.sqlx_pool(), id).await?;
    let web_approvals = bear_web_approvals(state.sqlx_pool(), id).await?;
    let web_fetches = bear_web_fetches(state.sqlx_pool(), id).await?;
    let plan_mode_rows = bear_plan_mode_rows(state.sqlx_pool(), id).await?;

    let talk_agent_id = bears_db::role_agent_id(state.sqlx_pool(), bear.id, BearAgentRole::Talk)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (letta_agent_summary, letta_agent_fetch_error): (Option<AgentSummary>, Option<String>) =
        if letta_configured {
            if let Some(agent_id) = talk_agent_id.as_deref() {
                match state.letta.fetch_agent(agent_id).await {
                    Ok(v) => (Some(AgentSummary::from_letta_agent_state(&v)), None),
                    Err(e) => (None, Some(e.to_string())),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    let tools_json_display = bear
        .tools_enabled
        .as_ref()
        .and_then(|j| serde_json::to_string_pretty(&j.0).ok())
        .filter(|s| !s.trim().is_empty());

    let letta_tool_ids_display = if bear.letta_tool_ids.0.is_empty() {
        None
    } else {
        Some(bear.letta_tool_ids.0.join(", "))
    };

    let letta_memory_blocks_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.memory_block_count)
        .map(|n| n.to_string());
    let letta_tools_count_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.tool_count)
        .map(|n| n.to_string());

    web::render_template(
        state,
        "admin/bears/detail.html",
        auth_session,
        context! {
            bear,
            member_count,
            letta_api_base,
            letta_configured,
            talk_agent_id,
            agent_health_rows,
            web_sources,
            web_approvals,
            web_fetches,
            plan_mode_rows,
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_retry_message => web_message.clone(),
            web_message,
            tools_json_display,
            letta_tool_ids_display,
            letta_memory_blocks_label,
            letta_tools_count_label,
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct BearDetailQuery {
    #[serde(default)]
    message: Option<String>,
}

async fn detail_view(
    Path(id): Path<Uuid>,
    Query(query): Query<BearDetailQuery>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    bear_detail_response(&state, auth_session, id, query.message).await
}

#[derive(Debug, Deserialize)]
struct BearsListQuery {
    #[serde(default)]
    error: Option<String>,
    memfs: Option<String>,
    views: Option<usize>,
}

async fn list_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<BearsListQuery>,
) -> Result<Response, CustomError> {
    let bears = bears_db::list_bears(state.sqlx_pool()).await?;
    let memfs_message = match query.memfs.as_deref() {
        Some("ok") => Some(format!(
            "MemFS sidecar role view registration complete: {} view(s) registered/refreshed.",
            query.views.unwrap_or(0)
        )),
        Some("error") => Some(format!(
            "MemFS sidecar role view registration failed: {}",
            query
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        )),
        _ => None,
    };
    web::render_template(
        &state,
        "admin/bears/list.html",
        auth_session,
        context! { bears, memfs_message },
    )
    .await
}

async fn register_memfs_views_action(
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    match provision::register_existing_role_views(state.sqlx_pool(), state.letta.as_ref()).await {
        Ok(count) => {
            Ok(Redirect::to(&format!("/admin/bears/?memfs=ok&views={count}")).into_response())
        }
        Err(err) => Ok(Redirect::to(&format!(
            "/admin/bears/?memfs=error&error={}",
            urlencoding::encode(&err.to_string())
        ))
        .into_response()),
    }
}

async fn new_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(_query): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, CustomError> {
    let form = NewBearForm::default();
    let page = bear_new_form_context(&state, &form).await;
    web::render_template(
        &state,
        "admin/bears/new.html",
        auth_session,
        context! {
            form,
            ..page
        },
    )
    .await
}

#[derive(Serialize)]
struct UnlinkedLettaAgentRow {
    display_name: String,
    agent_id: String,
}

async fn unlinked_letta_agents_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let mut letta_list_error: Option<String> = None;
    let mut unlinked_rows: Vec<UnlinkedLettaAgentRow> = Vec::new();

    if !state.letta.is_enabled() {
        letta_list_error = Some(
            "Letta is not configured (set LETTA_BASE_URL). Listing requires Letta.".to_string(),
        );
    } else {
        match state.letta.list_agents().await {
            Ok(agents) => {
                let in_use: HashSet<String> =
                    bears_db::list_letta_agent_ids_in_use(state.sqlx_pool())
                        .await?
                        .into_iter()
                        .collect();
                for a in agents {
                    if in_use.contains(&a.id) {
                        continue;
                    }
                    let LettaAgentListItem { id, name } = a;
                    let display_name = name.clone().unwrap_or_else(|| id.clone());
                    unlinked_rows.push(UnlinkedLettaAgentRow {
                        display_name,
                        agent_id: id,
                    });
                }
            }
            Err(e) => letta_list_error = Some(e.to_string()),
        }
    }

    web::render_template(
        &state,
        "admin/bears/unlinked_letta_agents.html",
        auth_session,
        context! {
            unlinked_rows,
            letta_list_error,
            letta_configured => state.letta.is_enabled(),
        },
    )
    .await
}

pub async fn new_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
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
        let id = bears_db::create_bear(
            state.sqlx_pool(),
            BearParams {
                slug: form.slug.trim(),
                name: form.name.trim(),
                description: form.description.trim(),
                system_prompt: form.system_prompt.trim(),
                default_model: default_model_opt,
                tools_enabled: None::<Json<serde_json::Value>>,
                letta_agent_type: letta_agent_type_db.as_deref(),
                letta_tool_ids: Json(letta_tool_ids.clone()),
                context_profile: None,
            },
        )
        .await?;

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
                return web::render_template(
                    &state,
                    "admin/bears/new.html",
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
                tracing::warn!(%id, message = %message, "Letta role sync after create had failures");
                let page = bear_new_form_context(&state, &form).await;
                return web::render_template(
                    &state,
                    "admin/bears/new.html",
                    auth_session,
                    context! {
                        form => form,
                        letta_sync_error => format!(
                            "Bear was saved and provisioned in Den, but one or more role agents rejected syncing fields: {message}"
                        ),
                        ..page
                    },
                )
                .await;
            }
        }

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        let page = bear_new_form_context(&state, &form).await;
        web::render_template(
            &state,
            "admin/bears/new.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                ..page
            },
        )
        .await
    }
}

async fn edit_view(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let form = NewBearForm::from(&bear);
    let page = bear_edit_page_context(&state, &bear, &form).await;
    web::render_template(
        &state,
        "admin/bears/edit.html",
        auth_session,
        context! {
            bear,
            form,
            ..page
        },
    )
    .await
}

async fn edit_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

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

    if bears_db::bear_slug_exists_excluding(state.sqlx_pool(), form.slug.trim(), id).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            id,
            BearParams {
                slug: form.slug.trim(),
                name: form.name.trim(),
                description: form.description.trim(),
                system_prompt: form.system_prompt.trim(),
                default_model: default_model_opt,
                tools_enabled: None::<Json<serde_json::Value>>,
                letta_agent_type: letta_agent_type_db.as_deref(),
                letta_tool_ids: Json(letta_tool_ids.clone()),
                context_profile: None,
            },
        )
        .await?;

        let sync_summary = sync::sync_all_bear_roles_to_letta(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            id,
        )
        .await?;
        if let Some(message) = sync_summary.diagnostic_message() {
            tracing::warn!(%id, message = %message, "Letta role sync after bear edit had failures");
            let bear = bears_db::get_bear(state.sqlx_pool(), id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            let page = bear_edit_page_context(&state, &bear, &form).await;
            let empty_errors = ValidationErrors::new();
            let skipped = sync_summary.skipped_roles().len();
            return web::render_template(
                &state,
                "admin/bears/edit.html",
                auth_session,
                context! {
                    errors => empty_errors,
                    form => form,
                    bear,
                    letta_sync_error => format!(
                        "Bear was saved in Den. {}. {} role(s) synced; {} unprovisioned role(s) skipped. Use the Bear detail page to inspect per-role health and provision missing roles.",
                        message,
                        sync_summary.synced_count(),
                        skipped
                    ),
                    ..page
                },
            )
            .await;
        }

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        let page = bear_edit_page_context(&state, &bear, &form).await;
        web::render_template(
            &state,
            "admin/bears/edit.html",
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
}

async fn add_web_source_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<AddWebSourceForm>,
) -> Result<Response, CustomError> {
    let scope_kind = form.scope_kind.trim();
    let policy = form.policy.trim();
    if !matches!(scope_kind, "host" | "url")
        || !matches!(policy, "preferred" | "allowed" | "blocked")
    {
        return bear_detail_response(
            &state,
            auth_session,
            id,
            Some("Invalid web source policy form.".to_string()),
        )
        .await;
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return bear_detail_response(&state, auth_session, id, Some(err.to_string())).await
        }
    };
    sqlx::query(
        r#"
        INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, label, policy, priority)
        VALUES ($1, $2, $3, NULLIF($4, ''), $5, $6)
        ON CONFLICT (bear_id, scope_kind, scope_value)
        DO UPDATE SET label = EXCLUDED.label,
                      policy = EXCLUDED.policy,
                      priority = EXCLUDED.priority,
                      updated_at = now()
        "#,
    )
    .bind(id)
    .bind(scope_kind)
    .bind(scope_value)
    .bind(form.label.trim())
    .bind(policy)
    .bind(form.priority.unwrap_or(0))
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}?message={}",
        urlencoding::encode("Web source saved.")
    ))
    .into_response())
}

async fn delete_web_source_action(
    Path((id, source_id)): Path<(Uuid, Uuid)>,
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    sqlx::query("DELETE FROM bear_web_sources WHERE bear_id = $1 AND id = $2")
        .bind(id)
        .bind(source_id)
        .execute(state.sqlx_pool())
        .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}?message={}",
        urlencoding::encode("Web source deleted.")
    ))
    .into_response())
}

async fn add_web_approval_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<AddWebApprovalForm>,
) -> Result<Response, CustomError> {
    let scope_kind = form.scope_kind.trim();
    if !matches!(scope_kind, "host" | "url") {
        return bear_detail_response(
            &state,
            auth_session,
            id,
            Some("Invalid web approval scope.".to_string()),
        )
        .await;
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return bear_detail_response(&state, auth_session, id, Some(err.to_string())).await
        }
    };
    web_policy::record_web_approval(
        state.sqlx_pool(),
        id,
        scope_kind,
        &scope_value,
        None,
        "admin",
        None,
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}?message={}",
        urlencoding::encode("Web approval added.")
    ))
    .into_response())
}

async fn revoke_web_approval_action(
    Path((id, approval_id)): Path<(Uuid, Uuid)>,
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    sqlx::query("UPDATE bear_web_approvals SET revoked_at = now() WHERE bear_id = $1 AND id = $2")
        .bind(id)
        .bind(approval_id)
        .execute(state.sqlx_pool())
        .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}?message={}",
        urlencoding::encode("Web approval revoked.")
    ))
    .into_response())
}

async fn provision_missing_roles_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let message = match provision::provision_missing_bear_roles(
        state.sqlx_pool(),
        state.letta.as_ref(),
        state.bifrost.as_ref(),
        id,
    )
    .await
    {
        Ok(0) => "No missing role agents to provision.".to_string(),
        Ok(n) => format!("Provisioned {n} missing role agent(s)."),
        Err(err) => format!("Provisioning missing role agents failed: {err}"),
    };

    bear_detail_response(&state, auth_session, id, Some(message)).await
}

async fn retry_letta_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    if bears_db::get_bear(state.sqlx_pool(), id).await?.is_none() {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }

    let existing_agents = bears_db::list_bear_agents(state.sqlx_pool(), id).await?;
    let has_any_role_agent = existing_agents.iter().any(|agent| {
        agent
            .letta_agent_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
    });

    let letta_retry_message = if !state.letta.is_enabled() {
        "Letta is not configured (set LETTA_BASE_URL).".to_string()
    } else if has_any_role_agent {
        "This bear already has one or more role agents. Use 'Provision missing role agents' to fill only empty roles.".to_string()
    } else {
        match provision::provision_bear_if_configured(
            state.sqlx_pool(),
            state.letta.as_ref(),
            state.bifrost.as_ref(),
            id,
        )
        .await
        {
            Ok(()) => {
                match sync::sync_all_bear_roles_to_letta(
                    state.sqlx_pool(),
                    state.letta.as_ref(),
                    state.bifrost.as_ref(),
                    id,
                )
                .await
                {
                    Ok(summary) => format!(
                        "Role agent provisioning finished. {} role(s) synced; {} unprovisioned role(s) skipped.",
                        summary.synced_count(),
                        summary.skipped_roles().len()
                    ),
                    Err(e) => format!(
                        "Role agent provisioning finished, but follow-up role sync failed: {e}"
                    ),
                }
            }
            Err(e) => format!("Letta provisioning failed: {e}"),
        }
    };

    bear_detail_response(&state, auth_session, id, Some(letta_retry_message)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use minijinja::Environment;
    use sqlx::{postgres::PgPoolOptions, types::Json};
    use std::sync::Arc;
    use tower::ServiceExt;
    use tower_sessions_sqlx_store::PostgresStore;

    use crate::{config::Config, startup::run_sqlx_migrations, web::AppState};

    static TEST_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn test_pool() -> Option<sqlx::PgPool> {
        dotenvy::dotenv().ok();
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping DB-backed admin route test: DATABASE_URL is not set");
            return None;
        };
        let pool = match PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&url)
            .await
        {
            Ok(pool) => pool,
            Err(err) => {
                eprintln!(
                    "skipping DB-backed admin route test: could not connect to DATABASE_URL: {err}"
                );
                return None;
            }
        };
        if let Err(err) = run_sqlx_migrations(&pool).await {
            eprintln!("skipping DB-backed admin route test: migrations failed: {err}");
            return None;
        }
        Some(pool)
    }

    fn test_state(pool: sqlx::PgPool) -> AppState {
        let config = Arc::new(Config::test_stub());
        let mut template_env = Environment::new();
        template_env
            .add_template("admin/bears/detail.html", "{{ web_message }} {{ web_sources | length }} {{ web_approvals | length }} {{ web_fetches | length }}{% for approval in web_approvals %} {{ approval.approved_by_user_label }}{% endfor %}")
            .expect("add test template");
        AppState::test_with_template_env(pool, template_env, config)
    }

    async fn test_app(pool: sqlx::PgPool) -> axum::Router {
        let store = PostgresStore::new(pool.clone());
        store.migrate().await.expect("session store migration");
        Router::new()
            .merge(router())
            .with_state(test_state(pool.clone()))
            .layer(
                axum_login::AuthManagerLayerBuilder::new(
                    crate::auth_backend::Backend::new(pool),
                    axum_login::tower_sessions::SessionManagerLayer::new(store),
                )
                .build(),
            )
    }

    async fn create_test_bear(pool: &sqlx::PgPool) -> Uuid {
        bears_db::create_bear(
            pool,
            &format!("web-admin-{}", Uuid::new_v4()),
            "Web Admin Test Bear",
            "",
            "System prompt",
            None,
            None::<Json<serde_json::Value>>,
            None,
            Json(Vec::new()),
        )
        .await
        .expect("create bear")
    }

    async fn create_test_user(pool: &sqlx::PgPool) -> i32 {
        sqlx::query_scalar::<_, i32>(
            r#"
            INSERT INTO users (email, username, display_name, passhash, is_admin)
            VALUES ($1, $2, $3, $4, true)
            RETURNING id
            "#,
        )
        .bind(format!("web-admin-{}@example.test", Uuid::new_v4()))
        .bind(format!("webadmin{}", Uuid::new_v4().simple()))
        .bind("Admin Display")
        .bind("test-passhash")
        .fetch_one(pool)
        .await
        .expect("create user")
    }

    #[tokio::test]
    async fn add_web_source_route_normalizes_host_and_flashes() {
        let _guard = TEST_DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let bear_id = create_test_bear(&pool).await;
        let _user_id = create_test_user(&pool).await;
        let app = test_app(pool.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bears/{bear_id}/web-sources"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("scope_kind=host&scope_value=Example.COM%3A8443.&policy=preferred&label=Docs&priority=10"))
                    .unwrap(),
            )
            .await
            .expect("add source response");

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .contains("message=Web%20source%20saved"));
        let stored: String = sqlx::query_scalar(
            "SELECT scope_value FROM bear_web_sources WHERE bear_id = $1 AND scope_kind = 'host'",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("stored source");
        assert_eq!(stored, "example.com:8443");
    }

    #[tokio::test]
    async fn add_web_source_route_rejects_url_in_host_scope() {
        let _guard = TEST_DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let bear_id = create_test_bear(&pool).await;
        let _user_id = create_test_user(&pool).await;
        let app = test_app(pool.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bears/{bear_id}/web-sources"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("scope_kind=host&scope_value=https%3A%2F%2Fexample.com%2Fdocs&policy=preferred&label=&priority=0"))
                    .unwrap(),
            )
            .await
            .expect("validation response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("host must be a bare hostname"));
    }

    #[tokio::test]
    async fn add_and_revoke_web_approval_routes_update_active_approvals() {
        let _guard = TEST_DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let bear_id = create_test_bear(&pool).await;
        let _user_id = create_test_user(&pool).await;
        let app = test_app(pool.clone()).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bears/{bear_id}/web-approvals"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("scope_kind=host&scope_value=Docs.RS"))
                    .unwrap(),
            )
            .await
            .expect("add approval response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let approval_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM bear_web_approvals WHERE bear_id = $1 AND scope_value = 'docs.rs' AND revoked_at IS NULL",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("active approval");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/bears/{bear_id}/web-approvals/{approval_id}/revoke"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("revoke response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM bear_web_approvals WHERE bear_id = $1 AND revoked_at IS NULL",
        )
        .bind(bear_id)
        .fetch_one(&pool)
        .await
        .expect("approval count");
        assert_eq!(active_count, 0);
    }

    #[tokio::test]
    async fn detail_route_displays_approval_user_label_and_recent_fetches() {
        let _guard = TEST_DB_LOCK.lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let bear_id = create_test_bear(&pool).await;
        let user_id = create_test_user(&pool).await;
        web_policy::record_web_approval(
            &pool,
            bear_id,
            "host",
            "example.com",
            Some(user_id),
            "admin",
            None,
        )
        .await
        .expect("record approval");
        web_policy::record_web_fetch_attempt(
            &pool,
            bear_id,
            Some("session-1"),
            Some("tool-1"),
            "https://example.com/",
            None,
            "example.com",
            "den",
            "user_host",
            Some(200),
            Some("text/html"),
            Some(123),
        )
        .await
        .expect("record fetch");

        let app = test_app(pool.clone()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/bears/{bear_id}?message=Saved"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("detail response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("Saved"));
        assert!(body.contains("Admin Display"));
        assert!(body.contains("1 1 1"));
    }
}
