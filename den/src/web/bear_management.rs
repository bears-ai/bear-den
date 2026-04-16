//! Member-facing bear lifecycle: create bears (you become admin), details, membership, edit/delete for bear admins.
//! When changing routes, update `src/web/ROUTES.md`.

use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;
use minijinja::context;
use serde::Deserialize;
use sqlx::types::Json;
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

use crate::{
    auth_backend::AuthSession,
    core::{
        bears::{
            db as bears_db,
            db::{BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER, role_is_bear_admin},
            provision,
            sync,
            Bear,
        },
        letta::{AgentSummary, LettaAgentDiagnostics},
        user,
        user::db as user_db,
    },
    errors::CustomError,
    web::{
        bear_create_support::{
            bear_edit_page_context, bear_new_form_context, ensure_stored_model_in_options_for_handle,
            insert_new_bear_row, validate_default_model_for_letta, NewBearForm,
        },
        render_template, AppState,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bears/new", get(new_bear_get).post(new_bear_post))
        .route_with_tsr("/bear/{slug}/details", get(bear_details_get))
        .route_with_tsr(
            "/bear/{slug}/details/edit",
            get(bear_edit_get).post(bear_edit_post),
        )
        .route_with_tsr("/bear/{slug}/details/delete", post(bear_delete_post))
        .route_with_tsr(
            "/bear/{slug}/details/members/add",
            post(member_add_post),
        )
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
        .ok_or_else(|| CustomError::NotFound("Bear not found or you do not have access.".to_string()))
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
    Form(mut form): Form<NewBearForm>,
) -> Result<Response, CustomError> {
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    form.attach_letta_agent_id.clear();

    let letta_fetch = if state.letta.is_enabled() {
        Some(
            state
                .letta
                .list_llm_models()
                .await
                .map(|opts| {
                    let model_trim = form.default_model.trim();
                    let h = (!model_trim.is_empty()).then_some(model_trim);
                    ensure_stored_model_in_options_for_handle(h, opts)
                }),
        )
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

        bears_db::grant_membership(
            state.sqlx_pool(),
            user_id,
            id,
            Some(BEAR_ROLE_ADMIN),
        )
        .await?;

        if let Err(e) = provision::provision_bear_if_configured(
            state.sqlx_pool(),
            state.letta.as_ref(),
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
            let bear = bears_db::get_bear(state.sqlx_pool(), id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            if bear.letta_agent_id.is_some() {
                if let Err(e) =
                    sync::sync_bear_to_letta(state.sqlx_pool(), state.letta.as_ref(), id).await
                {
                    tracing::warn!(%id, "Letta sync after create failed: {e}");
                    let page = bear_new_form_context(&state, &form).await;
                    return render_template(
                        &state,
                        "bear/new.html",
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

async fn bear_details_get(
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
    let is_admin = viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await?;
    let members = bears_db::list_members_for_bear(state.sqlx_pool(), bear.id).await?;
    let activity = bears_db::list_chat_activity_for_bear(state.sqlx_pool(), bear.id, 80).await?;

    let letta_configured = state.letta.is_enabled();
    let letta_api_base = state.config.letta_base_url.trim().to_string();

    let (letta_agent_summary, letta_agent_fetch_error, letta_diagnostics, letta_diag_error) =
        if letta_configured {
            if let Some(agent_id) = bear.letta_agent_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                match state.letta.fetch_agent(agent_id).await {
                    Ok(v) => (
                        Some(AgentSummary::from_letta_agent_state(&v)),
                        None,
                        Some(LettaAgentDiagnostics::from_agent_json(&v)),
                        None,
                    ),
                    Err(e) => {
                        let msg = e.to_string();
                        (None, Some(msg.clone()), None, Some(msg))
                    }
                }
            } else {
                (None, None, None, None)
            }
        } else {
            (None, None, None, None)
        };

    let letta_memory_blocks_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.memory_block_count)
        .map(|n| n.to_string());
    let letta_tools_count_label = letta_agent_summary
        .as_ref()
        .and_then(|s| s.tool_count)
        .map(|n| n.to_string());

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

    render_template(
        &state,
        "bear/details.html",
        auth_session,
        context! {
            bear,
            is_admin,
            members,
            activity,
            letta_configured,
            letta_api_base,
            letta_agent_summary,
            letta_agent_fetch_error,
            letta_diagnostics,
            letta_diag_error,
            letta_memory_blocks_label,
            letta_tools_count_label,
            tools_json_display,
            letta_tool_ids_display,
        },
    )
    .await
}

async fn bear_edit_get(
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
    if !viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }
    let form = NewBearForm::from(&bear);
    let page = bear_edit_page_context(&state, &bear, &form).await;
    render_template(
        &state,
        "bear/edit.html",
        auth_session,
        context! {
            bear,
            form,
            ..page
        },
    )
    .await
}

async fn bear_edit_post(
    Path(slug): Path<String>,
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

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }

    let letta_fetch = if state.letta.is_enabled() {
        Some(
            state
                .letta
                .list_llm_models()
                .await
                .map(|opts| {
                    let model_trim = form.default_model.trim();
                    let h = (!model_trim.is_empty()).then_some(model_trim);
                    ensure_stored_model_in_options_for_handle(h, opts)
                }),
        )
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
            form.system_prompt.trim(),
            default_model_opt,
            None::<Json<serde_json::Value>>,
            letta_agent_type_db.as_deref(),
            Json(letta_tool_ids.clone()),
        )
        .await?;

        if let Err(e) = sync::sync_bear_to_letta(state.sqlx_pool(), state.letta.as_ref(), bear.id).await {
            tracing::warn!(bear_id = %bear.id, "Letta sync after bear edit failed: {e}");
            let bear = bears_db::get_bear(state.sqlx_pool(), bear.id)
                .await?
                .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
            let page = bear_edit_page_context(&state, &bear, &form).await;
            let empty_errors = ValidationErrors::new();
                    return render_template(
                &state,
                "bear/edit.html",
                auth_session,
                context! {
                    errors => empty_errors,
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

        let out_slug = form.slug.trim().to_string();
        return Ok(Redirect::to(&format!("/bear/{out_slug}/details")).into_response());
    }

    let page = bear_edit_page_context(&state, &bear, &form).await;
    render_template(
        &state,
        "bear/edit.html",
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
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
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
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await? {
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

    Ok(Redirect::to(&format!("/bear/{}/details", bear.slug)).into_response())
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
    let user_id = auth_session
        .user
        .as_ref()
        .map(|u| u.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))?;
    if let Some(r) = email_verify_redirect(state.sqlx_pool(), user_id).await? {
        return Ok(r.into_response());
    }

    let bear = load_bear_member(state.sqlx_pool(), user_id, &slug).await?;
    if !viewer_is_bear_admin(state.sqlx_pool(), user_id, bear.id).await? {
        return Err(CustomError::Authorization(
            "bear admin role required".to_string(),
        ));
    }

    let target_role = bears_db::membership_role_for_user(state.sqlx_pool(), body.remove_user_id, bear.id)
        .await?
        .ok_or_else(|| CustomError::NotFound("user is not a member of this bear".to_string()))?;

    if role_is_bear_admin(target_role.as_deref()) {
        let n = bears_db::count_bear_admins(state.sqlx_pool(), bear.id).await?;
        if n <= 1 {
            return Err(CustomError::ValidationError(
                "cannot remove the last bear admin; promote another admin first".to_string(),
            ));
        }
    }

    bears_db::revoke_membership(state.sqlx_pool(), body.remove_user_id, bear.id).await?;

    Ok(Redirect::to(&format!("/bear/{}/details", bear.slug)).into_response())
}
