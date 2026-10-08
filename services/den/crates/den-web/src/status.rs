//! Public **`/status`** — aggregate stack health plus Den version vs optional GHCR.

use std::time::Duration;

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

use den_llm::model_registry::{self, ModelGatewayCompatibilityReport};

use crate::{
    auth_backend::{AuthSession, SessionUser},
    build_info,
};

#[path = "stack_health/status_feedback.rs"]
mod feedback;
use crate::web::stack_health::{self, CheckState, StackHealthReport, StackHealthTemplateRow};
use crate::web::AppState;

const GITHUB_FETCH_TIMEOUT: Duration = Duration::from_secs(12);

#[cfg(test)]
#[path = "stack_health/status_usability_tests.rs"]
mod usability_tests;

#[derive(Clone, serde::Serialize)]
pub struct StatusPayload {
    pub health: StackHealthReport,
    pub den_version: build_info::VersionBody,
    pub ghcr_den: Option<GhcrPackageRow>,
    pub ghcr_config_note: Option<String>,
    pub ghcr_error: Option<String>,
    pub model_registry: ModelRegistryStatus,
}

#[derive(Clone, serde::Serialize)]
pub struct ModelRegistryStatus {
    pub report: ModelGatewayCompatibilityReport,
    pub gateway_error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct GhcrPackageRow {
    pub package: String,
    pub tags: Vec<String>,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct GhPackageVersion {
    updated_at: String,
    metadata: Option<GhMetadata>,
}

#[derive(Deserialize)]
struct GhMetadata {
    container: Option<GhContainerMeta>,
}

#[derive(Deserialize)]
struct GhContainerMeta {
    tags: Option<Vec<String>>,
}

pub async fn page(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, crate::errors::CustomError> {
    let payload = gather_status(&state).await;
    render_page(&state, auth_session.user.as_ref(), payload)
}

fn render_page(
    state: &AppState,
    user: Option<&SessionUser>,
    mut payload: StatusPayload,
) -> Result<Response, crate::errors::CustomError> {
    feedback::sanitize(&mut payload, &state.config);
    let status = if payload.health.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let rows: Vec<StackHealthTemplateRow> = payload
        .health
        .checks
        .iter()
        .map(|c| StackHealthTemplateRow {
            id: c.id,
            label: c.label,
            state: match c.state {
                CheckState::Ok => "ok",
                CheckState::Warn => "warn",
                CheckState::Fail => "fail",
                CheckState::Skipped => "skipped",
            },
            detail: c.detail.clone(),
        })
        .collect();

    let deploy_rows = vec![build_deploy_row(&payload)];
    let ghcr_note = payload.ghcr_config_note.clone().unwrap_or_default();

    let ctx = minijinja::context! {
        session => user.map(|user| minijinja::context! {
            user_id => user.id, username => &user.username,
            is_admin => user.is_admin, theme => &user.theme,
        }),
        title => "BEARS status",
        template_tag => "page-bears-health",
        app_display_name => state.config.app_display_name.clone(),
        app_slug => state.config.app_slug.clone(),
        public_web_origin => state.config.web_public_origin(),
        rows => rows,
        overall_ok => payload.health.ok,
        checked_at => payload.health.checked_at,
        json_path => "/status.json",
        deploy_rows => deploy_rows,
        ghcr_note => ghcr_note,
        ghcr_error => payload.ghcr_error,
        model_registry => payload.model_registry,
    };
    let template = state
        .template_env
        .get_template("status.html")
        .map_err(|e| crate::errors::CustomError::Render(format!("status template: {e}")))?;
    let body = template
        .render(ctx)
        .map_err(|e| crate::errors::CustomError::Render(format!("status render: {e}")))?;
    Ok((
        status,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Html(body),
    )
        .into_response())
}

#[derive(serde::Serialize)]
pub struct DeployRow {
    pub component: String,
    pub git_sha: String,
    pub semver: String,
    pub built_at: String,
    pub ghcr_tags: String,
    pub ghcr_updated: String,
    pub in_sync: String,
}

fn build_deploy_row(payload: &StatusPayload) -> DeployRow {
    let den = &payload.den_version;
    DeployRow {
        component: "Den".to_string(),
        git_sha: den.git_sha.clone(),
        semver: den.version.clone(),
        built_at: den.built_at_utc.clone(),
        ghcr_tags: payload
            .ghcr_den
            .as_ref()
            .map(|g| g.tags.join(", "))
            .unwrap_or_else(|| "—".to_string()),
        ghcr_updated: payload
            .ghcr_den
            .as_ref()
            .map(|g| g.updated_at.clone())
            .unwrap_or_else(|| "—".to_string()),
        in_sync: sync_label(&den.git_sha, payload.ghcr_den.as_ref()),
    }
}

fn sync_label(deployed_sha: &str, ghcr: Option<&GhcrPackageRow>) -> String {
    let d = deployed_sha.trim();
    if d.is_empty() || d == "unknown" {
        return "unknown".to_string();
    }
    let Some(g) = ghcr else {
        return "?".to_string();
    };
    for t in &g.tags {
        if t.eq_ignore_ascii_case("latest") {
            continue;
        }
        if t == d {
            return "yes".to_string();
        }
        if d.len() >= 7 && t.len() >= 7 && d[..7].eq_ignore_ascii_case(&t[..7]) {
            return "yes".to_string();
        }
        if t.len() >= 7 && d.starts_with(t) {
            return "yes".to_string();
        }
    }
    "no".to_string()
}

pub async fn json_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    let payload = gather_status(&state).await;
    let status = if payload.health.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(payload),
    )
}

async fn gather_status(state: &AppState) -> StatusPayload {
    let health = stack_health::gather(state).await;
    let den_version = build_info::snapshot();

    let cfg = state.config.as_ref();
    let model_registry = gather_model_registry_status(state).await;
    let mut ghcr_config_note: Option<String> = None;
    let mut ghcr_error: Option<String> = None;
    let ghcr_den = if cfg.github_packages_token.trim().is_empty()
        || cfg.ghcr_packages_owner.trim().is_empty()
    {
        ghcr_config_note = Some(
                "Set GITHUB_PACKAGES_TOKEN and GHCR_PACKAGES_OWNER to compare running images with GHCR."
                    .to_string(),
            );
        None
    } else {
        match reqwest::Client::builder()
            .timeout(GITHUB_FETCH_TIMEOUT)
            .connect_timeout(Duration::from_secs(8))
            .build()
        {
            Ok(client) => {
                let token = cfg.github_packages_token.trim();
                let owner = cfg.ghcr_packages_owner.trim();
                let kind = cfg.ghcr_packages_owner_kind.as_str();
                let (d, err_d) = fetch_ghcr_package(&client, token, kind, owner, "den").await;
                let mut notes = Vec::new();
                if let Some(e) = err_d {
                    notes.push(format!("den GHCR: {e}"));
                }
                if !notes.is_empty() {
                    ghcr_error = Some(notes.join(" "));
                }
                d
            }
            Err(e) => {
                ghcr_error = Some(format!("Could not build HTTP client: {e}"));
                None
            }
        }
    };

    let mut payload = StatusPayload {
        health,
        den_version,
        ghcr_den,
        ghcr_config_note,
        ghcr_error,
        model_registry,
    };
    feedback::sanitize(&mut payload, cfg);
    payload
}

async fn gather_model_registry_status(state: &AppState) -> ModelRegistryStatus {
    match state.bifrost_catalog.read() {
        Ok(snapshot) => {
            let handles = snapshot
                .models_vec()
                .into_iter()
                .map(|model| model.handle)
                .collect::<Vec<_>>();
            let gateway_error = if snapshot.models.is_empty() {
                Some("Bifrost model catalog snapshot is empty.".to_string())
            } else if snapshot.stale {
                Some(
                    "Bifrost model catalog snapshot is stale; using last known values.".to_string(),
                )
            } else {
                None
            };
            ModelRegistryStatus {
                report: model_registry::gateway_compatibility_report(handles),
                gateway_error,
            }
        }
        Err(_) => ModelRegistryStatus {
            report: model_registry::gateway_compatibility_report(Vec::<String>::new()),
            gateway_error: Some("Could not read Bifrost model catalog snapshot.".to_string()),
        },
    }
}

async fn fetch_ghcr_package(
    client: &reqwest::Client,
    token: &str,
    owner_kind: &str,
    owner: &str,
    package_name: &str,
) -> (Option<GhcrPackageRow>, Option<String>) {
    let host = "https://api.github.com";
    let path = match owner_kind {
        "user" => format!("/users/{owner}/packages/container/{package_name}/versions"),
        _ => format!("/orgs/{owner}/packages/container/{package_name}/versions"),
    };
    let url = format!("{host}{path}?per_page=100");

    let ua = HeaderValue::from_static("den/BEARS-status-dashboard");
    let auth = HeaderValue::from_str(&format!("Bearer {token}"))
        .unwrap_or_else(|_| HeaderValue::from_static(""));
    let accept = HeaderValue::from_static("application/vnd.github+json");

    let resp = match client
        .get(&url)
        .header(AUTHORIZATION, auth)
        .header(ACCEPT, accept)
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(USER_AGENT, ua)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (None, Some(format!("request failed: {}", e.without_url())));
        }
    };
    let status_code = resp.status();
    if !status_code.is_success() {
        return (None, Some(format!("HTTP {status_code}; verify GHCR token permissions and package visibility. Upstream response body withheld.")));
    }
    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => return (None, Some(e.to_string())),
    };
    let versions: Vec<GhPackageVersion> = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return (None, Some(format!("parse: {e}"))),
    };
    let picked = pick_latestish_version(&versions);
    let Some(v) = picked else {
        return (None, Some("no package versions returned".into()));
    };
    let tags = v
        .metadata
        .as_ref()
        .and_then(|m| m.container.as_ref())
        .and_then(|c| c.tags.clone())
        .unwrap_or_default();
    (
        Some(GhcrPackageRow {
            package: package_name.to_string(),
            tags,
            updated_at: v.updated_at.clone(),
        }),
        None,
    )
}

fn pick_latestish_version(versions: &[GhPackageVersion]) -> Option<&GhPackageVersion> {
    if versions.is_empty() {
        return None;
    }
    let mut with_latest: Vec<&GhPackageVersion> = versions
        .iter()
        .filter(|v| {
            v.metadata
                .as_ref()
                .and_then(|m| m.container.as_ref())
                .and_then(|c| c.tags.as_ref())
                .map(|t| t.iter().any(|x| x == "latest"))
                .unwrap_or(false)
        })
        .collect();
    with_latest.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    if let Some(v) = with_latest.first() {
        return Some(*v);
    }
    let mut all: Vec<&GhPackageVersion> = versions.iter().collect();
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    all.first().copied()
}
