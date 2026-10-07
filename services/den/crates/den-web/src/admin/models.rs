// ROUTES: When modifying routes in this file, update /src/ROUTES.md.
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::auth_backend::AuthSession;
use crate::errors::CustomError;
use crate::web::{self, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/models", get(index).post(create_model))
        .route("/models/add-from-catalog", post(add_from_catalog))
        .route("/models/update", post(update_model))
        .route("/models/delete", post(delete_model))
}

#[derive(Debug, Deserialize)]
struct ModelQuery {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelForm {
    handle: String,
    display_name: String,
    selectable: Option<String>,
    recommended: Option<String>,
    sort_order: Option<i32>,
    notes: Option<String>,
    metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogModelForm {
    handle: String,
    selectable: Option<String>,
    recommended: Option<String>,
    sort_order: Option<i32>,
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteForm {
    handle: String,
}

#[derive(Debug, Serialize)]
struct ModelRow {
    handle: String,
    display_name: String,
    selectable: bool,
    recommended: bool,
    sort_order: Option<i32>,
    notes: String,
    metadata_json: String,
    bifrost_status: String,
    bifrost_context_window: String,
    bifrost_max_output_tokens: String,
    bifrost_capabilities: String,
}

#[derive(Debug, Serialize)]
struct CatalogRow {
    handle: String,
    provider: String,
    display_name: String,
    context_window: u32,
    max_output_tokens: String,
    capabilities: String,
    metadata_json: String,
}

#[derive(Debug, Serialize)]
struct UsageRow {
    model: String,
    provider: String,
    requests: String,
    tokens: String,
    cost: String,
}

#[derive(Debug, Serialize)]
struct UsageSummary {
    status: String,
    error: String,
    rows: Vec<UsageRow>,
    has_rows: bool,
}

async fn index(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(query): Query<ModelQuery>,
) -> Result<Response, CustomError> {
    render_index(state, auth_session, query.message).await
}

async fn create_model(
    State(state): State<AppState>,
    Form(form): Form<ModelForm>,
) -> Result<impl IntoResponse, CustomError> {
    let form = normalize_form(form)?;
    let metadata_json = parse_metadata_json(form.metadata_json.as_deref().unwrap_or("{}"))?;
    sqlx::query!(
        r"
        INSERT INTO model_selection_options (
            handle, display_name, selectable, recommended, sort_order, notes, metadata_json
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (handle) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            selectable = EXCLUDED.selectable,
            recommended = EXCLUDED.recommended,
            sort_order = EXCLUDED.sort_order,
            notes = EXCLUDED.notes,
            metadata_json = EXCLUDED.metadata_json,
            updated_at = NOW()
        ",
        &form.handle,
        &form.display_name,
        form.selectable.is_some(),
        form.recommended.is_some(),
        form.sort_order,
        form.notes.as_deref(),
        metadata_json,
    )
    .execute(state.sqlx_pool())
    .await?;

    Ok(Redirect::to("/admin/models?message=Saved"))
}

async fn add_from_catalog(
    State(state): State<AppState>,
    Form(form): Form<CatalogModelForm>,
) -> Result<impl IntoResponse, CustomError> {
    let form = normalize_catalog_form(form)?;
    let (display_name, metadata_json) = {
        let catalog = state
            .bifrost_catalog
            .read()
            .map_err(|_| CustomError::System("Bifrost catalog lock poisoned".to_string()))?;
        let entry = catalog.resolve(&form.handle).ok_or_else(|| {
            CustomError::ValidationError(format!(
                "{} is not present in the Bifrost catalog; use Add custom model instead",
                form.handle
            ))
        })?;
        (
            entry
                .display_name
                .clone()
                .unwrap_or_else(|| form.handle.clone()),
            catalog_metadata_json(entry),
        )
    };

    sqlx::query!(
        r"
        INSERT INTO model_selection_options (
            handle, display_name, selectable, recommended, sort_order, notes, metadata_json
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (handle) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            selectable = EXCLUDED.selectable,
            recommended = EXCLUDED.recommended,
            sort_order = EXCLUDED.sort_order,
            notes = EXCLUDED.notes,
            metadata_json = EXCLUDED.metadata_json,
            updated_at = NOW()
        ",
        &form.handle,
        &display_name,
        form.selectable.is_some(),
        form.recommended.is_some(),
        form.sort_order,
        form.notes.as_deref(),
        metadata_json,
    )
    .execute(state.sqlx_pool())
    .await?;

    Ok(Redirect::to(
        "/admin/models?message=Enabled%20from%20Bifrost%20catalog",
    ))
}

async fn update_model(
    State(state): State<AppState>,
    Form(form): Form<ModelForm>,
) -> Result<impl IntoResponse, CustomError> {
    let form = normalize_form(form)?;
    let result = sqlx::query!(
        r"
        UPDATE model_selection_options
        SET display_name = $2,
            selectable = $3,
            recommended = $4,
            sort_order = $5,
            notes = $6,
            updated_at = NOW()
        WHERE handle = $1
        ",
        &form.handle,
        &form.display_name,
        form.selectable.is_some(),
        form.recommended.is_some(),
        form.sort_order,
        form.notes.as_deref(),
    )
    .execute(state.sqlx_pool())
    .await?;

    if result.rows_affected() == 0 {
        return Err(CustomError::NotFound(format!(
            "model option {} was not found",
            form.handle
        )));
    }

    Ok(Redirect::to("/admin/models?message=Saved"))
}

async fn delete_model(
    State(state): State<AppState>,
    Form(form): Form<DeleteForm>,
) -> Result<impl IntoResponse, CustomError> {
    let handle = form.handle.trim();
    if handle.is_empty() {
        return Err(CustomError::ValidationError(
            "model handle is required".to_string(),
        ));
    }
    sqlx::query!(
        "DELETE FROM model_selection_options WHERE handle = $1",
        handle
    )
    .execute(state.sqlx_pool())
    .await?;
    Ok(Redirect::to("/admin/models?message=Deleted"))
}

async fn render_index(
    state: AppState,
    auth_session: AuthSession,
    message: Option<String>,
) -> Result<Response, CustomError> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            bool,
            bool,
            Option<i32>,
            Option<String>,
            Json<serde_json::Value>,
        ),
    >(
        r"
        SELECT handle, display_name, selectable, recommended, sort_order, notes, metadata_json
        FROM model_selection_options
        ORDER BY sort_order NULLS LAST, display_name, handle
        ",
    )
    .fetch_all(state.sqlx_pool())
    .await?;

    let catalog = state
        .bifrost_catalog
        .read()
        .map_err(|_| CustomError::System("Bifrost catalog lock poisoned".to_string()))?
        .clone();
    let configured_handles = rows
        .iter()
        .map(|row| den_service::bifrost::canonical_catalog_key(&row.0, None, None))
        .collect::<std::collections::HashSet<_>>();

    let models = rows
        .into_iter()
        .map(|row| {
            let bifrost = catalog.resolve(&row.0);
            ModelRow {
                handle: row.0,
                display_name: row.1,
                selectable: row.2,
                recommended: row.3,
                sort_order: row.4,
                notes: row.5.unwrap_or_default(),
                metadata_json: pretty_json(row.6 .0),
                bifrost_status: bifrost
                    .map(|entry| {
                        if entry.available {
                            "available"
                        } else {
                            "unavailable"
                        }
                    })
                    .unwrap_or("missing")
                    .to_string(),
                bifrost_context_window: bifrost
                    .map(|entry| entry.context_window.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                bifrost_max_output_tokens: bifrost
                    .and_then(|entry| entry.max_output_tokens)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                bifrost_capabilities: bifrost
                    .map(|entry| {
                        capabilities(
                            entry.supports_tools,
                            entry.supports_responses_api,
                            entry.supports_vision,
                        )
                    })
                    .unwrap_or_else(|| "—".to_string()),
            }
        })
        .collect::<Vec<_>>();

    let mut catalog_rows = catalog
        .models
        .iter()
        .filter(|(handle, _)| !configured_handles.contains(*handle))
        .map(|(handle, entry)| CatalogRow {
            handle: handle.clone(),
            provider: entry.provider.clone(),
            display_name: entry.display_name.clone().unwrap_or_else(|| handle.clone()),
            context_window: entry.context_window,
            max_output_tokens: entry
                .max_output_tokens
                .map(|value| value.to_string())
                .unwrap_or_else(|| "—".to_string()),
            capabilities: capabilities(
                entry.supports_tools,
                entry.supports_responses_api,
                entry.supports_vision,
            ),
            metadata_json: catalog_metadata_json(entry).to_string(),
        })
        .collect::<Vec<_>>();
    catalog_rows.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    let catalog_row_count = catalog_rows.len();
    let mut catalog_providers = catalog_rows
        .iter()
        .map(|row| row.provider.clone())
        .collect::<Vec<_>>();
    catalog_providers.sort();
    catalog_providers.dedup();

    let usage = bifrost_usage_summary(&state).await;

    web::render_template(
        &state,
        "admin/models/index.html",
        auth_session,
        context! {
            message => message.unwrap_or_default(),
            models,
            catalog_rows,
            catalog_row_count,
            catalog_providers,
            catalog_source => catalog.source,
            catalog_stale => catalog.stale,
            catalog_fetched_at => catalog.fetched_at.map(format_time).unwrap_or_else(|| "—".to_string()),
            usage,
            default_metadata => "{}",
        },
    )
    .await
}

async fn bifrost_usage_summary(state: &AppState) -> UsageSummary {
    let client = den_service::bifrost_governance::BifrostGovernanceClient::new(&state.config);
    match client.get_server_model_usage_rankings().await {
        Ok(payload) => {
            let rows = payload
                .get("rankings")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .map(|row| UsageRow {
                    model: json_str(row, "model"),
                    provider: json_str(row, "provider"),
                    requests: display_number(json_f64(row, "total_requests")),
                    tokens: display_number(json_f64(row, "total_tokens")),
                    cost: display_money(json_f64(row, "total_cost")),
                })
                .collect::<Vec<_>>();
            UsageSummary {
                status: "ok".to_string(),
                error: String::new(),
                has_rows: !rows.is_empty(),
                rows,
            }
        }
        Err(err) => UsageSummary {
            status: "unavailable".to_string(),
            error: err.to_string(),
            rows: Vec::new(),
            has_rows: false,
        },
    }
}

fn normalize_form(mut form: ModelForm) -> Result<ModelForm, CustomError> {
    form.handle = form.handle.trim().to_string();
    form.display_name = form.display_name.trim().to_string();
    form.notes = form
        .notes
        .map(|notes| notes.trim().to_string())
        .filter(|notes| !notes.is_empty());
    if form.handle.is_empty() {
        return Err(CustomError::ValidationError(
            "model handle is required".to_string(),
        ));
    }
    if form.display_name.is_empty() {
        return Err(CustomError::ValidationError(
            "display name is required".to_string(),
        ));
    }
    if let Some(raw) = &form.metadata_json {
        parse_metadata_json(raw)?;
    }
    Ok(form)
}

fn normalize_catalog_form(mut form: CatalogModelForm) -> Result<CatalogModelForm, CustomError> {
    form.handle = form.handle.trim().to_string();
    form.notes = form
        .notes
        .map(|notes| notes.trim().to_string())
        .filter(|notes| !notes.is_empty());
    if form.handle.is_empty() {
        return Err(CustomError::ValidationError(
            "model handle is required".to_string(),
        ));
    }
    Ok(form)
}

fn catalog_metadata_json(entry: &den_service::bifrost::BifrostCatalogEntry) -> serde_json::Value {
    json!({
        "context_window": entry.context_window,
        "max_output_tokens": entry.max_output_tokens,
        "supports_tools": entry.supports_tools,
        "supports_responses_api": entry.supports_responses_api,
        "supports_vision": entry.supports_vision,
        "supports_reasoning_effort": entry.supports_reasoning_effort,
    })
}

fn parse_metadata_json(raw: &str) -> Result<serde_json::Value, CustomError> {
    serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|err| CustomError::ValidationError(format!("metadata JSON is invalid: {err}")))
}

fn capabilities(tools: Option<bool>, responses: Option<bool>, vision: Option<bool>) -> String {
    let mut caps = Vec::new();
    if tools == Some(true) {
        caps.push("tools");
    }
    if responses == Some(true) {
        caps.push("responses");
    }
    if vision == Some(true) {
        caps.push("vision");
    }
    if caps.is_empty() {
        "—".to_string()
    } else {
        caps.join(", ")
    }
}

fn pretty_json(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

fn json_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(serde_json::Value::as_f64)
}

fn json_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("—")
        .to_string()
}

fn display_number(value: Option<f64>) -> String {
    value
        .map(|value| {
            if value.fract().abs() < f64::EPSILON {
                format!("{}", value as i64)
            } else {
                format!("{value:.4}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
        })
        .unwrap_or_else(|| "—".to_string())
}

fn display_money(value: Option<f64>) -> String {
    value
        .map(|value| {
            format!("${value:.4}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string())
}

fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod catalog_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_form_validates_metadata_json() {
        let err = normalize_form(ModelForm {
            handle: "openai/gpt-4o".to_string(),
            display_name: "GPT-4o".to_string(),
            selectable: Some("on".to_string()),
            recommended: None,
            sort_order: None,
            notes: None,
            metadata_json: Some("{".to_string()),
        })
        .unwrap_err();
        assert!(err.to_string().contains("metadata JSON is invalid"));
    }

    #[test]
    fn capabilities_omits_false_values() {
        assert_eq!(
            capabilities(Some(true), Some(false), Some(true)),
            "tools, vision"
        );
        assert_eq!(capabilities(None, Some(false), None), "—");
    }
}
