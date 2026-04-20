//! Aggregated **BEARS stack** health for operators and monitors.
//!
//! Combines **runtime probes** (PostgreSQL, upstream HTTP) with **low-cost config sanity**
//! aligned with the repo’s **`services/preflight`** script: JWT when
//! required, `LETTA_PG_URI` / `LLM_API_URL` shape, and `OPENAI_API_KEY` presence warnings.

use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use time::OffsetDateTime;
use tokio::time::timeout;
use url::Url;

use crate::startup;
use crate::web::AppState;

/// Wall-clock timeout for each upstream HTTP health call (Letta, Codepool, Bifrost).
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Serialize)]
pub struct BearsHealthReport {
    /// `true` when no check has [`CheckState::Fail`]. Warnings still yield `true`.
    pub ok: bool,
    pub checked_at: String,
    pub checks: Vec<HealthCheck>,
}

#[derive(Clone, Serialize)]
pub struct HealthCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub state: CheckState,
    pub detail: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Ok,
    Warn,
    Fail,
    Skipped,
}

impl BearsHealthReport {
    fn from_checks(checks: Vec<HealthCheck>) -> Self {
        let ok = checks.iter().all(|c| c.state != CheckState::Fail);
        let checked_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string());
        Self {
            ok,
            checked_at,
            checks,
        }
    }
}

#[derive(Serialize)]
struct BearsHealthRow {
    id: &'static str,
    label: &'static str,
    state: &'static str,
    detail: String,
}

pub async fn page(State(state): State<AppState>) -> Result<Response, crate::errors::CustomError> {
    let report = gather(&state).await;
    let status = if report.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let rows: Vec<BearsHealthRow> = report
        .checks
        .iter()
        .map(|c| BearsHealthRow {
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
    let ctx = minijinja::context! {
        title => "BEARS health",
        template_tag => "page-bears-health",
        app_display_name => state.config.app_display_name.clone(),
        app_slug => state.config.app_slug.clone(),
        public_web_origin => state.config.web_public_origin(),
        rows => rows,
        overall_ok => report.ok,
        checked_at => report.checked_at.clone(),
        json_path => "/health/bears.json",
    };
    let template = state
        .template_env
        .get_template("bears_health.html")
        .map_err(|e| crate::errors::CustomError::Render(format!("bears_health template: {e}")))?;
    let body = template
        .render(ctx)
        .map_err(|e| crate::errors::CustomError::Render(format!("bears_health render: {e}")))?;
    Ok((status, Html(body)).into_response())
}

pub async fn json_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    let report = gather(&state).await;
    let status = if report.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(report))
}

pub async fn gather(state: &AppState) -> BearsHealthReport {
    let cfg = state.config.as_ref();

    let mut checks: Vec<HealthCheck> = Vec::new();

    checks.push(jwt_check(cfg));
    checks.push(den_database_url_shape(cfg));
    if !cfg.letta_pg_uri.is_empty() {
        checks.push(letta_pg_uri_shape(&cfg.letta_pg_uri));
    }
    if let Some(c) = llm_api_url_shape() {
        checks.push(c);
    }
    checks.push(openai_key_warn());
    checks.push(web_server_url_shape(cfg));

    let (
        den_pg,
        letta_pg,
        codepool_h,
        letta_h,
        bifrost_h,
    ) = tokio::join!(
        check_den_postgres(state.sqlx_pool()),
        check_letta_postgres(&cfg.letta_pg_uri),
        check_codepool(&state),
        check_letta_api(&state),
        check_bifrost_http(&cfg.bifrost_base_url),
    );

    checks.push(den_pg);
    checks.push(letta_pg);
    checks.push(codepool_h);
    checks.push(letta_h);
    checks.push(bifrost_h);

    BearsHealthReport::from_checks(checks)
}

fn jwt_check(cfg: &crate::config::Config) -> HealthCheck {
    if !startup::requires_jwt_secret(cfg) {
        return HealthCheck {
            id: "jwt_secret",
            label: "JWT_SECRET",
            state: CheckState::Skipped,
            detail: "not required for this build/run mode".into(),
        };
    }
    let secret = std::env::var("JWT_SECRET").unwrap_or_default();
    if secret.trim().is_empty() {
        HealthCheck {
            id: "jwt_secret",
            label: "JWT_SECRET",
            state: CheckState::Fail,
            detail: "empty but required (production build or RUN_API); preflight also requires this"
                .into(),
        }
    } else {
        HealthCheck {
            id: "jwt_secret",
            label: "JWT_SECRET",
            state: CheckState::Ok,
            detail: "set".into(),
        }
    }
}

