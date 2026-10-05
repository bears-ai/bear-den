// ROUTES: When modifying routes in this file, update /src/ROUTES.md.
//! Managed work surfaces UI: den-level entities backing sandbox roots.
//!
//! Any logged-in user can create a surface (becoming its owner); owners
//! grant other users management; surfaces are assigned to bears (full
//! access — any member of an assigned bear can run jobs on it). Site admins
//! manage everything. Credentials are write-only here: stored encrypted in
//! Postgres, pushed to the sandbox provider, never rendered back.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth_backend::AuthSession,
    core::user::db as user_db,
    errors::CustomError,
    web::{self, AppState},
};
use den_docket::{DocketJobUpdate, DocketService, PgDocketService};
use den_sandbox::SandboxClient;
use den_service::bears::db as bears_db;
use den_service::work_surfaces::{
    self, NewWorkSurface, WorkSurfaceRow, WorkSurfaceUpdate, SURFACE_ROLE_MANAGER,
    SURFACE_ROLE_OWNER,
};

use super::{entity_ref, member_bears, require_user, resolve_work_surface_prefix, route_id};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/work/surfaces", get(index))
        .route("/work/surfaces/new", get(new_form).post(create))
        .route("/work/surfaces/{surface_id}", get(detail))
        .route("/work/surfaces/{surface_id}/diff", get(diff_detail))
        .route("/work/surfaces/{surface_id}/update", post(update))
        .route(
            "/work/surfaces/{surface_id}/credential",
            post(set_credential),
        )
        .route(
            "/work/surfaces/{surface_id}/credential/clear",
            post(clear_credential),
        )
        .route(
            "/work/surfaces/{surface_id}/github-app",
            post(set_github_app),
        )
        .route(
            "/work/surfaces/{surface_id}/github-app/clear",
            post(clear_github_app),
        )
        .route(
            "/work/surfaces/{surface_id}/managers/grant",
            post(grant_manager),
        )
        .route(
            "/work/surfaces/{surface_id}/managers/revoke",
            post(revoke_manager),
        )
        .route(
            "/work/surfaces/{surface_id}/bears/assign",
            post(assign_bear),
        )
        .route(
            "/work/surfaces/{surface_id}/bears/unassign",
            post(unassign_bear),
        )
        .route("/work/surfaces/{surface_id}/delete", post(delete))
        .route("/work/surfaces/{surface_id}/sync", post(sync_now))
}

fn viewer_is_admin(auth_session: &AuthSession) -> bool {
    auth_session.user.as_ref().is_some_and(|user| user.is_admin)
}

/// Manage-scope guard: manager/owner grant or site admin; anything else is a
/// 404 (surface existence is not disclosed to non-managers).
async fn load_managed_surface(
    state: &AppState,
    auth_session: &AuthSession,
    surface_id: Uuid,
) -> Result<WorkSurfaceRow, CustomError> {
    let user_id = require_user(auth_session)?;
    let surface = work_surfaces::surface_by_id(state.sqlx_pool(), surface_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("work surface not found".to_string()))?;
    if viewer_is_admin(auth_session)
        || work_surfaces::user_may_manage_surface(state.sqlx_pool(), user_id, surface_id).await?
    {
        Ok(surface)
    } else {
        Err(CustomError::NotFound("work surface not found".to_string()))
    }
}

/// Best-effort push of the managed config to the sandbox provider after a
/// mutation. Returns a note to append to the redirect message on failure —
/// the dispatch worker's periodic reconcile heals missed pushes.
pub(crate) async fn push_surfaces_best_effort(state: &AppState) -> Option<String> {
    let url = state
        .config
        .sandbox_server_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())?;
    let result = async {
        let managed = work_surfaces::build_managed_config(
            state.sqlx_pool(),
            &state.config.den_secret_encryption_key,
        )
        .await
        .map_err(|err| err.to_string())?;
        SandboxClient::new(url, &state.config.sandbox_server_token)
            .put_managed_config(&managed)
            .await
            .map_err(|err| err.to_string())
    }
    .await;
    match result {
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(error = %err, "work surfaces: provider sync push failed");
            Some(" (provider sync failed — retries within 5 minutes)".to_string())
        }
    }
}

