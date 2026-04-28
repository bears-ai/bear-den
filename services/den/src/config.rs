#![allow(dead_code)]

use std::collections::HashSet;

use url::Url;

/// Human-facing product name (browser title, emails, PWA). Override with `APP_DISPLAY_NAME`.
const DEFAULT_APP_DISPLAY_NAME: &str = "BEARS";

/// Machine slug (manifest `id`, short name). Override with `APP_SLUG`.
const DEFAULT_APP_SLUG: &str = "bears";

/// Default public web origin when `WEB_SERVER_URL` is unset in **production** builds.
/// Forks should set `WEB_SERVER_URL` / `API_SERVER_URL` or change these constants.
const DEFAULT_PROD_WEB_ORIGIN: &str = "https://bears.artificial.design";
const DEFAULT_PROD_API_ORIGIN: &str = "https://api.bears.artificial.design";

/// Letta HTTP API when `LETTA_BASE_URL` is unset — matches Docker Compose service `bears-letta` (repository root `docker-compose.yaml`).
pub const DEFAULT_LETTA_BASE_URL: &str = "http://bears-letta:8283";
/// Codepool harness when `CODEPOOL_BASE_URL` is unset — matches Docker Compose service `bears-codepool`.
pub const DEFAULT_CODEPOOL_BASE_URL: &str = "http://bears-codepool:3030";

pub fn session_cookie_secure_from_env(default: bool) -> bool {
    std::env::var("SESSION_COOKIE_SECURE")
        .map(|v| match v.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "no" | "off" => false,
            "1" | "true" | "yes" | "on" => true,
            _ => default,
        })
        .unwrap_or(default)
}

fn letta_base_url_from_env() -> String {
    let raw = std::env::var("LETTA_BASE_URL").unwrap_or_default();
    let trimmed = raw.trim_end_matches('/').to_string();
    if !trimmed.is_empty() {
        return trimmed;
    }
    #[cfg(feature = "production")]
    {
        DEFAULT_LETTA_BASE_URL.to_string()
    }
    #[cfg(not(feature = "production"))]
    {
        String::new()
    }
}

