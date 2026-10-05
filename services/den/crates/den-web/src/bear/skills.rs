//! Human review and attachment of instruction-only Skills; no executable installation.

use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use den_core::{
    ids::{BearId, UserId},
    RuntimeContextLabel,
};
use den_service::skills::{self, SkillId};
use minijinja::context;
use serde::Deserialize;

#[derive(Deserialize)]
struct Draft {
    name: String,
    version: String,
    description: String,
    content: String,
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
    web::render_template(&state,"bear/manage/skills.html",session,context!{catalog,can_manage_bear,manage_title=>"Skills",..super::settings::bear_nav_context(&bear,"skills")}).await
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
    skills::create_draft(
        state.sqlx_pool(),
        UserId::new(session.user.as_ref().unwrap().id),
        &form.name,
        &form.version,
        &form.description,
        &form.content,
    )
    .await?;
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
    match form.operation {
        Operation::Approve => {
            skills::approve(
                state.sqlx_pool(),
                actor,
                id,
                &form.content_hash,
                form.confirm_public,
            )
            .await?
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
            .await?
        }
        Operation::Detach => {
            skills::detach(state.sqlx_pool(), BearId::new(bear.id), actor, id).await?
        }
        Operation::Disable => skills::disable(state.sqlx_pool(), actor, id).await?,
    }
    Ok(Redirect::to(&format!("/bear/{}/skills", bear.slug)).into_response())
}
