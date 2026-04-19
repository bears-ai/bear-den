// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md if present.
//! Operator console: Letta connectivity and Letta Code harness YAML (Phase 1 M4b).

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use minijinja::context;
use serde::Serialize;

use crate::{
    auth_backend::AuthSession,
    core::bears::{db as bears_db, letta_code_harness},
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health/letta", get(letta_health_json))
        .route("/letta-code", get(letta_code_harness_page))
        .route("/letta-code.yaml", get(letta_code_harness_yaml_download))
}

#[derive(Serialize)]
struct LettaHealthJson {
    ok: bool,
    /// `disabled` | `ok` | `error`
    status: &'static str,
    detail: String,
}

async fn letta_health_json(State(state): State<AppState>) -> Json<LettaHealthJson> {
    if !state.letta.is_api_configured() {
        return Json(LettaHealthJson {
            ok: false,
            status: "disabled",
            detail: "LETTA_API_BASE_URL is not set — Letta API (provisioning, agent CRUD) is off.".to_string(),
        });
    }
    match state.letta.check_health().await {
        Ok(body) => Json(LettaHealthJson {
            ok: true,
            status: "ok",
            detail: body.trim().chars().take(500).collect(),
        }),
        Err(e) => Json(LettaHealthJson {
            ok: false,
            status: "error",
            detail: e.to_string(),
        }),
    }
}

async fn letta_code_harness_page(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let rows = bears_db::list_letta_code_harness_rows(state.sqlx_pool()).await?;
    let skipped = rows
        .iter()
        .filter(|r| r.letta_agent_id.is_none())
        .count();
    let yaml = letta_code_harness::render_letta_code_harness_yaml(
        state.config.letta_api_base_url.as_str(),
        &rows,
    )?;
    let letta_configured = state.letta.is_api_configured();

    web::render_template(
        &state,
        "admin/letta_code_harness.html",
        auth_session,
        context! {
            harness_yaml => yaml,
            skipped_unprovisioned => skipped,
            letta_configured => letta_configured,
        },
    )
    .await
}

async fn letta_code_harness_yaml_download(State(state): State<AppState>) -> Result<Response, CustomError> {
    let rows = bears_db::list_letta_code_harness_rows(state.sqlx_pool()).await?;
    let yaml = letta_code_harness::render_letta_code_harness_yaml(
        state.config.letta_api_base_url.as_str(),
        &rows,
    )?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/yaml; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"letta-code.yaml\""),
    );

    Ok((headers, yaml).into_response())
}
