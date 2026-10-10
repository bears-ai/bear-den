// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
// TODO(den-web extraction): superseded admin bear-detail handlers below are dead;
// remove during the den-web move rather than blind-deleting now.
#![allow(dead_code)]
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
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
    core::{user::db as user_db, web_policy},
    errors::CustomError,
    web::{self, AppState},
};
use den_core::DenError;
use den_service::bears::{db as bears_db, db::BearParams, provision};

use crate::web::bear::create_support::{
    admin_bear_edit_page_context, admin_bear_new_form_context, canonical_default_model_handle,
    AdminBearPromptForm, AdminNewBearForm, NewBearForm,
};

async fn redirect_bear_slug(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    Ok(Redirect::permanent(&format!(
        "/bear/{}/overview",
        bear.slug
    )))
}

async fn redirect_bear_slug_path(
    Path((id, rest)): Path<(Uuid, String)>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let path = rest.trim_start_matches('/');
    let target = match path {
        "" => format!("/bear/{}/overview", bear.slug),
        "edit" => format!("/bear/{}/edit/overview", bear.slug),
        "edit/prompt" => format!("/bear/{}/edit/prompt", bear.slug),
        "access" => format!("/bear/{}/access", bear.slug),
        "persona" => format!("/bear/{}/persona", bear.slug),
        "profiles" => format!("/bear/{}/profiles", bear.slug),
        "memory" => format!("/bear/{}/memory", bear.slug),
        p if p.starts_with("memory/records/") => {
            format!("/bear/{}/{}", bear.slug, p)
        }
        "conversations" => format!("/bear/{}/conversations", bear.slug),
        p if p.starts_with("conversations/") => format!("/bear/{}/{}", bear.slug, p),
        "context" => format!("/bear/{}/context", bear.slug),
        "policy" => format!("/bear/{}/policy", bear.slug),
        "advanced" => format!("/bear/{}/advanced", bear.slug),
        other => format!(
            "/bear/{}/overview?legacy={}",
            bear.slug,
            urlencoding::encode(other)
        ),
    };
    Ok(Redirect::permanent(&target))
}

pub fn router() -> Router<AppState> {
    Router::new().route_with_tsr("/bears/", get(list_view))
}

#[cfg(test)]
pub(crate) fn model_setup_test_router() -> Router<AppState> {
    Router::new()
        .route("/test-admin/bears/new", get(new_view).post(new_action))
        .route(
            "/test-admin/bears/{id}/edit",
            get(edit_view).post(edit_action),
        )
}

