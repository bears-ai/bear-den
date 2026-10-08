//! Human review and attachment of instruction-only Skills; no executable installation.

use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use den_core::{
    ids::{BearId, UserId},
    DenError, RuntimeContextLabel,
};
use den_service::skills::{self, SkillId};
use minijinja::context;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Draft {
    name: String,
    version: String,
    description: String,
    content: String,
}
#[derive(Default)]
struct FormFeedback {
    error: Option<String>,
    draft: Option<Draft>,
    pending: Option<(SkillId, Vec<RuntimeContextLabel>)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Approve,
    Attach,
    Detach,
    Disable,
}
#[derive(Deserialize)]
struct Action {
    operation: Operation,
    #[serde(default)]
    content_hash: String,
    #[serde(default)]
    profiles: Vec<RuntimeContextLabel>,
    #[serde(default)]
    confirm_public: bool,
    #[serde(default)]
    confirm_work: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bear/{slug}/skills", get(view).post(create))
        .route("/bear/{slug}/skills/{id}", post(action))
}
async fn view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) =
        match super::settings::load_session_bear(&state, &session, &slug).await? {
            Ok(v) => v,
            Err(r) => return Ok(r.into_response()),
        };
    let actor = UserId::new(
        session
            .user
            .as_ref()
            .ok_or_else(|| CustomError::Authentication("login required".into()))?
            .id,
    );
    let catalog = skills::list(state.sqlx_pool(), BearId::new(bear.id), actor).await?;
    render_catalog(
        &state,
        session,
        &bear,
        can_manage_bear,
        catalog,
        FormFeedback::default(),
    )
    .await
}
async fn render_catalog(
    state: &AppState,
    session: AuthSession,
    bear: &den_service::bears::Bear,
    can_manage_bear: bool,
    catalog: Vec<skills::Skill>,
    feedback: FormFeedback,
) -> Result<Response, CustomError> {
    let FormFeedback {
        error,
        draft,
        pending,
    } = feedback;
    let pending_skill_id = pending.as_ref().map(|(id, _)| id.0.to_string());
    let pending_profiles = pending.map(|(_, profiles)| {
        profiles
            .into_iter()
            .map(RuntimeContextLabel::as_str)
            .collect::<Vec<_>>()
    });
    let failed = error.is_some();
    let mut response = web::render_template(
        state,
        "bear/manage/skills.html",
        session,
        context! {
            catalog, can_manage_bear, error, draft, pending_skill_id, pending_profiles,
            manage_title => "Skills",
            ..super::settings::bear_nav_context(bear, "skills")
        },
    )
    .await?;
    if failed {
        *response.status_mut() = StatusCode::BAD_REQUEST;
    }
    Ok(response)
}

async fn create(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    session: AuthSession,
    Form(form): Form<Draft>,
) -> Result<Response, CustomError> {
    let (bear, admin) = match super::settings::load_session_bear(&state, &session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    if !admin {
        return Err(CustomError::Authorization(
            "Bear-admin review is required".into(),
        ));
    }
    let result = skills::create_draft(
        state.sqlx_pool(),
        UserId::new(session.user.as_ref().unwrap().id),
        &form.name,
        &form.version,
        &form.description,
        &form.content,
    )
    .await;
    if let Err(error) = result {
        let DenError::ValidationError(message) = error else {
            return Err(error.into());
        };
        let catalog = skills::list(
            state.sqlx_pool(),
            BearId::new(bear.id),
            UserId::new(session.user.as_ref().unwrap().id),
        )
        .await?;
        return render_catalog(
            &state,
            session,
            &bear,
            true,
            catalog,
            FormFeedback {
                error: Some(message),
                draft: Some(form),
                ..Default::default()
            },
        )
        .await;
    }
    Ok(Redirect::to(&format!("/bear/{}/skills", bear.slug)).into_response())
}
async fn action(
    Path((slug, id)): Path<(String, SkillId)>,
    State(state): State<AppState>,
    session: AuthSession,
    Form(form): Form<Action>,
) -> Result<Response, CustomError> {
    let (bear, admin) = match super::settings::load_session_bear(&state, &session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    if !admin {
        return Err(CustomError::Authorization(
            "Bear-admin review is required".into(),
        ));
    }
    let actor = UserId::new(session.user.as_ref().unwrap().id);
    let result = match form.operation {
        Operation::Approve => {
            skills::approve(
                state.sqlx_pool(),
                actor,
                id,
                &form.content_hash,
                form.confirm_public,
            )
            .await
        }
        Operation::Attach => {
            skills::attach(
                state.sqlx_pool(),
                BearId::new(bear.id),
                actor,
                id,
                &form.content_hash,
                &form.profiles,
                form.confirm_work,
            )
            .await
        }
        Operation::Detach => {
            skills::detach(state.sqlx_pool(), BearId::new(bear.id), actor, id).await
        }
        Operation::Disable => skills::disable(state.sqlx_pool(), actor, id).await,
    };
    if let Err(error) = result {
        let DenError::ValidationError(message) = error else {
            return Err(error.into());
        };
        let catalog = skills::list(state.sqlx_pool(), BearId::new(bear.id), actor).await?;
        return render_catalog(
            &state,
            session,
            &bear,
            true,
            catalog,
            FormFeedback {
                error: Some(message),
                pending: Some((id, form.profiles)),
                ..Default::default()
            },
        )
        .await;
    }
    Ok(Redirect::to(&format!("/bear/{}/skills", bear.slug)).into_response())
}
