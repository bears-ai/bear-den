//! Runtime probes and config checks for the public **`/status`** page ([`crate::web::status`]).
//!
//! Combines **runtime probes** (PostgreSQL, upstream HTTP) with **low-cost config sanity**
//! aligned with the repo’s **`services/preflight`** script: JWT when
//! required, `LLM_API_URL` shape, and provider key presence warnings.

use std::time::Duration;

use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::time::timeout;
use url::Url;

use crate::config::Config;
use crate::web::AppState;

/// Wall-clock timeout for each upstream HTTP health call.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
    if let Some(c) = llm_api_url_shape() {
        checks.push(c);
    }
    checks.push(openai_key_warn());
    checks.push(web_server_url_shape(cfg));

    let den_pg = check_den_postgres(state.sqlx_pool()).await;
    let bifrost_h = check_bifrost_http(&cfg.bifrost_base_url).await;
    let bifrost_management_auth = check_bifrost_management_auth(&cfg.bifrost_management_url).await;

    checks.push(den_pg);
    checks.push(bifrost_h);
    checks.push(HealthCheck {
        id: "bifrost_models",
        label: "Bifrost models",
        state: CheckState::Skipped,
        detail:
            "global /v1/models probe disabled; BEARS requires Bear-scoped x-bf-vk for model listing"
                .into(),
    });
    checks.push(bifrost_management_auth);
    checks.push(check_qdrant(cfg).await);

    StackHealthReport::from_checks(checks)
}

fn jwt_check(cfg: &crate::config::Config) -> HealthCheck {
    if !den_core::config::requires_jwt_secret(cfg) {
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
                    detail: "valid http(s) URL for Bifrost".into(),
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
    match sqlx::query_scalar!("SELECT 1").fetch_one(pool).await {
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

async fn check_bifrost_http(base: &str) -> HealthCheck {
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
            HealthCheck {
                id: "bifrost",
                label: "Bifrost",
                state: if status.is_success() {
                    CheckState::Ok
                } else {
                    CheckState::Fail
                },
                detail: format!("HTTP {status} from {url}"),
            }
        }
    }
}

#[allow(dead_code)]
async fn check_bifrost_live_models(llm_api_url: &str) -> HealthCheck {
    let base = llm_api_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return HealthCheck {
            id: "bifrost_models",
            label: "Bifrost models",
            state: CheckState::Skipped,
            detail: "LLM_API_URL unset — cannot probe Bifrost /v1/models".into(),
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
                id: "bifrost_models",
                label: "Bifrost models",
                state: CheckState::Fail,
                detail: format!("reqwest client: {e}"),
            };
        }
    };

    let url = format!("{base}/models");
    match timeout(HTTP_PROBE_TIMEOUT, client.get(&url).send()).await {
        Err(_) => HealthCheck {
            id: "bifrost_models",
            label: "Bifrost models",
            state: CheckState::Fail,
            detail: format!("timeout after {}s ({url})", HTTP_PROBE_TIMEOUT.as_secs()),
        },
        Ok(Err(e)) => HealthCheck {
            id: "bifrost_models",
            label: "Bifrost models",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
        Ok(Ok(resp)) => {
            let status = resp.status();
            let text = match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    return HealthCheck {
                        id: "bifrost_models",
                        label: "Bifrost models",
                        state: CheckState::Fail,
                        detail: format!("/v1/models body failed: {e}"),
                    };
                }
            };
            if !status.is_success() {
                return HealthCheck {
                    id: "bifrost_models",
                    label: "Bifrost models",
                    state: CheckState::Fail,
                    detail: format!("HTTP {status} from {url}; upstream response body withheld."),
                };
            }
            let value: serde_json::Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(e) => {
                    return HealthCheck {
                        id: "bifrost_models",
                        label: "Bifrost models",
                        state: CheckState::Fail,
                        detail: format!(
                            "/v1/models JSON parse failed: {e}; upstream response body withheld."
                        ),
                    };
                }
            };
            let (usable_count, wildcard_count) = count_usable_bifrost_models(&value);
            if usable_count == 0 {
                HealthCheck {
                    id: "bifrost_models",
                    label: "Bifrost models",
                    state: CheckState::Fail,
                    detail: format!(
                        "/v1/models returned no concrete usable models (wildcard entries filtered: {wildcard_count})"
                    ),
                }
            } else {
                HealthCheck {
                    id: "bifrost_models",
                    label: "Bifrost models",
                    state: CheckState::Ok,
                    detail: format!(
                        "/v1/models advertises {usable_count} concrete usable model(s); wildcard entries filtered: {wildcard_count}"
                    ),
                }
            }
        }
    }
}