#[derive(Debug, Serialize)]
pub(crate) struct BearWebSourceRow {
    id: Uuid,
    scope_kind: String,
    scope_value: String,
    label: Option<String>,
    policy: String,
    priority: i32,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct BearWebApprovalRow {
    id: Uuid,
    scope_kind: String,
    scope_value: String,
    source: String,
    approved_by_user_label: Option<String>,
    created_at: String,
    expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BearWebFetchRow {
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
pub(crate) struct AddWebSourceForm {
    pub(crate) scope_kind: String,
    pub(crate) scope_value: String,
    pub(crate) policy: String,
    #[serde(default)]
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AddWebApprovalForm {
    pub(crate) scope_kind: String,
    pub(crate) scope_value: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct BearPlanModeRow {
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
pub(crate) struct BearMemberAdminRow {
    pub(crate) user_id: i32,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) role: Option<String>,
    pub(crate) role_label: String,
}

pub(crate) fn membership_role_label(role: Option<&str>) -> String {
    match role.map(str::trim).filter(|s| !s.is_empty()) {
        Some("admin") => "Admin — can manage bear settings and members".to_string(),
        Some("member") | None => "Member — can use the bear".to_string(),
        Some(other) => format!("Custom ({other})"),
    }
}

pub(crate) async fn bear_web_sources(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebSourceRow>, CustomError> {
    let rows = sqlx::query!(
        r"
        SELECT id, scope_kind, scope_value, label, policy, priority, created_at
        FROM bear_web_sources
        WHERE bear_id = $1
        ORDER BY policy ASC, priority DESC, scope_kind ASC, scope_value ASC
        ",
        bear_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| BearWebSourceRow {
            id: row.id,
            scope_kind: row.scope_kind,
            scope_value: row.scope_value,
            label: row.label,
            policy: row.policy,
            priority: row.priority,
            created_at: row.created_at.to_string(),
        })
        .collect())
}

pub(crate) async fn bear_web_approvals(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebApprovalRow>, CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT a.id,
               a.scope_kind,
               a.scope_value,
               a.source,
               u.username AS "username?",
               NULLIF(u.display_name, '') AS display_name,
               a.created_at,
               a.expires_at
        FROM bear_web_approvals a
        LEFT JOIN users u ON u.id = a.approved_by_user_id
        WHERE a.bear_id = $1 AND a.revoked_at IS NULL
        ORDER BY a.created_at DESC
        "#,
        bear_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| BearWebApprovalRow {
            id: row.id,
            scope_kind: row.scope_kind,
            scope_value: row.scope_value,
            source: row.source,
            approved_by_user_label: match (row.display_name, row.username) {
                (Some(display_name), Some(username)) => {
                    Some(format!("{display_name} (@{username})"))
                }
                (Some(display_name), None) => Some(display_name),
                (None, Some(username)) => Some(format!("@{username}")),
                (None, None) => None,
            },
            created_at: row.created_at.to_string(),
            expires_at: row.expires_at.map(|t| t.to_string()),
        })
        .collect())
}

pub(crate) async fn bear_web_fetches(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearWebFetchRow>, CustomError> {
    let rows = sqlx::query!(
        r"
        SELECT url, final_url, host, execution_location, approval_kind, http_status, content_type, bytes, fetched_at
        FROM bear_web_fetches
        WHERE bear_id = $1
        ORDER BY fetched_at DESC
        LIMIT 25
        ",
        bear_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| BearWebFetchRow {
            url: row.url,
            final_url: row.final_url,
            host: row.host,
            execution_location: row.execution_location,
            approval_kind: row.approval_kind,
            http_status: row.http_status,
            content_type: row.content_type,
            bytes: row.bytes,
            fetched_at: row.fetched_at.to_string(),
        })
        .collect())
}

pub(crate) async fn bear_plan_mode_rows(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearPlanModeRow>, CustomError> {
    let rows = sqlx::query!(
        r"
        SELECT p.id, p.user_id, u.username, p.client_session_id, p.state, p.reason,
               p.plan_artifact_path, p.plan_title, p.created_at, p.updated_at
        FROM client_plan_mode_sessions p
        LEFT JOIN users u ON u.id = p.user_id
        WHERE p.bear_id = $1
        ORDER BY p.updated_at DESC
        LIMIT 10
        ",
        bear_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| BearPlanModeRow {
            id: row.id,
            user_id: row.user_id,
            username: Some(row.username),
            acp_session_id: row.client_session_id,
            state: row.state,
            reason: row.reason,
            plan_artifact_path: row.plan_artifact_path,
            plan_title: row.plan_title,
            created_at: row.created_at.to_string(),
            updated_at: row.updated_at.to_string(),
        })
        .collect())
}

async fn list_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bears = bears_db::list_bears(state.sqlx_pool()).await?;
    web::render_template(
        &state,
        "admin/bears/list.html",
        auth_session,
        context! { bears, native_runtime => true },
    )
    .await
}

async fn new_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(_query): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, CustomError> {
    let form = AdminNewBearForm::default();
    let users = user_db::get_users(state.sqlx_pool()).await?;
    let page = admin_bear_new_form_context(&state, &form.bear).await;
    web::render_template(
        &state,
        "admin/bears/new.html",
        auth_session,
        context! {
            form => form.bear,
            admin_form => form,
            users,
            ..page
        },
    )
    .await
}

pub async fn new_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(admin_form): Form<AdminNewBearForm>,
) -> Result<Response, CustomError> {
    let form = admin_form.bear.clone();

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
        let id = match crate::bear::model_setup::create_with_validated_model(&state, &form).await {
            Ok(id) => id,
            Err(failure) => {
                let users = user_db::get_users(state.sqlx_pool()).await?;
                let page = admin_bear_new_form_context(&state, &form).await;
                let mut response = web::render_template(
                    &state,
                    "admin/bears/new.html",
                    auth_session,
                    context! {
                        form, admin_form, users, provision_error => failure.message,
                        saved_bear_slug => failure.saved_bear.map(|_| form.slug.trim()),
                        ..page
                    },
                )
                .await?;
                *response.status_mut() = failure.status;
                return Ok(response);
            }
        };

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await
        {
            tracing::warn!(%id, "Bear provision failed: {e}");
            let users = user_db::get_users(state.sqlx_pool()).await?;
            let page = admin_bear_new_form_context(&state, &form).await;
            return web::render_template(
                &state,
                "admin/bears/new.html",
                auth_session,
                context! {
                    form => form,
                    admin_form => admin_form,
                    users,
                    provision_error => "The Bear and verified model were saved, but initialization failed. Inspect the saved Bear's diagnostics; do not create it again.",
                    saved_bear_slug => form.slug.trim(),
                    ..page
                },
            )
            .await;
        }

        if let Ok(user_id) = admin_form.grant_user_id.trim().parse::<i32>() {
            if user_id > 0
                && user_db::get_user_by_id(state.sqlx_pool(), user_id)
                    .await?
                    .is_some()
            {
                let role = admin_form.grant_role.trim();
                let role_opt = match role {
                    "" | "member" => Some(bears_db::BEAR_ROLE_MEMBER),
                    "admin" => Some(bears_db::BEAR_ROLE_ADMIN),
                    other => Some(other),
                };
                bears_db::grant_membership(state.sqlx_pool(), user_id, id, role_opt).await?;
            }
        }

        Ok(Redirect::to(&format!("/bear/{}/overview", form.slug.trim())).into_response())
    } else {
        let users = user_db::get_users(state.sqlx_pool()).await?;
        let page = admin_bear_new_form_context(&state, &form).await;
        web::render_template(
            &state,
            "admin/bears/new.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                admin_form => admin_form,
                users,
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
    let page = admin_bear_edit_page_context(&state, &bear, &form).await;
    web::render_template(
        &state,
        "admin/bears/edit.html",
        auth_session,
        context! {
            bear,
            form,
            context_profile_enabled => bear.context_profile.is_some(),
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
            crate::model_availability::validate_model(&state, id.into(), model).await
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

    if bears_db::bear_slug_exists_excluding(state.sqlx_pool(), form.slug.trim(), id).await? {
        validation_errors.add(
            "slug",
            ValidationError::new("A bear with this slug already exists."),
        );
    }

    if validation_errors.is_empty() {
        let system_prompt = if bear.context_profile.is_some() {
            bear.system_prompt.as_str()
        } else {
            form.system_prompt.trim()
        };
        bears_db::update_bear(
            state.sqlx_pool(),
            id,
            BearParams {
                slug: form.slug.trim(),
                name: form.name.trim(),
                description: form.description.trim(),
                system_prompt,
                default_model: default_model_opt.as_deref(),
                tools_enabled: None::<Json<serde_json::Value>>,
                context_profile: bear.context_profile.clone(),
            },
        )
        .await?;

        if let Err(e) =
            provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await
        {
            tracing::warn!(%id, "Native profile refresh after bear edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            let page = admin_bear_edit_page_context(&state, &bear, &form).await;
            return web::render_template(
                &state,
                "admin/bears/edit.html",
                auth_session,
                context! {
                    errors => ValidationErrors::new(),
                    form => form,
                    bear,
                    provision_error => e.to_string(),
                    ..page
                },
            )
            .await;
        }

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        let page = admin_bear_edit_page_context(&state, &bear, &form).await;
        let mut response = web::render_template(
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
        .await?;
        if let Some(status) = availability_status {
            *response.status_mut() = status;
        }
        Ok(response)
    }
}

#[derive(Debug, Deserialize)]
struct GrantMemberForm {
    user_id: i32,
    role: String,
}

async fn edit_prompt_view(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let (form, context_profile_enabled) = AdminBearPromptForm::from_bear(&bear)?;
    web::render_template(
        &state,
        "admin/bears/edit_prompt.html",
        auth_session,
        context! {
            bear,
            form,
            context_profile_enabled,
            native_runtime => true,
        },
    )
    .await
}

async fn edit_prompt_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<AdminBearPromptForm>,
) -> Result<Response, CustomError> {
    let bear = bears_db::get_bear(state.sqlx_pool(), id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    let context_profile_enabled = bear.context_profile.is_some();

    let mut validation_errors = ValidationErrors::new();
    if let Err(e) = form.validate() {
        validation_errors = e;
    }

    let context_profile = match form.context_profile_for_bear(&bear, context_profile_enabled) {
        Ok(profile) => profile,
        Err(CustomError::ValidationError(_)) => {
            validation_errors.add(
                "system_prompt",
                ValidationError::new("Check role prompts and shared context fields."),
            );
            None
        }
        Err(err) => return Err(err),
    };

    let system_prompt = match form.resolved_system_prompt(bear.name.trim(), &context_profile) {
        Ok(prompt) => prompt,
        Err(CustomError::ValidationError(_)) => {
            validation_errors.add(
                "system_prompt",
                ValidationError::new("System prompt is required."),
            );
            String::new()
        }
        Err(err) => return Err(err),
    };

    if validation_errors.is_empty() {
        bears_db::update_bear(
            state.sqlx_pool(),
            id,
            BearParams {
                slug: bear.slug.as_str(),
                name: bear.name.as_str(),
                description: bear.description.as_str(),
                system_prompt: system_prompt.trim(),
                default_model: bear.default_model.as_deref(),
                tools_enabled: None::<Json<serde_json::Value>>,
                context_profile: context_profile.clone(),
            },
        )
        .await?;

        provision::initialize_bear_native(state.sqlx_pool(), &state.memory_stores, id).await?;

        Ok(Redirect::to(&format!("/admin/bears/{id}")).into_response())
    } else {
        web::render_template(
            &state,
            "admin/bears/edit_prompt.html",
            auth_session,
            context! {
                errors => validation_errors,
                form => form,
                bear,
                context_profile_enabled,
                native_runtime => true,
            },
        )
        .await
    }
}

async fn grant_member_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    _auth_session: AuthSession,
    Form(form): Form<GrantMemberForm>,
) -> Result<Response, CustomError> {
    if bears_db::get_bear(state.sqlx_pool(), id).await?.is_none() {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    if user_db::get_user_by_id(state.sqlx_pool(), form.user_id)
        .await?
        .is_none()
    {
        return Ok(Redirect::to(&format!(
            "/admin/bears/{id}/access?message={}",
            urlencoding::encode("User not found.")
        ))
        .into_response());
    }
    let role = form.role.trim();
    let role_opt = match role {
        "" | "member" => Some(bears_db::BEAR_ROLE_MEMBER),
        "admin" => Some(bears_db::BEAR_ROLE_ADMIN),
        other => Some(other),
    };
    bears_db::grant_membership(state.sqlx_pool(), form.user_id, id, role_opt).await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}/access?message={}",
        urlencoding::encode("Access granted.")
    ))
    .into_response())
}

async fn revoke_member_action(
    Path((id, user_id)): Path<(Uuid, i32)>,
    State(state): State<AppState>,
    _auth_session: AuthSession,
) -> Result<Response, CustomError> {
    match bears_db::revoke_membership(state.sqlx_pool(), user_id, id).await {
        Ok(()) => Ok(Redirect::to(&format!(
            "/admin/bears/{id}/access?message={}",
            urlencoding::encode("Access removed.")
        ))
        .into_response()),
        Err(DenError::NotFound(_)) => Ok(Redirect::to(&format!(
            "/admin/bears/{id}/access?message={}",
            urlencoding::encode("Membership not found.")
        ))
        .into_response()),
        Err(err) => Err(err.into()),
    }
}

async fn add_web_source_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    _auth_session: AuthSession,
    Form(form): Form<AddWebSourceForm>,
) -> Result<Response, CustomError> {
    let scope_kind = form.scope_kind.trim();
    let policy = form.policy.trim();
    if !matches!(scope_kind, "host" | "url")
        || !matches!(policy, "preferred" | "allowed" | "blocked")
    {
        return Ok(Redirect::to(&format!(
            "/admin/bears/{id}/policy?message={}",
            urlencoding::encode("Invalid web source policy form.")
        ))
        .into_response());
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return Ok(Redirect::to(&format!(
                "/admin/bears/{id}/policy?message={}",
                urlencoding::encode(&err.to_string())
            ))
            .into_response());
        }
    };
    sqlx::query!(
        r"
        INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, label, policy, priority)
        VALUES ($1, $2, $3, NULLIF($4, ''), $5, $6)
        ON CONFLICT (bear_id, scope_kind, scope_value)
        DO UPDATE SET label = EXCLUDED.label,
                      policy = EXCLUDED.policy,
                      priority = EXCLUDED.priority,
                      updated_at = now()
        ",
        id,
        scope_kind,
        scope_value,
        form.label.trim(),
        policy,
        form.priority.unwrap_or(0),
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}/policy?message={}",
        urlencoding::encode("Web source saved.")
    ))
    .into_response())
}

