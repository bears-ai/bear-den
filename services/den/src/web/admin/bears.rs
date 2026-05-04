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
use std::borrow::Cow;
use std::collections::HashSet;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::{
        bears::{db as bears_db, provision, sync, BearAgent, BearAgentRole},
        letta::{AgentBearPrefill, AgentSummary, LettaAgentListItem},
        memory_manager_head::fetch_memfs_role_view_health,
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
            "/bears/repair-legacy-agents",
            post(repair_legacy_agents_action),
        )
        .route_with_tsr(
            "/bears/register-memfs-views",
            post(register_memfs_views_action),
        )
        .route_with_tsr("/bears/new", get(new_view).post(new_action))
        .route_with_tsr("/bears/{id}/edit", get(edit_view).post(edit_action))
        .route_with_tsr(
            "/bears/{id}/provision-missing-roles",
            post(provision_missing_roles_action),
        )
        .route_with_tsr("/bears/{id}/retry-letta", post(retry_letta_action))
        .route_with_tsr("/bears/{id}", get(detail_view))
}

#[derive(Debug, Deserialize)]
struct NewBearPageQuery {
    #[serde(default, rename = "from_letta_agent")]
    from_letta_agent: Option<String>,
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
                Ok(v) => BearAgentHealthRow::ok(
                    &agent,
                    role,
                    AgentSummary::from_letta_agent_state(&v),
                ),
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
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let member_count = bears_db::count_bear_members(state.sqlx_pool(), id).await?;

    let letta_api_base = state.config.letta_base_url.trim().to_string();
    let letta_configured = state.letta.is_enabled();

    let agent_health_rows = bear_agent_health_rows(state, id, letta_configured).await?;

    let talk_agent_id = bears_db::role_agent_id_or_legacy(state.sqlx_pool(), &bear, BearAgentRole::Talk)
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
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_retry_message,
            tools_json_display,
            letta_tool_ids_display,
            letta_memory_blocks_label,
            letta_tools_count_label,
        },
    )
    .await
}

async fn detail_view(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    bear_detail_response(&state, auth_session, id, None).await
}

#[derive(Debug, Deserialize)]
struct BearsListQuery {
    #[serde(default)]
    repair: Option<String>,
    #[serde(default)]
    migrated: Option<i64>,
    #[serde(default)]
    inserted: Option<i64>,
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
    let repair_message = match query.repair.as_deref() {
        Some("ok") => Some(format!(
            "Legacy bear-agent repair complete: {} talk row(s) migrated/updated; {} missing role row(s) inserted.",
            query.migrated.unwrap_or(0),
            query.inserted.unwrap_or(0)
        )),
        Some("error") => Some(format!(
            "Legacy bear-agent repair failed: {}",
            query.error.clone().unwrap_or_else(|| "unknown error".to_string())
        )),
        _ => None,
    };
    let memfs_message = match query.memfs.as_deref() {
        Some("ok") => Some(format!(
            "MemFS sidecar role view registration complete: {} view(s) registered/refreshed.",
            query.views.unwrap_or(0)
        )),
        Some("error") => Some(format!(
            "MemFS sidecar role view registration failed: {}",
            query.error.clone().unwrap_or_else(|| "unknown error".to_string())
        )),
        _ => None,
    };
    web::render_template(
        &state,
        "admin/bears/list.html",
        auth_session,
        context! { bears, repair_message, memfs_message },
    )
    .await
}

async fn register_memfs_views_action(State(state): State<AppState>) -> Result<Response, CustomError> {
    match provision::register_existing_role_views(state.sqlx_pool(), state.letta.as_ref()).await {
        Ok(count) => Ok(Redirect::to(&format!(
            "/admin/bears/?memfs=ok&views={count}"
        ))
        .into_response()),
        Err(err) => Ok(Redirect::to(&format!(
            "/admin/bears/?memfs=error&error={}",
            urlencoding::encode(&err.to_string())
        ))
        .into_response()),
    }
}

async fn repair_legacy_agents_action(State(state): State<AppState>) -> Result<Response, CustomError> {
    match bears_db::repair_legacy_bear_agents(state.sqlx_pool()).await {
        Ok(result) => Ok(Redirect::to(&format!(
            "/admin/bears/?repair=ok&migrated={}&inserted={}",
            result.migrated_talk_rows, result.inserted_missing_role_rows
        ))
        .into_response()),
        Err(err) => Ok(Redirect::to(&format!(
            "/admin/bears/?repair=error&error={}",
            urlencoding::encode(&err.to_string())
        ))
        .into_response()),
    }
}

async fn new_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<NewBearPageQuery>,
) -> Result<Response, CustomError> {
    let mut from_agent_error: Option<String> = None;
    let form = if let Some(raw) = query.from_letta_agent.as_ref() {
        let agent_id = raw.trim();
        if agent_id.is_empty() {
            NewBearForm::default()
        } else if !state.letta.is_enabled() {
            from_agent_error = Some(format!(
                "Letta is not configured; cannot pre-fill from agent {agent_id:?}."
            ));
            NewBearForm::default()
        } else {
            match state.letta.fetch_agent(agent_id).await {
                Ok(v) => {
                    let pre = AgentBearPrefill::from_agent_json(&v);
                    NewBearForm {
                        slug: pre.suggested_slug,
                        name: pre.name,
                        description: pre.description,
                        system_prompt: pre.system_prompt,
                        default_model: pre.default_model,
                        letta_agent_type: pre.letta_agent_type,
                        letta_tool_ids: pre.letta_tool_ids,
                        attach_letta_agent_id: agent_id.to_string(),
                    }
                }
                Err(e) => {
                    from_agent_error =
                        Some(format!("Could not load Letta agent {agent_id:?}: {e}"));
                    NewBearForm::default()
                }
            }
        }
    } else {
        NewBearForm::default()
    };

    let page = bear_new_form_context(&state, &form).await;
    web::render_template(
        &state,
        "admin/bears/new.html",
        auth_session,
        context! {
            form,
            from_agent_error,
            ..page
        },
    )
    .await
}

