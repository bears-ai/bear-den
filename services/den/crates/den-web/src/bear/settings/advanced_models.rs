//! Validate every input, including a replacement gateway secret, before writes.
//! Drafts deliberately exclude secrets and errors never echo provider responses.
use super::{
    load_session_bear_manage, model_configurations, parse_loop_control_form_value,
    parse_tool_budget_multiplier_form_value, render_models_page_with_draft, BearModelsForm,
};
use crate::{auth_backend::AuthSession, errors::CustomError, web::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use den_service::bears::db as bears_db;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub(super) struct Draft {
    pub bear_loop_control: String,
    pub bear_tool_budget_multiplier: String,
    pub bifrost_virtual_key_id: String,
    pub bifrost_virtual_key_name: String,
    pub bifrost_virtual_key_clear: bool,
}

impl From<&BearModelsForm> for Draft {
    fn from(form: &BearModelsForm) -> Self {
        Self {
            bear_loop_control: form.bear_loop_control.clone(),
            bear_tool_budget_multiplier: form.bear_tool_budget_multiplier.clone(),
            bifrost_virtual_key_id: form.bifrost_virtual_key_id.clone(),
            bifrost_virtual_key_name: form.bifrost_virtual_key_name.clone(),
            bifrost_virtual_key_clear: matches!(
                form.bifrost_virtual_key_clear.trim(),
                "on" | "true" | "1" | "yes"
            ),
        }
    }
}

async fn rejected(
    state: AppState,
    auth_session: AuthSession,
    slug: &str,
    draft: Draft,
    field_errors: std::collections::BTreeMap<&'static str, String>,
    error: String,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let response = render_models_page_with_draft(
        state,
        auth_session,
        bear,
        true,
        None,
        Some(error),
        model_configurations::PendingModelsForm::default(),
        Some(draft),
        field_errors,
    )
    .await?;
    Ok((StatusCode::BAD_REQUEST, response).into_response())
}

pub(super) async fn save(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<BearModelsForm>,
) -> Result<Response, CustomError> {
    let bear = match load_session_bear_manage(&state, &auth_session, &slug).await? {
        Ok(bear) => bear,
        Err(redirect) => return Ok(redirect.into_response()),
    };
    let draft = Draft::from(&form);
    let mut errors = std::collections::BTreeMap::new();
    let loop_control = parse_loop_control_form_value(&form.bear_loop_control);
    let multiplier = parse_tool_budget_multiplier_form_value(&form.bear_tool_budget_multiplier);
    if let Err(error) = &loop_control {
        errors.insert("bear_loop_control", error.to_string());
    }
    if let Err(error) = &multiplier {
        errors.insert("bear_tool_budget_multiplier", error.to_string());
    }
    if draft.bifrost_virtual_key_clear && !form.bifrost_virtual_key_value.trim().is_empty() {
        errors.insert(
            "bifrost_virtual_key_value",
            "Choose either Clear or a replacement key, not both.".into(),
        );
    }
    if !errors.is_empty() {
        let message = format!(
            "Nothing saved. {}",
            errors.values().cloned().collect::<Vec<_>>().join(" ")
        );
        return rejected(state, auth_session, &slug, draft, errors, message).await;
    }
    let loop_control = loop_control?;
    let multiplier = multiplier?;
    let value = form.bifrost_virtual_key_value.trim();
    if !value.is_empty() {
        let client = den_service::bifrost_governance::BifrostGovernanceClient::new(&state.config);
        if client.validate_virtual_key_value(value).await.is_err() {
            let message = "Nothing saved. Replacement key could not be validated. Check the key and gateway connectivity, then enter the secret again; it has not been retained.";
            errors.insert("bifrost_virtual_key_value", message.into());
            return rejected(state, auth_session, &slug, draft, errors, message.into()).await;
        }
    }
    let mut saved = Vec::new();
    let writes: Result<(), (&str, CustomError)> = async {
        bears_db::set_bear_agent_loop_control_setting(state.sqlx_pool(), bear.id, loop_control)
            .await
            .map_err(|error| ("loop control", error.into()))?;
        saved.push("loop control");
        bears_db::set_bear_tool_budget_multiplier(state.sqlx_pool(), bear.id, multiplier)
            .await
            .map_err(|error| ("tool budget", error.into()))?;
        saved.push("tool budget");
        if draft.bifrost_virtual_key_clear {
            bears_db::clear_bear_bifrost_virtual_key(state.sqlx_pool(), bear.id)
                .await
                .map_err(|error| ("gateway settings", error.into()))?;
        } else {
            let id = form.bifrost_virtual_key_id.trim();
            let name = form.bifrost_virtual_key_name.trim();
            if value.is_empty() {
                bears_db::set_bear_bifrost_virtual_key_metadata(
                    state.sqlx_pool(),
                    bear.id,
                    (!id.is_empty()).then_some(id),
                    (!name.is_empty()).then_some(name),
                )
                .await
                .map_err(|error| ("gateway settings", error.into()))?;
            } else {
                bears_db::set_bear_bifrost_virtual_key(
                    state.sqlx_pool(),
                    bear.id,
                    (!id.is_empty()).then_some(id),
                    (!name.is_empty()).then_some(name),
                    Some(value),
                    &state.config.den_secret_encryption_key,
                )
                .await
                .map_err(|error| ("gateway settings", error.into()))?;
            }
        }
        saved.push("gateway settings");
        Ok(())
    }
    .await;
    if let Err((stage, _error)) = writes {
        // Database/provider diagnostics can contain supplied material; the page
        // and query string contain only this safe saved-versus-not projection.
        let message = if saved.is_empty() {
            format!(
                "Nothing saved. Could not save {stage}; retry. Enter any replacement secret again."
            )
        } else {
            format!("Saved: {}. Could not save {stage}; remaining settings were not saved. Review the proposed values and retry; enter any replacement secret again.", saved.join(", "))
        };
        return rejected(state, auth_session, &slug, draft, errors, message).await;
    }
    Ok(Redirect::to(&format!(
        "/bear/{}/models?message={}",
        bear.slug,
        urlencoding::encode("Loop-control, tool-budget, and Bifrost settings saved.")
    ))
    .into_response())
}