fn den_database_url_shape(cfg: &crate::config::Config) -> HealthCheck {
    match Url::parse(cfg.database_url.trim()) {
        Ok(u) => {
            let scheme = u.scheme();
            if matches!(scheme, "postgres" | "postgresql") {
                if u.host_str().is_none() {
                    HealthCheck {
                        id: "database_url_shape",
                        label: "DATABASE_URL (shape)",
                        state: CheckState::Warn,
                        detail: "missing host (preflight requires a hostname)".into(),
                    }
                } else {
                    HealthCheck {
                        id: "database_url_shape",
                        label: "DATABASE_URL (shape)",
                        state: CheckState::Ok,
                        detail: "PostgreSQL URI with host".into(),
                    }
                }
            } else {
                HealthCheck {
                    id: "database_url_shape",
                    label: "DATABASE_URL (shape)",
                    state: CheckState::Warn,
                    detail: format!("expected postgres:// or postgresql://, got scheme {scheme:?}"),
                }
            }
        }
        Err(e) => HealthCheck {
            id: "database_url_shape",
            label: "DATABASE_URL (shape)",
            state: CheckState::Warn,
            detail: format!("parse error: {e}"),
        },
    }
}

fn letta_pg_uri_shape(uri: &str) -> HealthCheck {
    match Url::parse(uri) {
        Ok(u) => {
            if u.scheme() == "postgres" {
                HealthCheck {
                    id: "letta_pg_uri_shape",
                    label: "LETTA_PG_URI (shape)",
                    state: CheckState::Warn,
                    detail: "uses postgres:// — use postgresql:// for Alembic/SQLAlchemy (see services/letta/COOLIFY_DEPLOY.md; preflight fails on this)".into(),
                }
            } else if u.scheme() == "postgresql" {
                HealthCheck {
                    id: "letta_pg_uri_shape",
                    label: "LETTA_PG_URI (shape)",
                    state: CheckState::Ok,
                    detail: "scheme acceptable".into(),
                }
            } else {
                HealthCheck {
                    id: "letta_pg_uri_shape",
                    label: "LETTA_PG_URI (shape)",
                    state: CheckState::Warn,
                    detail: format!("expected postgres or postgresql scheme, got {:?}", u.scheme()),
                }
            }
        }
        Err(e) => HealthCheck {
            id: "letta_pg_uri_shape",
            label: "LETTA_PG_URI (shape)",
            state: CheckState::Warn,
            detail: format!("parse error: {e}"),
        },
    }
}

fn llm_api_url_shape() -> Option<HealthCheck> {
    let raw = std::env::var("LLM_API_URL").ok()?;
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    Some(match Url::parse(v) {
        Ok(u) => {
            let ok = (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some();
            if ok {
                HealthCheck {
                    id: "llm_api_url_shape",
                    label: "LLM_API_URL (shape)",
                    state: CheckState::Ok,
                    detail: "valid http(s) URL (Letta → Bifrost; mirrors preflight)".into(),
                }
            } else {
                HealthCheck {
                    id: "llm_api_url_shape",
                    label: "LLM_API_URL (shape)",
                    state: CheckState::Warn,
                    detail: "must be http(s) with a host (see preflight)".into(),
                }
            }
        }
        Err(e) => HealthCheck {
            id: "llm_api_url_shape",
            label: "LLM_API_URL (shape)",
            state: CheckState::Warn,
            detail: format!("parse error: {e}"),
        },
    })
}

fn openai_key_warn() -> HealthCheck {
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.trim().is_empty() {
        HealthCheck {
            id: "openai_api_key",
            label: "OPENAI_API_KEY",
            state: CheckState::Warn,
            detail: "empty — embeddings and direct OpenAI calls may fail (preflight warns similarly)"
                .into(),
        }
    } else {
        HealthCheck {
            id: "openai_api_key",
            label: "OPENAI_API_KEY",
            state: CheckState::Ok,
            detail: "set".into(),
        }
    }
}

fn web_server_url_shape(cfg: &crate::config::Config) -> HealthCheck {
    match Url::parse(cfg.web_server_url.trim()) {
        Ok(u) => {
            let ok =
                (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some();
            if ok {
                HealthCheck {
                    id: "web_server_url_shape",
                    label: "WEB_SERVER_URL (shape)",
                    state: CheckState::Ok,
                    detail: "valid http(s) URL (preflight)".into(),
                }
            } else {
                HealthCheck {
                    id: "web_server_url_shape",
                    label: "WEB_SERVER_URL (shape)",
                    state: CheckState::Warn,
                    detail: "expected http(s) with a host".into(),
                }
            }
        }
        Err(e) => HealthCheck {
            id: "web_server_url_shape",
            label: "WEB_SERVER_URL (shape)",
            state: CheckState::Warn,
            detail: format!("parse error: {e}"),
        },
    }
}

async fn check_den_postgres(pool: &PgPool) -> HealthCheck {
    match sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool).await {
        Ok(_) => HealthCheck {
            id: "den_postgres",
            label: "Den PostgreSQL",
            state: CheckState::Ok,
            detail: "SELECT 1 on DATABASE_URL pool succeeded".into(),
        },
        Err(e) => HealthCheck {
            id: "den_postgres",
            label: "Den PostgreSQL",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
    }
}

async fn check_letta_postgres(uri: &str) -> HealthCheck {
    if uri.is_empty() {
        return HealthCheck {
            id: "letta_postgres",
            label: "Letta PostgreSQL",
            state: CheckState::Skipped,
            detail: "LETTA_PG_URI unset — optional; Letta may use embedded defaults (preflight)".into(),
        };
    }

    let pool = match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(4))
        .connect(uri)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return HealthCheck {
                id: "letta_postgres",
                label: "Letta PostgreSQL",
                state: CheckState::Fail,
                detail: format!("connect: {e}"),
            };
        }
    };

    let result = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await;
    pool.close().await;
    match result {
        Ok(_) => HealthCheck {
            id: "letta_postgres",
            label: "Letta PostgreSQL",
            state: CheckState::Ok,
            detail: "SELECT 1 on LETTA_PG_URI succeeded".into(),
        },
        Err(e) => HealthCheck {
            id: "letta_postgres",
            label: "Letta PostgreSQL",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
    }
}

