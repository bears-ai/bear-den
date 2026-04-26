//! Public **`/status`** — aggregate stack health ([`super::stack_health::gather`]) plus Den/Codepool versions vs optional GHCR (GitHub Packages API).

use std::time::Duration;

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

use crate::build_info;
use crate::web::stack_health::{self, CheckState, StackHealthReport, StackHealthTemplateRow};
use crate::web::AppState;

const GITHUB_FETCH_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, serde::Serialize)]
pub struct StatusPayload {
    pub health: StackHealthReport,
    pub den_version: build_info::VersionBody,
    pub codepool_version: Option<serde_json::Value>,
    pub codepool_error: Option<String>,
    pub ghcr_den: Option<GhcrPackageRow>,
    pub ghcr_codepool: Option<GhcrPackageRow>,
    pub ghcr_config_note: Option<String>,
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

pub async fn page(State(state): State<AppState>) -> Result<Response, crate::errors::CustomError> {
    let payload = gather_status(&state).await;
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

    let deploy_rows = build_deploy_rows(&payload);
    let ghcr_note = payload.ghcr_config_note.clone().unwrap_or_default();

    let ctx = minijinja::context! {
        title => "BEARS status",
        template_tag => "page-bears-health",
        app_display_name => state.config.app_display_name.clone(),
        app_slug => state.config.app_slug.clone(),
        public_web_origin => state.config.web_public_origin(),
        rows => rows,
        overall_ok => payload.health.ok,
        checked_at => payload.health.checked_at.clone(),
        json_path => "/status.json",
        deploy_rows => deploy_rows,
        ghcr_note => ghcr_note,
    };
    let template = state
        .template_env
        .get_template("status.html")
        .map_err(|e| crate::errors::CustomError::Render(format!("status template: {e}")))?;
    let body = template
        .render(ctx)
        .map_err(|e| crate::errors::CustomError::Render(format!("status render: {e}")))?;
    Ok((status, Html(body)).into_response())
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

fn build_deploy_rows(payload: &StatusPayload) -> Vec<DeployRow> {
    let den = &payload.den_version;
    let mut out = vec![DeployRow {
        component: "Den".to_string(),
        git_sha: den.git_sha.to_string(),
        semver: den.version.to_string(),
        built_at: den.built_at_utc.to_string(),
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
        in_sync: sync_label(den.git_sha, payload.ghcr_den.as_ref()),
    }];

    match &payload.codepool_version {
        Some(v) => {
            let sha = v
                .get("git_sha")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string();
            let ver = v
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("—")
                .to_string();
            out.push(DeployRow {
                component: "Codepool".to_string(),
                git_sha: sha.clone(),
                semver: ver,
                built_at: "—".to_string(),
                ghcr_tags: payload
                    .ghcr_codepool
                    .as_ref()
                    .map(|g| g.tags.join(", "))
                    .unwrap_or_else(|| "—".to_string()),
                ghcr_updated: payload
                    .ghcr_codepool
                    .as_ref()
                    .map(|g| g.updated_at.clone())
                    .unwrap_or_else(|| "—".to_string()),
                in_sync: sync_label(&sha, payload.ghcr_codepool.as_ref()),
            });
        }
        None => {
            let err = payload
                .codepool_error
                .clone()
                .unwrap_or_else(|| "CODEPOOL_BASE_URL not set".to_string());
            out.push(DeployRow {
                component: "Codepool".to_string(),
                git_sha: err,
                semver: "—".to_string(),
                built_at: "—".to_string(),
                ghcr_tags: payload
                    .ghcr_codepool
                    .as_ref()
                    .map(|g| g.tags.join(", "))
                    .unwrap_or_else(|| "—".to_string()),
                ghcr_updated: payload
                    .ghcr_codepool
                    .as_ref()
                    .map(|g| g.updated_at.clone())
                    .unwrap_or_else(|| "—".to_string()),
                in_sync: "—".to_string(),
            });
        }
    }
    out
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
    (status, Json(payload))
}

async fn gather_status(state: &AppState) -> StatusPayload {
    let health = stack_health::gather(state).await;
    let den_version = build_info::snapshot();

    let (codepool_version, codepool_error) = if state.codepool.is_enabled() {
        match state.codepool.fetch_version_json().await {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(v) => (Some(v), None),
                Err(e) => (
                    None,
                    Some(format!("invalid JSON from Codepool /version: {e}")),
                ),
            },
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };

    let cfg = state.config.as_ref();
    let mut ghcr_config_note: Option<String> = None;
    let (ghcr_den, ghcr_codepool) = if cfg.github_packages_token.trim().is_empty()
        || cfg.ghcr_packages_owner.trim().is_empty()
    {
        ghcr_config_note = Some(
                "Set GITHUB_PACKAGES_TOKEN and GHCR_PACKAGES_OWNER to compare running images with GHCR."
                    .to_string(),
            );
        (None, None)
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
                let (c, err_c) = fetch_ghcr_package(&client, token, kind, owner, "codepool").await;
                let mut notes = Vec::new();
                if let Some(e) = err_d {
                    notes.push(format!("den GHCR: {e}"));
                }
                if let Some(e) = err_c {
                    notes.push(format!("codepool GHCR: {e}"));
                }
                if !notes.is_empty() {
                    ghcr_config_note = Some(notes.join(" "));
                }
                (d, c)
            }
            Err(e) => {
                ghcr_config_note = Some(format!("Could not build HTTP client: {e}"));
                (None, None)
            }
        }
    };

    StatusPayload {
        health,
        den_version,
        codepool_version,
        codepool_error,
        ghcr_den,
        ghcr_codepool,
        ghcr_config_note,
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
            return (None, Some(format!("request failed: {e}")));
        }
    };
    let status_code = resp.status();
    if !status_code.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        return (None, Some(format!("HTTP {status_code} — {txt}")));
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
