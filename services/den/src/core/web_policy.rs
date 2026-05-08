use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebApprovalDecision {
    Preferred,
    Allowed,
    ApprovedUrl,
    ApprovedHost,
    Blocked,
    RequiresApproval,
}

impl WebApprovalDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preferred => "preferred",
            Self::Allowed => "allowed",
            Self::ApprovedUrl => "user_url",
            Self::ApprovedHost => "user_host",
            Self::Blocked => "denied",
            Self::RequiresApproval => "requires_approval",
        }
    }

    pub fn is_approved(&self) -> bool {
        matches!(
            self,
            Self::Preferred | Self::Allowed | Self::ApprovedUrl | Self::ApprovedHost
        )
    }
}

#[derive(Debug, Clone)]
pub struct NormalizedWebUrl {
    pub url: String,
    pub host: String,
}

pub fn normalize_web_url(raw: &str) -> Result<NormalizedWebUrl, CustomError> {
    let mut url = url::Url::parse(raw.trim()).map_err(|err| {
        CustomError::ValidationError(format!("url must be a valid HTTP(S) URL: {err}"))
    })?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(CustomError::ValidationError(
                "url scheme must be http or https".to_string(),
            ));
        }
    }
    url.set_fragment(None);
    let host = normalize_host_for_url(&url)?;
    Ok(NormalizedWebUrl {
        url: url.to_string(),
        host,
    })
}

pub fn normalize_host_for_url(url: &url::Url) -> Result<String, CustomError> {
    let host = url
        .host_str()
        .ok_or_else(|| CustomError::ValidationError("url must include a host".to_string()))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    Ok(match url.port() {
        Some(port)
            if !((url.scheme() == "https" && port == 443)
                || (url.scheme() == "http" && port == 80)) =>
        {
            format!("{host}:{port}")
        }
        _ => host,
    })
}

pub async fn decide_web_fetch_approval(
    pool: &PgPool,
    bear_id: Uuid,
    raw_url: &str,
) -> Result<(NormalizedWebUrl, WebApprovalDecision), CustomError> {
    let normalized = normalize_web_url(raw_url)?;
    if let Some(policy) = source_policy(pool, bear_id, &normalized).await? {
        return Ok((
            normalized,
            match policy.as_str() {
                "preferred" => WebApprovalDecision::Preferred,
                "allowed" => WebApprovalDecision::Allowed,
                "blocked" => WebApprovalDecision::Blocked,
                _ => WebApprovalDecision::RequiresApproval,
            },
        ));
    }
    if approval_exists(pool, bear_id, "url", &normalized.url).await? {
        return Ok((normalized, WebApprovalDecision::ApprovedUrl));
    }
    if approval_exists(pool, bear_id, "host", &normalized.host).await? {
        return Ok((normalized, WebApprovalDecision::ApprovedHost));
    }
    Ok((normalized, WebApprovalDecision::RequiresApproval))
}

async fn source_policy(
    pool: &PgPool,
    bear_id: Uuid,
    normalized: &NormalizedWebUrl,
) -> Result<Option<String>, CustomError> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT policy
        FROM bear_web_sources
        WHERE bear_id = $1
          AND ((scope_kind = 'url' AND scope_value = $2)
            OR (scope_kind = 'host' AND scope_value = $3))
        ORDER BY CASE scope_kind WHEN 'url' THEN 0 ELSE 1 END,
                 CASE policy WHEN 'blocked' THEN 0 WHEN 'preferred' THEN 1 ELSE 2 END,
                 priority DESC
        LIMIT 1
        "#,
    )
    .bind(bear_id)
    .bind(&normalized.url)
    .bind(&normalized.host)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

async fn approval_exists(
    pool: &PgPool,
    bear_id: Uuid,
    scope_kind: &str,
    scope_value: &str,
) -> Result<bool, CustomError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM bear_web_approvals
            WHERE bear_id = $1
              AND scope_kind = $2
              AND scope_value = $3
              AND revoked_at IS NULL
              AND (expires_at IS NULL OR expires_at > now())
        )
        "#,
    )
    .bind(bear_id)
    .bind(scope_kind)
    .bind(scope_value)
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

pub fn is_local_web_url(normalized: &NormalizedWebUrl) -> bool {
    local_web_hosts().iter().any(|host| {
        host == &normalized.host || host == normalized.host.split(':').next().unwrap_or("")
    })
}

