//! Whole-configuration hat override; this preference never changes hat authority.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::{extract::Form, routing::RouterExt};
use den_core::ids::{BearId, HatId};
use den_service::bears::model_configurations;
use uuid::Uuid;

use super::{hat_url, load_session_bear_manage, render_detail};
use crate::{
    auth_backend::AuthSession, bear::settings::model_configurations::ConfigurationSelection,
    errors::CustomError, AppState,
};

pub(super) fn router() -> Router<AppState> {
    Router::new().route_with_tsr("/bear/{slug}/hats/{hat_id}/model", post(set_override))
}

async fn set_override(
    Path((slug, hat_id)): Path<(String, Uuid)>,
    State(state): State<AppState>,
    auth: AuthSession,
    Form(form): Form<ConfigurationSelection>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let hat_id = HatId::new(hat_id);
    let result: Result<(), CustomError> = async {
        super::manage::get_hat(state.sqlx_pool(), BearId::new(bear.id), hat_id).await?;
        crate::model_availability::validate_configuration_selection(
            &state,
            BearId::new(bear.id),
            form.configuration_id,
        )
        .await?;
        model_configurations::set_hat_override(
            state.sqlx_pool(),
            BearId::new(bear.id),
            hat_id,
            form.configuration_id,
        )
        .await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(Redirect::to(&hat_url(&bear.slug, hat_id)).into_response()),
        Err(error) => {
            let Some((status, message)) = crate::model_availability::form_failure(&error) else {
                return Err(error);
            };
            let mut response = render_detail(
                state,
                auth,
                bear,
                hat_id,
                None,
                Some(message),
                form.configuration_id.into(),
            )
            .await?;
            *response.status_mut() = status;
            Ok(response)
        }
    }
}
