//! Staged new-Bear model setup: gateway authority precedes the first active selection.

use axum::http::StatusCode;
use den_core::ids::BearId;
use den_service::bears::{db, db::BearParams, model_configurations};
use uuid::Uuid;

use super::create_support::{provision_bifrost_virtual_key_for_bear, NewBearForm};
use crate::{errors::CustomError, model_availability, AppState};

pub(crate) struct SetupFailure {
    pub status: StatusCode,
    pub message: String,
    pub saved_bear: Option<BearId>,
}

impl SetupFailure {
    fn selection(error: &CustomError) -> Self {
        let (status, message) = model_availability::form_failure(error).unwrap_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Bear model setup could not be completed. Check gateway setup and try again."
                    .into(),
            )
        });
        Self {
            status,
            message,
            saved_bear: None,
        }
    }
}

/// Compensate only the row just inserted by this operation, before membership or
/// runtime initialization. Web-state checks and conservative shared deletion refuse intervening work.
async fn compensate(
    state: &AppState,
    id: Uuid,
    form: &NewBearForm,
    mut failure: SetupFailure,
) -> SetupFailure {
    let untouched: Result<bool, CustomError> = async {
        let Some(bear) = db::get_bear(state.sqlx_pool(), id).await? else {
            return Ok(false);
        };
        Ok(bear.name == form.name.trim()
            && bear.description == form.description.trim()
            && bear.system_prompt == form.system_prompt.trim()
            && bear.default_model.is_none()
            && db::list_members_for_bear(state.sqlx_pool(), id)
                .await?
                .is_empty()
            && model_configurations::list(state.sqlx_pool(), id.into())
                .await?
                .is_empty()
            && den_service::bears::hats::list_hats(state.sqlx_pool(), id.into())
                .await?
                .is_empty()
            && den_service::conversation::persistence::list_conversations_for_bear(
                state.sqlx_pool(),
                id,
                1,
            )
            .await?
            .is_empty())
    }
    .await;
    if matches!(untouched, Ok(true)) && db::delete_bear(state.sqlx_pool(), id).await.is_ok() {
        failure
            .message
            .push_str(" No Bear was saved. Your draft is preserved.");
    } else {
        failure.saved_bear = Some(id.into());
        failure.message.push_str(" The new Bear remains saved; the proposed selection was not confirmed saved. Cleanup was refused; inspect that Bear in Models and Diagnostics. Do not create it again.");
    }
    failure
}

pub(crate) async fn create_with_validated_model(
    state: &AppState,
    form: &NewBearForm,
) -> Result<Uuid, SetupFailure> {
    // This is only metadata validation, never proof of gateway access. Reject bad
    // proposals before creating rows or contacting the gateway management API.
    let explicit = !form.default_model.trim().is_empty();
    let proposed = if explicit {
        form.default_model.as_str()
    } else {
        state.config.default_llm_model.as_str()
    };
    let model =
        model_configurations::validate_model_configuration(state.sqlx_pool(), proposed, None)
            .await
            .map_err(|error| SetupFailure::selection(&error.into()))?
            .model_handle;
    let id = db::create_bear(state.sqlx_pool(), BearParams {
        slug: form.slug.trim(), name: form.name.trim(), description: form.description.trim(),
        system_prompt: form.system_prompt.trim(), default_model: None,
        tools_enabled: None, context_profile: None,
    }).await.map_err(|_| SetupFailure {
        status: StatusCode::CONFLICT,
        message: "The Bear could not be created. Check whether its handle is already in use before retrying.".into(),
        saved_bear: None,
    })?;
    if provision_bifrost_virtual_key_for_bear(state, id, form.slug.trim())
        .await
        .is_err()
    {
        return Err(compensate(
            state,
            id,
            form,
            SetupFailure {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message:
                    "Bear setup failed while provisioning its gateway key. Check Bifrost key setup."
                        .into(),
                saved_bear: None,
            },
        )
        .await);
    }
    if let Err(error) = model_availability::validate_model(state, id.into(), model.as_str()).await {
        return Err(compensate(state, id, form, SetupFailure::selection(&error)).await);
    }
    // The initial compatibility write routes atomically to the canonical configuration.
    if db::update_bear(
        state.sqlx_pool(),
        id,
        BearParams {
            slug: form.slug.trim(),
            name: form.name.trim(),
            description: form.description.trim(),
            system_prompt: form.system_prompt.trim(),
            default_model: explicit.then(|| model.as_str()),
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .is_err()
    {
        return Err(compensate(state, id, form, SetupFailure {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "The verified model selection could not be saved. Try again after checking Den's database health.".into(),
            saved_bear: None,
        }).await);
    }
    Ok(id)
}