pub fn local_web_hosts() -> Vec<String> {
    std::env::var("BEARS_LOCAL_WEB_HOSTS")
        .unwrap_or_else(|_| "localhost,127.0.0.1,::1".to_string())
        .split(',')
        .map(|s| {
            s.trim()
                .trim_matches('[')
                .trim_matches(']')
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

pub async fn record_web_approval(
    pool: &PgPool,
    bear_id: Uuid,
    scope_kind: &str,
    scope_value: &str,
    approved_by_user_id: Option<i32>,
    source: &str,
    ttl_seconds: Option<i64>,
) -> Result<(), CustomError> {
    if !matches!(scope_kind, "url" | "host") {
        return Err(CustomError::ValidationError(
            "web approval scope_kind must be url or host".to_string(),
        ));
    }
    let expires_expr = ttl_seconds
        .map(|seconds| format!("now() + interval '{} seconds'", seconds.clamp(1, 86_400)));
    let sql = if expires_expr.is_some() {
        r#"
        INSERT INTO bear_web_approvals (bear_id, scope_kind, scope_value, approved_by_user_id, source, expires_at)
        VALUES ($1, $2, $3, $4, $5, now() + ($6::text || ' seconds')::interval)
        ON CONFLICT (bear_id, scope_kind, scope_value) WHERE revoked_at IS NULL
        DO UPDATE SET approved_by_user_id = EXCLUDED.approved_by_user_id,
                      source = EXCLUDED.source,
                      expires_at = EXCLUDED.expires_at
        "#
    } else {
        r#"
        INSERT INTO bear_web_approvals (bear_id, scope_kind, scope_value, approved_by_user_id, source, expires_at)
        VALUES ($1, $2, $3, $4, $5, NULL)
        ON CONFLICT (bear_id, scope_kind, scope_value) WHERE revoked_at IS NULL
        DO UPDATE SET approved_by_user_id = EXCLUDED.approved_by_user_id,
                      source = EXCLUDED.source,
                      expires_at = NULL
        "#
    };
    let mut query = sqlx::query(sql)
        .bind(bear_id)
        .bind(scope_kind)
        .bind(scope_value)
        .bind(approved_by_user_id)
        .bind(source);
    if let Some(seconds) = ttl_seconds {
        query = query.bind(seconds.to_string());
    }
    query.execute(pool).await?;
    Ok(())
}

pub async fn record_web_fetch_attempt(
    pool: &PgPool,
    bear_id: Uuid,
    session_id: Option<&str>,
    tool_call_id: Option<&str>,
    url: &str,
    final_url: Option<&str>,
    host: &str,
    execution_location: &str,
    approval_kind: &str,
    http_status: Option<i32>,
    content_type: Option<&str>,
    bytes: Option<i64>,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO bear_web_fetches (
            bear_id, session_id, tool_call_id, url, final_url, host,
            execution_location, approval_kind, http_status, content_type, bytes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(bear_id)
    .bind(session_id)
    .bind(tool_call_id)
    .bind(url)
    .bind(final_url)
    .bind(host)
    .bind(execution_location)
    .bind(approval_kind)
    .bind(http_status)
    .bind(content_type)
    .bind(bytes)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn preferred_hosts_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<String>, CustomError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT scope_value
        FROM bear_web_sources
        WHERE bear_id = $1
          AND scope_kind = 'host'
          AND policy = 'preferred'
        ORDER BY priority DESC, scope_value ASC
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

pub async fn preferred_hosts_json(pool: &PgPool, bear_id: Uuid) -> Result<Value, CustomError> {
    Ok(json!(preferred_hosts_for_bear(pool, bear_id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_url_and_host() {
        let n = normalize_web_url("HTTPS://Example.COM:443/a?b=1#frag").unwrap();
        assert_eq!(n.url, "https://example.com/a?b=1");
        assert_eq!(n.host, "example.com");
        let n = normalize_web_url("http://Example.COM:8080/a").unwrap();
        assert_eq!(n.host, "example.com:8080");
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(normalize_web_url("file:///tmp/x").is_err());
    }

    #[test]
    fn detects_default_local_hosts() {
        let n = normalize_web_url("http://localhost:3000/docs").unwrap();
        assert!(is_local_web_url(&n));
        let n = normalize_web_url("https://example.com/docs").unwrap();
        assert!(!is_local_web_url(&n));
    }
}
