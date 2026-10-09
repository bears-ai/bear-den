//! Explicit, fresh-inventory hard deletion; private remediation stays creator-only.
use crate::{auth_backend::AuthSession, errors::CustomError, web, AppState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use axum_login::tower_sessions::Session;
use den_core::{BearId, DenError, UserId};
use den_service::{
    artifacts::snapshot_retirement::{self, InventoryFingerprint},
    bears::{db::deletion as service, Bear},
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CONFIRMATION_KEY: &str = "bear_hard_delete_confirmation";
#[derive(Serialize, Deserialize)]
struct Confirmation {
    actor: UserId,
    bear: BearId,
    expected: InventoryFingerprint,
    token: Uuid,
    issued_at: i64,
}
#[derive(Default, Deserialize)]
pub(super) struct DeleteForm {
    #[serde(default)]
    confirm_slug: String,
    #[serde(default)]
    acknowledged: bool,
    expected: Option<InventoryFingerprint>,
    token: Option<Uuid>,
}
async fn context(
    state: &AppState,
    auth: &AuthSession,
    slug: &str,
) -> Result<(UserId, Bear), CustomError> {
    let user = auth
        .user
        .as_ref()
        .ok_or_else(|| CustomError::Authentication("Sign in to continue".into()))?;
    let bear = super::member::load_bear_member(state.sqlx_pool(), user.id, slug).await?;
    if !super::member::viewer_can_manage_bear(state.sqlx_pool(), user, bear.id).await? {
        return Err(CustomError::Authorization(
            "Bear Admin access required".into(),
        ));
    }
    Ok((UserId::new(user.id), bear))
}
async fn render(
    state: &AppState,
    auth: AuthSession,
    session: &Session,
    actor: UserId,
    bear: Bear,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, CustomError> {
    let preview = service::preview(state.sqlx_pool(), actor, BearId::new(bear.id)).await?;
    let copies = snapshot_retirement::history(
        state.sqlx_pool(),
        actor,
        None,
        Some(BearId::new(bear.id)),
        None,
    )
    .await?;
    let blockers: Vec<_> = preview
        .blockers
        .iter()
        .map(|blocker| blocker.explanation())
        .collect();
    let token = Uuid::new_v4();
    session
        .insert(
            CONFIRMATION_KEY,
            Confirmation {
                actor,
                bear: BearId::new(bear.id),
                expected: preview.fingerprint.clone(),
                token,
                issued_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            },
        )
        .await?;
    let mut response=web::render_template(state,"bear/delete.html",auth,context! {
        bear,preview,blockers,copies,token,error,can_manage_bear => true,bear_nav_active => "identity",
    }).await?;
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    Ok(response)
}
pub(super) async fn get(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
) -> Result<Response, CustomError> {
    let (actor, bear) = context(&state, &auth, &slug).await?;
    if let Some(redirect) =
        super::member::email_verify_redirect(state.sqlx_pool(), actor.get()).await?
    {
        return Ok(redirect.into_response());
    }
    render(&state, auth, &session, actor, bear, None, StatusCode::OK).await
}
pub(super) async fn post(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth: AuthSession,
    session: Session,
    Form(form): Form<DeleteForm>,
) -> Result<Response, CustomError> {
    let (actor, bear) = context(&state, &auth, &slug).await?;
    if let Some(redirect) =
        super::member::email_verify_redirect(state.sqlx_pool(), actor.get()).await?
    {
        return Ok(redirect.into_response());
    }
    let confirmation = session.remove::<Confirmation>(CONFIRMATION_KEY).await?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh = confirmation.is_some_and(|value| {
        value.actor == actor
            && value.bear == BearId::new(bear.id)
            && Some(value.token) == form.token
            && Some(&value.expected) == form.expected.as_ref()
            && (0..=900).contains(&(now - value.issued_at))
    });
    if !fresh {
        return render(&state,auth,&session,actor,bear,Some("Review the fresh deletion preview and confirm again. The previous confirmation expired, changed, or was already submitted; nothing was deleted.".into()),StatusCode::CONFLICT).await;
    }
    let result = service::delete_confirmed(
        state.sqlx_pool(),
        service::ConfirmBearDeletion {
            actor,
            bear_id: BearId::new(bear.id),
            expected: form
                .expected
                .ok_or_else(|| CustomError::ValidationError("Review a fresh preview".into()))?,
            confirm_slug: form.confirm_slug,
            acknowledged: form.acknowledged,
        },
    )
    .await;
    match result {
        Ok(()) => Ok(Redirect::to("/").into_response()),
        Err(DenError::ValidationError(error)) => {
            render(
                &state,
                auth,
                &session,
                actor,
                bear,
                Some(error),
                StatusCode::CONFLICT,
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}