async fn check_codepool(state: &AppState) -> HealthCheck {
    if !state.codepool.is_enabled() {
        return HealthCheck {
            id: "codepool",
            label: "Codepool",
            state: CheckState::Skipped,
            detail: "CODEPOOL_BASE_URL empty".into(),
        };
    }
    match timeout(
        HTTP_PROBE_TIMEOUT,
        state.codepool.check_health(),
    )
    .await
    {
        Err(_) => HealthCheck {
            id: "codepool",
            label: "Codepool",
            state: CheckState::Fail,
            detail: format!("timeout after {}s (GET /health)", HTTP_PROBE_TIMEOUT.as_secs()),
        },
        Ok(Ok(body)) => HealthCheck {
            id: "codepool",
            label: "Codepool",
            state: CheckState::Ok,
            detail: truncate_detail(body),
        },
        Ok(Err(e)) => HealthCheck {
            id: "codepool",
            label: "Codepool",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
    }
}

async fn check_letta_api(state: &AppState) -> HealthCheck {
    if !state.letta.is_enabled() {
        return HealthCheck {
            id: "letta_api",
            label: "Letta API",
            state: CheckState::Skipped,
            detail: "LETTA_BASE_URL empty".into(),
        };
    }
    match timeout(HTTP_PROBE_TIMEOUT, state.letta.check_health()).await {
        Err(_) => HealthCheck {
            id: "letta_api",
            label: "Letta API",
            state: CheckState::Fail,
            detail: format!("timeout after {}s (GET /v1/health)", HTTP_PROBE_TIMEOUT.as_secs()),
        },
        Ok(Ok(body)) => HealthCheck {
            id: "letta_api",
            label: "Letta API",
            state: CheckState::Ok,
            detail: truncate_detail(body),
        },
        Ok(Err(e)) => HealthCheck {
            id: "letta_api",
            label: "Letta API",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
    }
}

async fn check_bifrost_http(base: &str) -> HealthCheck {
    if base.trim().is_empty() {
        return HealthCheck {
            id: "bifrost",
            label: "Bifrost",
            state: CheckState::Skipped,
            detail: "BIFROST_BASE_URL unset — set e.g. http://bear-bifrost:8080 to probe the gateway"
                .into(),
        };
    }

    let client = match reqwest::Client::builder()
        .timeout(HTTP_PROBE_TIMEOUT)
        .connect_timeout(Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HealthCheck {
                id: "bifrost",
                label: "Bifrost",
                state: CheckState::Fail,
                detail: format!("reqwest client: {e}"),
            };
        }
    };

    let url = format!("{}/health", base.trim_end_matches('/'));
    match timeout(HTTP_PROBE_TIMEOUT, client.get(&url).send()).await {
        Err(_) => HealthCheck {
            id: "bifrost",
            label: "Bifrost",
            state: CheckState::Fail,
            detail: format!("timeout after {}s ({url})", HTTP_PROBE_TIMEOUT.as_secs()),
        },
        Ok(Err(e)) => HealthCheck {
            id: "bifrost",
            label: "Bifrost",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
        Ok(Ok(resp)) => {
            let status = resp.status();
            if status.is_success() {
                HealthCheck {
                    id: "bifrost",
                    label: "Bifrost",
                    state: CheckState::Ok,
                    detail: format!("HTTP {status} from {url}"),
                }
            } else {
                HealthCheck {
                    id: "bifrost",
                    label: "Bifrost",
                    state: CheckState::Fail,
                    detail: format!("HTTP {status} from {url}"),
                }
            }
        }
    }
}

fn truncate_detail(s: String) -> String {
    const MAX: usize = 240;
    if s.len() <= MAX {
        return s;
    }
    format!("{}…", &s[..MAX.saturating_sub(1)])
}