fn codepool_base_url_from_env() -> String {
    let raw = std::env::var("CODEPOOL_BASE_URL").unwrap_or_default();
    let trimmed = raw.trim_end_matches('/').to_string();
    if !trimmed.is_empty() {
        return trimmed;
    }
    #[cfg(feature = "production")]
    {
        DEFAULT_CODEPOOL_BASE_URL.to_string()
    }
    #[cfg(not(feature = "production"))]
    {
        String::new()
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub templates_dir: String,
    pub database_url: String,

    pub mailgun_api_key: String,
    pub mailgun_domain: String,

    /// Shown as the sender display name in outbound mail.
    pub app_display_name: String,
    /// Local-part@domain for Mailgun From (full address). Override with `MAIL_FROM_ADDRESS`.
    pub mail_from_address: String,

    pub telemetry_url_prefix: String,
    pub email_verify_url_prefix: String,

    /// Short machine id (`manifest.json` id, etc.).
    pub app_slug: String,

    /// Optional cookie `Domain` attribute (production). When unset or empty, cookies are host-only.
    pub session_cookie_domain: Option<String>,

    /// Enable the web server (`RUN_WEB`, default false).
    pub run_web: bool,

    /// Enable the API server (`RUN_API`, default false).
    pub run_api: bool,

    /// Background worker slot (`RUN_WORKERS`); this slim starter has no domain workers yet.
    pub run_workers: bool,

    pub web_port: u16,
    pub api_port: u16,

    /// Public base URL for the **web** service (no trailing slash). Links, redirects, CORS.
    pub web_server_url: String,
    /// Public base URL for the **API** service (no trailing slash).
    pub api_server_url: String,

    /// Letta server base URL (no trailing slash), e.g. `http://letta:8283`. Empty = provisioning/chat proxy disabled.
    pub letta_base_url: String,
    /// Optional `Authorization: Bearer` value for Letta (omit when local Letta has no auth).
    pub letta_api_key: String,

    /// **Codepool** harness base URL (no trailing slash), e.g. `http://bears-codepool:3030`.
    /// Required when `run_web` is true — [`crate::startup::validate_runtime_config`] enforces this.
    pub codepool_base_url: String,
    /// Optional `Authorization: Bearer` for Codepool (must match `CODEPOOL_INTERNAL_TOKEN` on the pool).
    pub codepool_internal_token: String,

    /// **MemFS Manager** (Letta `LETTA_MEMFS_SERVICE_URL` — git sidecar) base URL (no trailing slash), e.g. `http://bears-memfs-manager:8285`. Empty = skip private-memory readout in Den.
    pub letta_memfs_service_url: String,

    /// Letta’s Postgres URI when external DB is used (`LETTA_PG_URI`). Empty = not checked on Den.
    /// Shape rules match deploy docs and `services/preflight` (prefer `postgresql://`).
    pub letta_pg_uri: String,

    /// Bifrost gateway base URL (no trailing slash), e.g. `http://bears-bifrost:8080`. Empty = skip HTTP check.
    pub bifrost_base_url: String,

    /// S3-compatible endpoint (e.g. `http://bears-garage:3900`). Empty = media upload disabled.
    pub s3_endpoint: String,
    /// S3 bucket for chat media and generated images.
    pub s3_bucket: String,
    /// S3 region (Garage default: `garage`).
    pub s3_region: String,
    /// S3 access key id.
    pub s3_access_key_id: String,
    /// S3 secret access key.
    pub s3_secret_access_key: String,
    /// Public URL prefix for presigned download URLs (the origin browsers can reach).
    /// Falls back to `s3_endpoint` when empty.
    pub s3_public_url: String,
    /// Use path-style addressing (`endpoint/bucket/key` instead of `bucket.endpoint/key`).
    /// Required for Garage and most self-hosted S3; defaults to true.
    pub s3_force_path_style: bool,

    /// Maximum number of connections in the SQLx pool (`DB_MAX_CONNECTIONS`, default 5).
    pub db_max_connections: u32,
    /// Seconds to wait for a connection from the pool before timing out (`DB_ACQUIRE_TIMEOUT_SECS`, default 3).
    pub db_acquire_timeout_secs: u64,
    /// Seconds a connection can sit idle before being closed (`DB_IDLE_TIMEOUT_SECS`, default 600).
    /// Set to 0 to disable idle reaping.
    pub db_idle_timeout_secs: u64,

    /// PAT with `read:packages` for [`crate::web::status`] GHCR comparison (optional; when empty, registry columns show "not configured").
    pub github_packages_token: String,
    /// GitHub org or username that owns `den` / `codepool` images on GHCR (e.g. `theartificial`).
    pub ghcr_packages_owner: String,
    /// `org` or `user` — used with GitHub Packages REST paths.
    pub ghcr_packages_owner_kind: String,
}

impl Config {
    /// Web origin without trailing slash — use for path suffixes: `{}{path}`.
    pub fn web_public_origin(&self) -> String {
        self.web_server_url.trim_end_matches('/').to_string()
    }

    /// Distinct browser `Origin` values for CORS (scheme + host + port, no path).
    pub fn cors_allowed_origins(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for raw in [&self.web_server_url, &self.api_server_url] {
            match Url::parse(raw.as_str()) {
                Ok(u) => {
                    let origin = u.origin().ascii_serialization();
                    if origin != "null" && seen.insert(origin.clone()) {
                        out.push(origin);
                    }
                }
                Err(e) => {
                    tracing::warn!("Could not parse URL for CORS origin (value={raw:?}): {e}")
                }
            }
        }
        out
    }

    /// Load configuration from process environment (and optional `.env` via dotenvy).
    ///
    /// Call once at process startup; thread an [`std::sync::Arc`] through services that need it.
    pub fn load() -> Self {
        if dotenvy::dotenv().is_ok() {
            tracing::info!("Loaded .env file");
        }

        let app_display_name = std::env::var("APP_DISPLAY_NAME")
            .unwrap_or_else(|_| DEFAULT_APP_DISPLAY_NAME.to_string());
        let app_slug = std::env::var("APP_SLUG").unwrap_or_else(|_| DEFAULT_APP_SLUG.to_string());

        fn parse_bool_env(name: &str, default: bool) -> bool {
            std::env::var(name)
                .unwrap_or_else(|_| {
                    if default {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                })
                .parse::<bool>()
                .unwrap_or_else(|_| {
                    tracing::warn!(
                        "Invalid {} environment variable. Expected 'true' or 'false'. Defaulting to {}.",
                        name,
                        default
                    );
                    default
                })
        }

        let (run_web, run_api, run_workers) = if let Ok(server_mode) = std::env::var("SERVER_MODE")
        {
            match server_mode.to_lowercase().as_str() {
                "web" => (true, false, true),
                "api" => (false, true, true),
                "both" => (true, true, true),
                _ => {
                    tracing::warn!(
                        "Invalid SERVER_MODE value '{}'. Use RUN_WEB, RUN_API, RUN_WORKERS instead.",
                        server_mode
                    );
                    (false, false, false)
                }
            }
        } else {
            (
                parse_bool_env("RUN_WEB", false),
                parse_bool_env("RUN_API", false),
                parse_bool_env("RUN_WORKERS", false),
            )
        };

        let web_port = std::env::var("PORT")
            .unwrap_or_else(|_| "3000".to_string())
            .parse::<u16>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid PORT environment variable. Defaulting to 3000");
                3000
            });

        let api_port = std::env::var("API_PORT")
            .unwrap_or_else(|_| "3001".to_string())
            .parse::<u16>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid API_PORT environment variable. Defaulting to 3001");
                3001
            });

        let web_server_url = std::env::var("WEB_SERVER_URL").unwrap_or_else(|_| {
            #[cfg(feature = "production")]
            {
                DEFAULT_PROD_WEB_ORIGIN.to_string()
            }
            #[cfg(not(feature = "production"))]
            {
                format!("http://localhost:{web_port}")
            }
        });
        let web_server_url = trim_url_for_storage(web_server_url);

        let api_server_url = std::env::var("API_SERVER_URL").unwrap_or_else(|_| {
            #[cfg(feature = "production")]
            {
                DEFAULT_PROD_API_ORIGIN.to_string()
            }
            #[cfg(not(feature = "production"))]
            {
                format!("http://localhost:{api_port}")
            }
        });
        let api_server_url = trim_url_for_storage(api_server_url);

        let public_web_base = format_url_prefix(&web_server_url);
        let email_verify_url_prefix = format!("{}settings/email/verify/", public_web_base);
        let telemetry_url_prefix = format!("{}telemetry/", public_web_base);

        let session_cookie_domain = std::env::var("SESSION_COOKIE_DOMAIN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mail_from_address = std::env::var("MAIL_FROM_ADDRESS").unwrap_or_else(|_| {
            derive_default_mail_from(&web_server_url)
                .unwrap_or_else(|| "noreply@bears.artificial.design".to_string())
        });

        let letta_base_url = letta_base_url_from_env();
        let letta_api_key = std::env::var("LETTA_API_KEY").unwrap_or_default();

        let codepool_base_url = codepool_base_url_from_env();
        let codepool_internal_token = std::env::var("CODEPOOL_INTERNAL_TOKEN").unwrap_or_default();

        let letta_memfs_service_url = std::env::var("LETTA_MEMFS_SERVICE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();

        let letta_pg_uri = std::env::var("LETTA_PG_URI").unwrap_or_default();
        let letta_pg_uri = letta_pg_uri.trim().to_string();

        let bifrost_base_url = std::env::var("BIFROST_BASE_URL").unwrap_or_default();
        let bifrost_base_url = bifrost_base_url.trim_end_matches('/').to_string();

        let s3_endpoint = std::env::var("S3_ENDPOINT")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let s3_bucket = std::env::var("S3_BUCKET").unwrap_or_default();
        let s3_region = std::env::var("S3_REGION").unwrap_or_else(|_| "garage".to_string());
        let s3_access_key_id = std::env::var("S3_ACCESS_KEY_ID").unwrap_or_default();
        let s3_secret_access_key = std::env::var("S3_SECRET_ACCESS_KEY").unwrap_or_default();
        let s3_public_url = std::env::var("S3_PUBLIC_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let s3_force_path_style = parse_bool_env("S3_FORCE_PATH_STYLE", true);

        let db_max_connections: u32 = std::env::var("DB_MAX_CONNECTIONS")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid DB_MAX_CONNECTIONS, defaulting to 5");
                5
            });
        let db_acquire_timeout_secs: u64 = std::env::var("DB_ACQUIRE_TIMEOUT_SECS")
            .unwrap_or_else(|_| "3".to_string())
            .parse()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid DB_ACQUIRE_TIMEOUT_SECS, defaulting to 3");
                3
            });
        let db_idle_timeout_secs: u64 = std::env::var("DB_IDLE_TIMEOUT_SECS")
            .unwrap_or_else(|_| "600".to_string())
            .parse()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid DB_IDLE_TIMEOUT_SECS, defaulting to 600");
                600
            });

        let github_packages_token = std::env::var("GITHUB_PACKAGES_TOKEN").unwrap_or_default();
        let ghcr_packages_owner = std::env::var("GHCR_PACKAGES_OWNER")
            .unwrap_or_default()
            .trim()
            .to_string();
        let ghcr_packages_owner_kind = std::env::var("GHCR_PACKAGES_OWNER_KIND")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let ghcr_packages_owner_kind = if ghcr_packages_owner_kind.is_empty()
            || matches!(ghcr_packages_owner_kind.as_str(), "org" | "user")
        {
            ghcr_packages_owner_kind
        } else {
            tracing::warn!("Invalid GHCR_PACKAGES_OWNER_KIND (expected org|user). Leaving empty.");
            String::new()
        };

        Config {
            templates_dir: std::env::var("TEMPLATES_DIR")
                .unwrap_or("src/web/templates".to_string()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL"),

            mailgun_api_key: std::env::var("MAILGUN_API_KEY").unwrap_or_default(),
            mailgun_domain: std::env::var("MAILGUN_DOMAIN").unwrap_or_default(),

            app_display_name,
            mail_from_address,

            telemetry_url_prefix,
            email_verify_url_prefix,

            app_slug,

            session_cookie_domain,

            run_web,
            run_api,
            run_workers,
            web_port,
            api_port,
            web_server_url,
            api_server_url,
            letta_base_url,
            letta_api_key,
            codepool_base_url,
            codepool_internal_token,
            letta_memfs_service_url,
            letta_pg_uri,
            bifrost_base_url,
            s3_endpoint,
            s3_bucket,
            s3_region,
            s3_access_key_id,
            s3_secret_access_key,
            s3_public_url,
            s3_force_path_style,
            db_max_connections,
            db_acquire_timeout_secs,
            db_idle_timeout_secs,
            github_packages_token,
            ghcr_packages_owner,
            ghcr_packages_owner_kind,
        }
    }
}