async fn prepare_surface(state: &AppState, name: &str) -> Result<String, String> {
    if let Some(note) = push_surfaces_best_effort(state).await {
        return Err(note);
    }
    let url = state
        .config
        .sandbox_server_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "No sandbox provider configured.".to_string())?;
    let result = SandboxClient::new(url, &state.config.sandbox_server_token)
        .sync_root(name)
        .await
        .map_err(|err| format!("Provider root test failed: {err}"))?;
    if result.synced {
        Ok(match result.head {
            Some(head) => format!("Surface is ready at commit {head}."),
            None => "Surface is ready.".to_string(),
        })
    } else {
        Err(result
            .error
            .unwrap_or_else(|| "Provider could not prepare the surface.".to_string()))
    }
}

fn surface_redirect(surface_id: Uuid, message: &str, sync_note: Option<String>) -> Response {
    let full = format!("{message}{}", sync_note.unwrap_or_default());
    Redirect::to(&format!(
        "/work/surfaces/{}?message={}",
        route_id(surface_id),
        urlencoding::encode(&full)
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageQuery {
    #[serde(default)]
    message: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<MessageQuery>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    let managed = if viewer_is_admin(&auth_session) {
        work_surfaces::list_all_surfaces(state.sqlx_pool()).await?
    } else {
        work_surfaces::list_surfaces_managed_by(state.sqlx_pool(), user_id).await?
    };

    let bears = member_bears(&state, user_id).await?;
    let bear_ids: Vec<Uuid> = bears.keys().copied().collect();
    let available: Vec<serde_json::Value> =
        work_surfaces::list_surfaces_for_bears(state.sqlx_pool(), &bear_ids)
            .await?
            .into_iter()
            .map(|surface| {
                serde_json::json!({
                    "id": route_id(surface.id),
                    "reference": entity_ref(surface.id, "Repository", &surface.name, None),
                    "name": surface.name,
                    "description": surface.description,
                    "bear_slug": bears.get(&surface.bear_id).cloned().unwrap_or_default(),
                })
            })
            .collect();

    let managed: Vec<serde_json::Value> = managed
        .into_iter()
        .map(|surface| {
            serde_json::json!({
                "id": route_id(surface.id),
                "reference": entity_ref(surface.id, "Repository", &surface.name, None),
                "name": surface.name,
                "description": surface.description,
                "upstream_url": surface.upstream_url,
                "default_ref": surface.default_ref,
                "credential_kind": surface.credential_kind,
            })
        })
        .collect();

    web::render_template(
        &state,
        "work/surfaces.html",
        auth_session,
        context! {
            title => "Repositories",
            managed => managed,
            available => available,
            message => query.message,
        },
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
struct NewSurfaceQuery {
    #[serde(default)]
    bear_id: Option<Uuid>,
    #[serde(default)]
    return_job_id: Option<Uuid>,
}

async fn new_form(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<NewSurfaceQuery>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    if let Some(bear_id) = query.bear_id {
        let bears = member_bears(&state, user_id).await?;
        if !bears.contains_key(&bear_id) {
            return Err(CustomError::NotFound("bear not found".to_string()));
        }
    }
    let images = work_surfaces::list_catalog_images(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "work/surface_new.html",
        auth_session,
        context! {
            title => "Connect repository",
            images => images,
            bear_id => query.bear_id.map(|id| id.to_string()),
            return_job_id => query.return_job_id.map(|id| id.to_string()),
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct NewSurfaceForm {
    name: String,
    #[serde(default)]
    description: String,
    upstream_url: String,
    #[serde(default)]
    default_ref: String,
    #[serde(default)]
    default_image: String,
    /// One exact hostname per line. The service owns validation and
    /// normalization so this form deliberately does not duplicate it.
    #[serde(default)]
    allowed_outbound_hosts: String,
    #[serde(default)]
    credential_kind: String,
    #[serde(default)]
    credential_value: String,
    #[serde(default)]
    bear_id: Option<Uuid>,
    #[serde(default)]
    return_job_id: Option<Uuid>,
}

fn clean(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn outbound_hosts_from_form(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .collect()
}

fn credential_from_form(kind: &str, value: &str) -> Result<Option<(String, String)>, CustomError> {
    match (clean(kind), clean(value)) {
        (None, None) | (Some(_), None) => Ok(None),
        (None, Some(_)) => Err(CustomError::ValidationError(
            "select a credential kind for the provided credential value".to_string(),
        )),
        (Some(kind), Some(value)) => Ok(Some((kind, value))),
    }
}

async fn create(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewSurfaceForm>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    if let Some(bear_id) = form.bear_id {
        if !bears.contains_key(&bear_id) {
            return Err(CustomError::NotFound("bear not found".to_string()));
        }
    }
    if let Some(job_id) = form.return_job_id {
        let bear_id = form.bear_id.ok_or_else(|| {
            CustomError::ValidationError("return job requires a selected bear".to_string())
        })?;
        let job_bear_id: Option<Uuid> =
            sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
                .fetch_optional(state.sqlx_pool())
                .await
                .map_err(den_core::DenError::from)?;
        if job_bear_id != Some(bear_id) {
            return Err(CustomError::NotFound("job not found".to_string()));
        }
    }
    let credential = credential_from_form(&form.credential_kind, &form.credential_value)?;
    let surface = work_surfaces::create_surface(
        state.sqlx_pool(),
        user_id,
        NewWorkSurface {
            name: form.name.trim().to_string(),
            description: clean(&form.description),
            upstream_url: form.upstream_url.trim().to_string(),
            default_ref: clean(&form.default_ref).unwrap_or_else(|| "main".to_string()),
            default_image: clean(&form.default_image),
            allowed_outbound_hosts: outbound_hosts_from_form(&form.allowed_outbound_hosts),
            credential,
        },
        &state.config.den_secret_encryption_key,
    )
    .await?;
    if let Some(bear_id) = form.bear_id {
        work_surfaces::assign_bear(state.sqlx_pool(), surface.id, bear_id, user_id).await?;
        if let Some(job_id) = form.return_job_id {
            PgDocketService::from_pool(state.sqlx_pool())
                .update_job(DocketJobUpdate {
                    bear_id,
                    job_id,
                    actor_role: den_service::bears::RuntimeContextLabel::ArmatureConversation,
                    actor_user_id: Some(user_id),
                    actor_agent_id: None,
                    goal: None,
                    work_surface_id: Some(Some(surface.id)),
                    commit_policy: None,
                    work_branch: None,
                    status: None,
                    visibility: None,
                })
                .await?;
        }
    }
    let prepared = prepare_surface(&state, &surface.name).await;
    if let (Some(job_id), Ok(_)) = (form.return_job_id, &prepared) {
        return Ok(Redirect::to(&format!("/work/jobs/{job_id}")).into_response());
    }
    let message = match prepared {
        Ok(message) => format!("Work surface created. {message}"),
        Err(error) => format!("Work surface created, but is not ready: {error}"),
    };
    Ok(surface_redirect(surface.id, &message, None))
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    head: Option<String>,
}

async fn diff_detail(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    let surface = load_managed_surface(&state, &auth_session, surface_id).await?;
    let base = query.base.unwrap_or_else(|| surface.default_ref.clone());
    let head = query.head.unwrap_or_else(|| "origin/HEAD".to_string());
    let (comparison, unavailable) = match state
        .config
        .sandbox_server_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
    {
        Some(url) => match SandboxClient::new(url.trim(), &state.config.sandbox_server_token)
            .compare_root(&surface.name, &base, &head)
            .await
        {
            Ok(value) => (Some(value), None),
            Err(err) => (None, Some(err.to_string())),
        },
        None => (None, Some("No sandbox provider configured.".to_string())),
    };
    web::render_template(&state, "work/diff.html", auth_session, context! {
        title => "Work surface diff", surface_id => route_id(surface.id), surface_name => surface.name,
        base => base, head => head, comparison => comparison, unavailable => unavailable,
    }).await
}

async fn detail(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Query(query): Query<MessageQuery>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    let surface = load_managed_surface(&state, &auth_session, surface_id).await?;
    let managers = work_surfaces::list_managers(state.sqlx_pool(), surface_id).await?;
    let assigned = work_surfaces::list_assigned_bears(state.sqlx_pool(), surface_id).await?;
    let images = work_surfaces::list_catalog_images(state.sqlx_pool()).await?;
    let provider_root_status = match state.config.sandbox_server_url.as_deref() {
        Some(url) if !url.trim().is_empty() => {
            SandboxClient::new(url.trim(), &state.config.sandbox_server_token)
                .health()
                .await
                .ok()
                .and_then(|health| {
                    health
                        .roots
                        .into_iter()
                        .find(|root| root.name == surface.name)
                })
        }
        _ => None,
    };
    let provider_root_inspection = match state.config.sandbox_server_url.as_deref() {
        Some(url) if !url.trim().is_empty() => {
            SandboxClient::new(url.trim(), &state.config.sandbox_server_token)
                .inspect_root(&surface.name)
                .await
                .ok()
        }
        _ => None,
    };

    // Bears the viewer can assign: their member bears (admins: all bears),
    // minus ones already assigned.
    let assigned_ids: std::collections::HashSet<Uuid> =
        assigned.iter().map(|bear| bear.bear_id).collect();
    let assignable: Vec<(String, String)> = if viewer_is_admin(&auth_session) {
        sqlx::query!("SELECT id, slug FROM bears ORDER BY slug")
            .fetch_all(state.sqlx_pool())
            .await
            .map_err(den_core::DenError::from)?
            .into_iter()
            .filter(|row| !assigned_ids.contains(&row.id))
            .map(|row| (row.id.to_string(), row.slug))
            .collect()
    } else {
        let mut bears: Vec<(String, String)> = member_bears(&state, user_id)
            .await?
            .into_iter()
            .filter(|(id, _)| !assigned_ids.contains(id))
            .map(|(id, slug)| (id.to_string(), slug))
            .collect();
        bears.sort_by(|a, b| a.1.cmp(&b.1));
        bears
    };

    let assigned: Vec<serde_json::Value> = assigned
        .iter()
        .map(|bear| {
            serde_json::json!({
                "bear_id": bear.bear_id.to_string(),
                "slug": bear.slug,
                "display_name": bear.display_name,
            })
        })
        .collect();

    web::render_template(
        &state,
        "work/surface.html",
        auth_session,
        context! {
            title => "Repository",
            surface_id => route_id(surface.id),
            surface_reference => entity_ref(surface.id, "Repository", &surface.name, None),
            name => surface.name,
            description => surface.description,
            upstream_url => surface.upstream_url,
            default_ref => surface.default_ref,
            default_image => surface.default_image,
            allowed_outbound_hosts => surface.allowed_outbound_hosts.join("\n"),
            credential_kind => surface.credential_kind,
            github_app_installation_id => surface.github_app_installation_id,
            github_app_write_enabled => surface.github_app_write_enabled,
            managers => managers,
            assigned_bears => assigned,
            assignable_bears => assignable,
            images => images,
            provider_root_status => provider_root_status,
            provider_root_inspection => provider_root_inspection,
            message => query.message,
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct UpdateSurfaceForm {
    #[serde(default)]
    description: String,
    upstream_url: String,
    default_ref: String,
    #[serde(default)]
    default_image: String,
    #[serde(default)]
    allowed_outbound_hosts: String,
}

async fn update(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<UpdateSurfaceForm>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::update_surface(
        state.sqlx_pool(),
        surface_id,
        WorkSurfaceUpdate {
            description: Some(clean(&form.description)),
            upstream_url: clean(&form.upstream_url),
            default_ref: clean(&form.default_ref),
            default_image: Some(clean(&form.default_image)),
            allowed_outbound_hosts: Some(outbound_hosts_from_form(&form.allowed_outbound_hosts)),
            github_app_installation_id: None,
            github_app_write_enabled: None,
        },
    )
    .await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(surface_redirect(surface_id, "Surface updated.", sync_note))
}

#[derive(Debug, Deserialize)]
struct CredentialForm {
    credential_kind: String,
    credential_value: String,
}

async fn set_credential(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<CredentialForm>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    let Some((kind, value)) = credential_from_form(&form.credential_kind, &form.credential_value)?
    else {
        return Err(CustomError::ValidationError(
            "credential kind and value are both required".to_string(),
        ));
    };
    work_surfaces::set_credential(
        state.sqlx_pool(),
        surface_id,
        &kind,
        &value,
        &state.config.den_secret_encryption_key,
    )
    .await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(surface_redirect(
        surface_id,
        "Credential stored.",
        sync_note,
    ))
}

async fn clear_credential(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::clear_credential(state.sqlx_pool(), surface_id).await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(surface_redirect(
        surface_id,
        "Credential cleared.",
        sync_note,
    ))
}

fn validate_github_app_origin(upstream_url: &str) -> Result<(), CustomError> {
    let url = url::Url::parse(upstream_url).map_err(|_| {
        CustomError::ValidationError(
            "GitHub App authorization requires an HTTPS github.com origin".to_string(),
        )
    })?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CustomError::ValidationError(
            "GitHub App authorization requires an HTTPS github.com origin".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GitHubAppForm {
    installation_id: i64,
    #[serde(default)]
    write_enabled: bool,
}

async fn set_github_app(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<GitHubAppForm>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    let surface = load_managed_surface(&state, &auth_session, surface_id).await?;
    validate_github_app_origin(&surface.upstream_url)?;
    if form.installation_id <= 0 {
        return Err(CustomError::ValidationError(
            "GitHub App installation id must be positive".to_string(),
        ));
    }
    work_surfaces::update_surface(
        state.sqlx_pool(),
        surface_id,
        WorkSurfaceUpdate {
            github_app_installation_id: Some(Some(form.installation_id)),
            github_app_write_enabled: Some(form.write_enabled),
            ..Default::default()
        },
    )
    .await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(surface_redirect(
        surface_id,
        "GitHub App authorization updated.",
        sync_note,
    ))
}

async fn clear_github_app(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::update_surface(
        state.sqlx_pool(),
        surface_id,
        WorkSurfaceUpdate {
            github_app_installation_id: Some(None),
            github_app_write_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(surface_redirect(
        surface_id,
        "GitHub App authorization cleared.",
        sync_note,
    ))
}

#[derive(Debug, Deserialize)]
struct GrantManagerForm {
    username: String,
    #[serde(default)]
    role: String,
}

async fn grant_manager(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<GrantManagerForm>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    let username = form.username.trim();
    let Some(target) = user_db::get_user_by_username(state.sqlx_pool(), username).await? else {
        return Ok(surface_redirect(
            surface_id,
            &format!("No user named '{username}'."),
            None,
        ));
    };
    let role = match form.role.trim() {
        "" | SURFACE_ROLE_MANAGER => SURFACE_ROLE_MANAGER,
        SURFACE_ROLE_OWNER => SURFACE_ROLE_OWNER,
        other => {
            return Err(CustomError::ValidationError(format!(
                "unknown surface role '{other}'"
            )))
        }
    };
    work_surfaces::grant_manager(state.sqlx_pool(), surface_id, target.id, role, user_id).await?;
    Ok(surface_redirect(
        surface_id,
        &format!("{username} can now manage this surface."),
        None,
    ))
}

#[derive(Debug, Deserialize)]
struct RevokeManagerForm {
    user_id: i32,
}

async fn revoke_manager(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<RevokeManagerForm>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::revoke_manager(state.sqlx_pool(), surface_id, form.user_id).await?;
    Ok(surface_redirect(surface_id, "Manager removed.", None))
}

#[derive(Debug, Deserialize)]
struct BearForm {
    bear_id: Uuid,
}

async fn assign_bear(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<BearForm>,
) -> Result<Response, CustomError> {
    let user_id = require_user(&auth_session)?;
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    // Managers assign from their own member bears; site admins any bear.
    if !viewer_is_admin(&auth_session)
        && !bears_db::user_may_use_bear(state.sqlx_pool(), user_id, form.bear_id).await?
    {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    work_surfaces::assign_bear(state.sqlx_pool(), surface_id, form.bear_id, user_id).await?;
    Ok(surface_redirect(surface_id, "Bear assigned.", None))
}

async fn unassign_bear(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
    Form(form): Form<BearForm>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::unassign_bear(state.sqlx_pool(), surface_id, form.bear_id).await?;
    Ok(surface_redirect(surface_id, "Bear unassigned.", None))
}

async fn delete(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    let surface = load_managed_surface(&state, &auth_session, surface_id).await?;
    work_surfaces::delete_surface(state.sqlx_pool(), surface_id).await?;
    let sync_note = push_surfaces_best_effort(&state).await;
    Ok(Redirect::to(&format!(
        "/work/surfaces?message={}",
        urlencoding::encode(&format!(
            "Surface '{}' deleted.{}",
            surface.name,
            sync_note.unwrap_or_default()
        ))
    ))
    .into_response())
}

async fn sync_now(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(surface_ref): Path<String>,
) -> Result<Response, CustomError> {
    let surface_id = resolve_work_surface_prefix(state.sqlx_pool(), &surface_ref).await?;
    let surface = load_managed_surface(&state, &auth_session, surface_id).await?;
    let message = match prepare_surface(&state, &surface.name).await {
        Ok(message) => message,
        Err(error) => format!("Surface is not ready: {error}"),
    };
    Ok(surface_redirect(surface_id, &message, None))
}

#[cfg(test)]
mod tests {
    use super::validate_github_app_origin;

    #[test]
    fn github_app_requires_plain_github_https_origin() {
        assert!(validate_github_app_origin("https://github.com/owner/repo.git").is_ok());
        assert!(validate_github_app_origin("git@github.com:owner/repo.git").is_err());
        assert!(validate_github_app_origin("https://gitlab.com/owner/repo.git").is_err());
        assert!(validate_github_app_origin("https://token@github.com/owner/repo.git").is_err());
    }
}
