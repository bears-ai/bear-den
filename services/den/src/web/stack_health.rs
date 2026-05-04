//! Runtime probes and config checks for the public **`/status`** page ([`crate::web::status`]).
//!
//! Combines **runtime probes** (PostgreSQL, upstream HTTP) with **low-cost config sanity**
//! aligned with the repo’s **`services/preflight`** script: JWT when
//! required, `LETTA_PG_URI` / `LLM_API_URL` shape, and `OPENAI_API_KEY` presence warnings.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use time::OffsetDateTime;
use tokio::time::timeout;
use url::Url;

use crate::startup;
use crate::web::AppState;

/// Wall-clock timeout for each upstream HTTP health call (Letta, Codepool, Bifrost).
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Serialize)]
pub struct StackHealthReport {
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Ok,
    Warn,
    Fail,
    Skipped,
}

impl StackHealthReport {
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

/// Row shape for the `/status` HTML table.
#[derive(Serialize)]
pub struct StackHealthTemplateRow {
    pub id: &'static str,
    pub label: &'static str,
    pub state: &'static str,
    pub detail: String,
}

pub async fn gather(state: &AppState) -> StackHealthReport {
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

    let (den_pg, letta_pg, codepool_h, letta_h, bifrost_h, memfs_views_h) = tokio::join!(
        check_den_postgres(state.sqlx_pool()),
        check_letta_postgres(&cfg.letta_pg_uri),
        check_codepool(&state),
        check_letta_api(&state),
        check_bifrost_http(&cfg.bifrost_base_url, &cfg.bifrost_metadata_url),
        check_memfs_sidecar_views(&cfg.letta_memfs_service_url),
    );

    checks.push(den_pg);
    checks.push(letta_pg);
    checks.push(codepool_h);
    checks.push(letta_h);
    checks.push(bifrost_h);
    checks.push(memfs_views_h);

    StackHealthReport::from_checks(checks)
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
            detail:
                "empty but required (production build or RUN_API); preflight also requires this"
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
                    detail: format!(
                        "expected postgres or postgresql scheme, got {:?}",
                        u.scheme()
                    ),
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
            detail:
                "empty — embeddings and direct OpenAI calls may fail (preflight warns similarly)"
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
            let ok = (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some();
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
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
    {
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
            detail: "LETTA_PG_URI unset — optional; Letta may use embedded defaults (preflight)"
                .into(),
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
    match timeout(HTTP_PROBE_TIMEOUT, state.codepool.check_health()).await {
        Err(_) => HealthCheck {
            id: "codepool",
            label: "Codepool",
            state: CheckState::Fail,
            detail: format!(
                "timeout after {}s (GET /health)",
                HTTP_PROBE_TIMEOUT.as_secs()
            ),
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
            detail: format!(
                "timeout after {}s (GET /v1/health)",
                HTTP_PROBE_TIMEOUT.as_secs()
            ),
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

async fn check_memfs_sidecar_views(base: &str) -> HealthCheck {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return HealthCheck {
            id: "memfs_views",
            label: "MemFS role views",
            state: CheckState::Skipped,
            detail: "LETTA_MEMFS_SERVICE_URL empty".into(),
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
                id: "memfs_views",
                label: "MemFS role views",
                state: CheckState::Fail,
                detail: format!("reqwest client: {e}"),
            };
        }
    };
    let url = format!("{base}/v1/management/bears");
    let resp = match timeout(HTTP_PROBE_TIMEOUT, client.get(&url).send()).await {
        Err(_) => {
            return HealthCheck {
                id: "memfs_views",
                label: "MemFS role views",
                state: CheckState::Fail,
                detail: format!("timeout after {}s ({url})", HTTP_PROBE_TIMEOUT.as_secs()),
            };
        }
        Ok(Err(e)) => {
            return HealthCheck {
                id: "memfs_views",
                label: "MemFS role views",
                state: CheckState::Fail,
                detail: e.to_string(),
            };
        }
        Ok(Ok(resp)) => resp,
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return HealthCheck {
            id: "memfs_views",
            label: "MemFS role views",
            state: CheckState::Fail,
            detail: format!("HTTP {status} from {url}: {}", truncate_detail(text)),
        };
    }
    let value: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return HealthCheck {
                id: "memfs_views",
                label: "MemFS role views",
                state: CheckState::Fail,
                detail: format!("JSON parse failed: {e}"),
            };
        }
    };
    let views = value
        .get("views")
        .and_then(|v| v.as_object())
        .map(|m| m.values().collect::<Vec<_>>())
        .unwrap_or_default();
    if views.is_empty() {
        return HealthCheck {
            id: "memfs_views",
            label: "MemFS role views",
            state: CheckState::Warn,
            detail: "sidecar reachable but no registered role views".into(),
        };
    }
    let mut quarantined = 0usize;
    let mut drift = 0usize;
    let mut missing = 0usize;
    let mut errors = 0usize;
    let mut stale = 0usize;
    let now = OffsetDateTime::now_utc().unix_timestamp() as f64;
    for view in &views {
        let state = view
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if view
            .get("quarantined")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || state == "quarantined"
        {
            quarantined += 1;
        }
        if state == "drift" {
            drift += 1;
        }
        if matches!(state, "missing_view" | "missing_canonical") {
            missing += 1;
        }
        if matches!(state, "error" | "git_error") {
            errors += 1;
        }
        if let Some(ts) = view.get("last_reconciled_at").and_then(|v| v.as_f64()) {
            if now - ts > 600.0 {
                stale += 1;
            }
        }
    }
    let total = views.len();
    let detail = format!(
        "{total} view(s); quarantined={quarantined}, drift={drift}, missing={missing}, stale_reconcile={stale}, errors={errors}"
    );
    let state = if quarantined > 0 || missing > 0 || errors > 0 {
        CheckState::Fail
    } else if drift > 0 || stale > 0 {
        CheckState::Warn
    } else {
        CheckState::Ok
    };
    HealthCheck {
        id: "memfs_views",
        label: "MemFS role views",
        state,
        detail,
    }
}

async fn check_bifrost_http(base: &str, metadata_url: &str) -> HealthCheck {
    if base.trim().is_empty() {
        return HealthCheck {
            id: "bifrost",
            label: "Bifrost",
            state: CheckState::Skipped,
            detail:
                "BIFROST_BASE_URL unset — set e.g. http://bears-bifrost:8080 to probe the gateway"
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
                let metadata_detail = check_bifrost_metadata(&client, metadata_url).await;
                HealthCheck {
                    id: "bifrost",
                    label: "Bifrost",
                    state: metadata_detail.0,
                    detail: format!("HTTP {status} from {url}; {}", metadata_detail.1),
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

async fn check_bifrost_metadata(
    client: &reqwest::Client,
    metadata_url: &str,
) -> (CheckState, String) {
    let url = metadata_url.trim();
    if url.is_empty() {
        return (
            CheckState::Warn,
            "BIFROST_METADATA_URL unset — model context windows cannot be verified".into(),
        );
    }

    match timeout(HTTP_PROBE_TIMEOUT, client.get(url).send()).await {
        Err(_) => (
            CheckState::Warn,
            format!(
                "metadata timeout after {}s ({url})",
                HTTP_PROBE_TIMEOUT.as_secs()
            ),
        ),
        Ok(Err(e)) => (CheckState::Warn, format!("metadata request failed: {e}")),
        Ok(Ok(resp)) => {
            let status = resp.status();
            if !status.is_success() {
                return (
                    CheckState::Warn,
                    format!("metadata HTTP {status} from {url}"),
                );
            }
            let text = match resp.text().await {
                Ok(t) => t,
                Err(e) => return (CheckState::Warn, format!("metadata body failed: {e}")),
            };
            let value: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => return (CheckState::Warn, format!("metadata JSON parse failed: {e}")),
            };
            let models = value
                .get("models")
                .and_then(|x| x.as_array())
                .map(|xs| {
                    xs.iter()
                        .filter(|m| m.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true))
                        .count()
                })
                .unwrap_or(0);
            if models == 0 {
                (
                    CheckState::Warn,
                    "metadata returned no enabled models".into(),
                )
            } else {
                (
                    CheckState::Ok,
                    format!("metadata OK ({models} enabled models)"),
                )
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