fn trim_url_for_storage(url: String) -> String {
    url.trim_end_matches('/').to_string()
}

/// Ensures a trailing slash for URL-prefix style concatenation.
fn format_url_prefix(base: &str) -> String {
    format!("{}/", base.trim_end_matches('/'))
}

/// If `WEB_SERVER_URL` is a normal `https` host, use `noreply@<host>`.
fn derive_default_mail_from(web_server_url: &str) -> Option<String> {
    let u = Url::parse(web_server_url).ok()?;
    let host = u.host_str()?;
    if host == "localhost" || host.starts_with('[') {
        return None;
    }
    Some(format!("noreply@{host}"))
}

#[cfg(test)]
impl Config {
    /// Minimal config for unit tests that only need URL / branding fields.
    pub fn test_stub() -> Self {
        Self {
            templates_dir: "x".into(),
            database_url: "postgres://localhost/den_test".into(),
            mailgun_api_key: String::new(),
            mailgun_domain: String::new(),
            app_display_name: "Test".into(),
            mail_from_address: "noreply@localhost".into(),
            telemetry_url_prefix: "http://localhost:3000/telemetry/".into(),
            email_verify_url_prefix: "http://localhost:3000/settings/email/verify/".into(),
            app_slug: "test".into(),
            session_cookie_domain: None,
            run_web: false,
            run_api: false,
            run_workers: false,
            web_port: 3000,
            api_port: 3001,
            web_server_url: "http://localhost:3000".into(),
            api_server_url: "http://localhost:3001".into(),
            letta_base_url: String::new(),
            letta_api_key: String::new(),
            codepool_base_url: String::new(),
            codepool_internal_token: String::new(),
            letta_memfs_service_url: String::new(),
            letta_pg_uri: String::new(),
            bifrost_base_url: String::new(),
            s3_endpoint: String::new(),
            s3_bucket: String::new(),
            s3_region: "garage".into(),
            s3_access_key_id: String::new(),
            s3_secret_access_key: String::new(),
            s3_public_url: String::new(),
            s3_force_path_style: true,
            db_max_connections: 5,
            db_acquire_timeout_secs: 3,
            db_idle_timeout_secs: 600,
            github_packages_token: String::new(),
            ghcr_packages_owner: String::new(),
            ghcr_packages_owner_kind: String::new(),
        }
    }
}