#[derive(Serialize)]
struct UnlinkedLettaAgentRow {
    display_name: String,
    agent_id: String,
    new_bear_href: String,
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
                    let new_bear_href = format!(
                        "/admin/bears/new?from_letta_agent={}",
                        urlencoding::encode(&id).into_owned()
                    );
                    unlinked_rows.push(UnlinkedLettaAgentRow {
                        display_name,
                        agent_id: id,
                        new_bear_href,
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

    let attach_trim = form.attach_letta_agent_id.trim();
    if !attach_trim.is_empty() {
        if !state.letta.is_enabled() {
            validation_errors.add(
                "attach_letta_agent_id",
                ValidationError::new(
                    "Letta is not configured; remove attach mode or configure Letta first.",
                ),
            );
        } else {
            match state.letta.fetch_agent(attach_trim).await {
                Ok(_) => {}
                Err(e) => {
                    validation_errors.add(
                        "attach_letta_agent_id",
                        ValidationError::new("letta_agent_fetch_failed").with_message(Cow::Owned(
                            format!(
                                "Could not load this Letta agent (it may have been deleted): {e}"
                            ),
                        )),
                    );
                }
            }
            if bears_db::bear_exists_for_letta_agent_id(state.sqlx_pool(), attach_trim).await? {
                validation_errors.add(
                    "attach_letta_agent_id",
                    ValidationError::new("Another bear already uses this Letta agent id."),
                );
            }
        }
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
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            form.system_prompt.trim(),
            default_model_opt,
            None::<Json<serde_json::Value>>,
            letta_agent_type_db.as_deref(),
            Json(letta_tool_ids.clone()),
        )
        .await?;

        if attach_trim.is_empty() {
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
                let bear = bears_db::get_bear(state.sqlx_pool(), id)
                    .await?
                    .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
                if bear.letta_agent_id.is_some() {
                    if let Err(e) = sync::sync_bear_to_letta(
                        state.sqlx_pool(),
                        state.letta.as_ref(),
                        state.bifrost.as_ref(),
                        id,
                    )
                    .await
                    {
                        tracing::warn!(%id, "Letta sync after create failed: {e}");
                        let page = bear_new_form_context(&state, &form).await;
                        return web::render_template(
                            &state,
                            "admin/bears/new.html",
                            auth_session,
                            context! {
                                form => form,
                                letta_sync_error => format!(
                                    "Bear was saved and provisioned in Den, but Letta rejected syncing fields: {e}"
                                ),
                                ..page
                            },
                        )
                        .await;
                    }
                }
            }
        } else {
            if bears_db::bear_exists_for_letta_agent_id(state.sqlx_pool(), attach_trim).await? {
                let page = bear_new_form_context(&state, &form).await;
                return web::render_template(
                    &state,
                    "admin/bears/new.html",
                    auth_session,
                    context! {
                        form => form,
                        provision_error => "Another bear was linked to this Letta agent while you were editing; the new bear row was created without an agent. Delete it or pick a different agent.".to_string(),
                        ..page
                    },
                )
                .await;
            }
            bears_db::set_letta_agent_id(state.sqlx_pool(), id, attach_trim).await?;
            if let Err(e) = sync::sync_bear_to_letta(
                state.sqlx_pool(),
                state.letta.as_ref(),
                state.bifrost.as_ref(),
                id,
            )
            .await
            {
                tracing::warn!(%id, "Letta sync after attach failed: {e}");
                let page = bear_new_form_context(&state, &form).await;
                return web::render_template(
                    &state,
                    "admin/bears/new.html",
                    auth_session,
                    context! {
                        form => form,
                        letta_sync_error => format!(
                            "Bear was saved and linked to Letta agent {attach_trim}, but Letta rejected syncing fields: {e}"
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
            form.slug.trim(),
            form.name.trim(),
            form.description.trim(),
            form.system_prompt.trim(),
            default_model_opt,
            None::<Json<serde_json::Value>>,
            letta_agent_type_db.as_deref(),
            Json(letta_tool_ids.clone()),
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
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;

    let letta_retry_message = if !state.letta.is_enabled() {
        "Letta is not configured (set LETTA_BASE_URL).".to_string()
    } else if bear.letta_agent_id.is_some() {
        format!(
            "This bear already has a Letta agent ({}). No new agent was created.",
            bear.letta_agent_id.as_deref().unwrap_or("")
        )
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
                let bear2 = bears_db::get_bear(state.sqlx_pool(), id)
                    .await?
                    .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
                if let Some(agent) = bear2.letta_agent_id.as_deref() {
                    let mut msg = format!("Letta agent provisioned: {agent}.");
                    if let Err(e) = sync::sync_bear_to_letta(
                        state.sqlx_pool(),
                        state.letta.as_ref(),
                        state.bifrost.as_ref(),
                        id,
                    )
                    .await
                    {
                        msg.push_str(&format!(
                            " Den saved the bear but a follow-up sync to Letta failed: {e}"
                        ));
                    }
                    msg
                } else {
                    "Provisioning finished but letta_agent_id is still unset.".to_string()
                }
            }
            Err(e) => format!("Letta provisioning failed: {e}"),
        }
    };

    bear_detail_response(&state, auth_session, id, Some(letta_retry_message)).await
}