async fn delete_web_source_action(
    Path((id, source_id)): Path<(Uuid, Uuid)>,
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    sqlx::query!(
        "DELETE FROM bear_web_sources WHERE bear_id = $1 AND id = $2",
        id,
        source_id
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}/policy?message={}",
        urlencoding::encode("Web source deleted.")
    ))
    .into_response())
}

async fn add_web_approval_action(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    _auth_session: AuthSession,
    Form(form): Form<AddWebApprovalForm>,
) -> Result<Response, CustomError> {
    let scope_kind = form.scope_kind.trim();
    if !matches!(scope_kind, "host" | "url") {
        return Ok(Redirect::to(&format!(
            "/admin/bears/{id}/policy?message={}",
            urlencoding::encode("Invalid web approval scope.")
        ))
        .into_response());
    }
    let scope_value = match web_policy::normalize_web_scope_value(scope_kind, &form.scope_value) {
        Ok(scope_value) => scope_value,
        Err(err) => {
            return Ok(Redirect::to(&format!(
                "/admin/bears/{id}/policy?message={}",
                urlencoding::encode(&err.to_string())
            ))
            .into_response());
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
        "/admin/bears/{id}/policy?message={}",
        urlencoding::encode("Web approval added.")
    ))
    .into_response())
}

async fn revoke_web_approval_action(
    Path((id, approval_id)): Path<(Uuid, Uuid)>,
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    sqlx::query!(
        "UPDATE bear_web_approvals SET revoked_at = now() WHERE bear_id = $1 AND id = $2",
        id,
        approval_id
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to(&format!(
        "/admin/bears/{id}/policy?message={}",
        urlencoding::encode("Web approval revoked.")
    ))
    .into_response())
}
