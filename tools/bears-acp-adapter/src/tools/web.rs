use crate::ToolPolicy;
use anyhow::{anyhow, Result};
use reqwest::Url;
use serde_json::{json, Value};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

pub(crate) async fn handle_local_web_fetch(
    session_id: &str,
    args: &Value,
    policy: &ToolPolicy,
) -> Result<Value> {
    let raw_url = args
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("web_fetch args missing url"))?;
    let url = Url::parse(raw_url).map_err(|err| anyhow!("web_fetch invalid url: {err}"))?;
    validate_local_fetch_url(&url)?;
    let policy_max_bytes = policy.max_bytes.unwrap_or(262_144).clamp(1, 1_048_576);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, policy_max_bytes) as usize)
        .unwrap_or(policy_max_bytes as usize);
    let timeout_ms = policy.total_timeout_ms.unwrap_or(120_000).clamp(1, 120_000);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(Duration::from_millis(timeout_ms))
        .build()?;
    let started = std::time::Instant::now();
    let response = client
        .get(url.clone())
        .header(
            reqwest::header::ACCEPT,
            "text/*, application/json, application/xml, application/xhtml+xml, */*;q=0.5",
        )
        .send()
        .await?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await?;
    let truncated = bytes.len() > max_bytes;
    let body_bytes = if truncated {
        &bytes[..max_bytes]
    } else {
        &bytes[..]
    };
    let body = String::from_utf8_lossy(body_bytes).to_string();
    eprintln!(
        "bears-acp-adapter: web_fetch session_id={} url={} status={} bytes={} truncated={} duration_ms={}",
        session_id,
        raw_url,
        status,
        bytes.len(),
        truncated,
        started.elapsed().as_millis(),
    );
    Ok(json!({
        "ok": status < 400,
        "url": raw_url,
        "final_url": final_url,
        "status": status,
        "content_type": content_type,
        "body": body,
        "bytes": bytes.len(),
        "returned_bytes": body_bytes.len(),
        "truncated": truncated,
        "elapsed_ms": started.elapsed().as_millis(),
        "source": "adapter_local",
        "content": format!("Fetched {} with HTTP {}{}", raw_url, status, if truncated { " (truncated)" } else { "" }),
        "policy": { "max_bytes": policy_max_bytes, "applied_max_bytes": max_bytes, "timeout_ms": timeout_ms }
    }))
}

fn validate_local_fetch_url(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(anyhow!(
                "local_web_fetch only supports http and https URLs, got {other:?}"
            ))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("local_web_fetch URL must include a host"))?;
    let normalized = normalize_host(host, url.port());
    if local_web_hosts()
        .iter()
        .any(|allowed| allowed == &normalized || allowed == host)
    {
        return Ok(());
    }
    Err(anyhow!(
        "local_web_fetch host {normalized:?} is not in BEARS_LOCAL_WEB_HOSTS"
    ))
}

fn validate_fetch_url(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(anyhow!(
                "web_fetch only supports http and https URLs, got {other:?}"
            ))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("web_fetch URL must include a host"))?;
    let lower = host.to_ascii_lowercase();
    if !allow_local_web_fetch_for_tests()
        && (matches!(lower.as_str(), "localhost" | "localhost.localdomain")
            || lower.ends_with(".localhost"))
    {
        return Err(anyhow!("web_fetch denies localhost URLs"));
    }
    if !allow_local_web_fetch_for_tests() {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_denied_ip(ip) {
                return Err(anyhow!(
                    "web_fetch denies private, loopback, link-local, and metadata IP URLs"
                ));
            }
        }
    }
    Ok(())
}

fn allow_local_web_fetch_for_tests() -> bool {
    cfg!(test)
        && std::env::var("BEARS_ACP_ALLOW_LOCAL_WEB_FETCH_FOR_TESTS")
            .ok()
            .as_deref()
            == Some("1")
}

fn local_web_hosts() -> Vec<String> {
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

fn normalize_host(host: &str, port: Option<u16>) -> String {
    let host = host
        .trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase();
    match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

fn is_denied_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || matches!(ip.segments()[0] & 0xfe00, 0xfc00)
                || matches!(ip.segments()[0] & 0xffc0, 0xfe80)
                || ip == Ipv6Addr::LOCALHOST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_urls() {
        assert!(validate_fetch_url(&Url::parse("file:///tmp/x").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("http://localhost:3000").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("http://127.0.0.1").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("http://169.254.169.254").unwrap()).is_err());
        assert!(validate_fetch_url(&Url::parse("https://example.com").unwrap()).is_ok());
        assert!(validate_local_fetch_url(&Url::parse("http://localhost:3000").unwrap()).is_ok());
    }
}