async fn check_bifrost_management_auth(management_url: &str) -> HealthCheck {
    let base = management_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return HealthCheck {
            id: "bifrost_management_auth",
            label: "Bifrost management auth",
            state: CheckState::Skipped,
            detail: "BIFROST_MANAGEMENT_URL unset — cannot verify virtual-key management auth"
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
                id: "bifrost_management_auth",
                label: "Bifrost management auth",
                state: CheckState::Fail,
                detail: format!("reqwest client: {e}"),
            };
        }
    };

    let url = format!("{base}/config");
    match timeout(HTTP_PROBE_TIMEOUT, client.get(&url).send()).await {
        Err(_) => HealthCheck {
            id: "bifrost_management_auth",
            label: "Bifrost management auth",
            state: CheckState::Fail,
            detail: format!("timeout after {}s ({url})", HTTP_PROBE_TIMEOUT.as_secs()),
        },
        Ok(Err(e)) => HealthCheck {
            id: "bifrost_management_auth",
            label: "Bifrost management auth",
            state: CheckState::Fail,
            detail: e.to_string(),
        },
        Ok(Ok(resp)) => {
            let status = resp.status();
            let text = match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    return HealthCheck {
                        id: "bifrost_management_auth",
                        label: "Bifrost management auth",
                        state: CheckState::Fail,
                        detail: format!("/api/config body failed: {e}"),
                    };
                }
            };

            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                if text.contains("Authentication is not enabled") {
                    return stale_bifrost_auth_config_check(format!(
                        "HTTP {status} from {url}: Bifrost reports that management authentication is not enabled."
                    ));
                }
                return HealthCheck {
                    id: "bifrost_management_auth",
                    label: "Bifrost management auth",
                    state: CheckState::Ok,
                    detail: format!(
                        "/api/config is protected (HTTP {status}); management auth appears enabled"
                    ),
                };
            }

            if !status.is_success() {
                return HealthCheck {
                    id: "bifrost_management_auth",
                    label: "Bifrost management auth",
                    state: CheckState::Fail,
                    detail: format!("HTTP {status} from {url}; upstream response body withheld."),
                };
            }

            let value: serde_json::Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(e) => {
                    return HealthCheck {
                        id: "bifrost_management_auth",
                        label: "Bifrost management auth",
                        state: CheckState::Fail,
                        detail: format!(
                            "/api/config JSON parse failed: {e}; upstream response body withheld."
                        ),
                    };
                }
            };
            bifrost_management_auth_config_check_from_value(&value)
        }
    }
}

fn bifrost_management_auth_config_check_from_value(value: &serde_json::Value) -> HealthCheck {
    let auth_enabled = value
        .get("auth_config")
        .or_else(|| {
            value
                .get("governance")
                .and_then(|governance| governance.get("auth_config"))
        })
        .and_then(|auth_config| auth_config.get("is_enabled"))
        .and_then(serde_json::Value::as_bool);

    match auth_enabled
    {
        Some(true) => HealthCheck {
            id: "bifrost_management_auth",
            label: "Bifrost management auth",
            state: CheckState::Ok,
            detail: "runtime /api/config reports auth_config.is_enabled=true; virtual-key management login should be available".into(),
        },
        Some(false) => stale_bifrost_auth_config_check(
            "runtime /api/config reports auth_config.is_enabled=false".to_string(),
        ),
        None => HealthCheck {
            id: "bifrost_management_auth",
            label: "Bifrost management auth",
            state: CheckState::Fail,
            detail: "runtime /api/config does not expose auth_config.is_enabled or governance.auth_config.is_enabled; Den cannot verify Bifrost virtual-key management auth".into(),
        },
    }
}

fn stale_bifrost_auth_config_check(observed: String) -> HealthCheck {
    HealthCheck {
        id: "bifrost_management_auth",
        label: "Bifrost management auth",
        state: CheckState::Fail,
        detail: format!(
            "{observed}. Bifrost virtual-key provisioning will fail. If services/bifrost/config.json has governance.auth_config.is_enabled=true, Bifrost is likely serving stale config-store state from /app/data/config.db or reading a different config file; recreate/reset bears-bifrost config.db after redeploying the corrected image."
        ),
    }
}

#[allow(dead_code)]
fn count_usable_bifrost_models(value: &serde_json::Value) -> (usize, usize) {
    let Some(items) = value.get("data").and_then(|data| data.as_array()) else {
        return (0, 0);
    };
    let mut usable = 0;
    let mut wildcard = 0;
    for item in items {
        let Some(id) = item.get("id").and_then(|id| id.as_str()).map(str::trim) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        if den_llm::model_registry::is_routing_wildcard_model_handle(id) {
            wildcard += 1;
        } else {
            usable += 1;
        }
    }
    (usable, wildcard)
}

/// Probe the derived recall index (Qdrant). Recall is optional and derived (ADR-0038), so an
/// unreachable store is a **warning** (Den degrades to keyword search) rather than a hard
/// failure that would 503 the whole stack. Idempotently ensures the recall collection so the
/// status surface self-heals startup races against the `recall` compose profile.
async fn check_qdrant(config: &Config) -> HealthCheck {
    let Some(recall) = den_service::recall::QdrantRecall::from_config(config) else {
        return HealthCheck {
            id: "qdrant",
            label: "Qdrant recall",
            state: CheckState::Skipped,
            detail: "QDRANT_URL unset — derived recall disabled (keyword fallback)".into(),
        };
    };

    match timeout(HTTP_PROBE_TIMEOUT, recall.ensure_collection()).await {
        Ok(Ok(created)) => HealthCheck {
            id: "qdrant",
            label: "Qdrant recall",
            state: CheckState::Ok,
            detail: if created {
                format!(
                    "reachable at {}; collection {} created",
                    recall.base_url(),
                    recall.collection_name()
                )
            } else {
                format!(
                    "reachable at {}; collection {} present",
                    recall.base_url(),
                    recall.collection_name()
                )
            },
        },
        Ok(Err(e)) => HealthCheck {
            id: "qdrant",
            label: "Qdrant recall",
            state: CheckState::Warn,
            detail: format!("{e} — recall degraded to keyword fallback"),
        },
        Err(_) => HealthCheck {
            id: "qdrant",
            label: "Qdrant recall",
            state: CheckState::Warn,
            detail: format!(
                "timeout after {}s probing {} — recall degraded to keyword fallback",
                HTTP_PROBE_TIMEOUT.as_secs(),
                recall.base_url()
            ),
        },
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "stack_health/status_probe_tests.rs"]
mod status_probe_tests;
