#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use url::Url;

/// Human-facing product name (browser title, emails, PWA). Override with `APP_DISPLAY_NAME`.
const DEFAULT_APP_DISPLAY_NAME: &str = "BEARS";

/// Machine slug (manifest `id`, short name). Override with `APP_SLUG`.
const DEFAULT_APP_SLUG: &str = "bears";

/// Default public web origin when `WEB_SERVER_URL` is unset in **production** builds.
/// Forks should set `WEB_SERVER_URL` / `API_SERVER_URL` or change these constants.
const DEFAULT_PROD_WEB_ORIGIN: &str = "https://bears.artificial.design";
const DEFAULT_PROD_API_ORIGIN: &str = "https://api.bears.artificial.design";

/// Bifrost OpenAI-compatible API when `LLM_API_URL` is unset — matches Docker Compose `bears-bifrost:8080/v1`.
pub const DEFAULT_LLM_API_URL: &str = "http://bears-bifrost:8080/v1";

/// Sandbox provider host used by the default Docker Compose stack.
const DEFAULT_SANDBOX_SERVER_HOST: &str = "bears-sandbox-provider";

/// Qdrant host used by the default Docker Compose recall profile.
const DEFAULT_QDRANT_HOST: &str = "bears-qdrant";
const DEFAULT_QDRANT_PORT: u16 = 6333;

/// GitHub account type used for GHCR REST API paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhcrPackagesOwnerKind {
    #[default]
    Unconfigured,
    Organization,
    User,
}

impl GhcrPackagesOwnerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "",
            Self::Organization => "org",
            Self::User => "user",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" => Some(Self::Unconfigured),
            "org" => Some(Self::Organization),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

pub fn session_cookie_secure_from_env(default: bool) -> bool {
    std::env::var("SESSION_COOKIE_SECURE")
        .map(|v| match v.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "no" | "off" => false,
            "1" | "true" | "yes" | "on" => true,
            _ => default,
        })
        .unwrap_or(default)
}

/// Explicit fixture profiles for feature-gated web UI smoke testing.
///
/// These profiles are only usable when the `web-ui-fixtures` Cargo feature is compiled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiFixtureProfile {
    BearDetailsRich,
    BearDetailsWarning,
    BearChatBasic,
    BearChatError,
}

impl UiFixtureProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BearDetailsRich => "bear-details-rich",
            Self::BearDetailsWarning => "bear-details-warning",
            Self::BearChatBasic => "bear-chat-basic",
            Self::BearChatError => "bear-chat-error",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "bear-details-rich" => Some(Self::BearDetailsRich),
            "bear-details-warning" => Some(Self::BearDetailsWarning),
            "bear-chat-basic" => Some(Self::BearChatBasic),
            "bear-chat-error" => Some(Self::BearChatError),
            _ => None,
        }
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

    /// Enable background workers (`RUN_WORKERS`).
    ///
    /// Current worker set includes the memory-curate queue runner used to process
    /// queued `memory_curate` reflection runs.
    pub run_workers: bool,

    /// Enable the sandbox provider service (`RUN_SANDBOX`, default false).
    ///
    /// The sandbox server can run standalone on a separate host from the Den
    /// instance using it; when it is the only enabled service the process does
    /// not require `DATABASE_URL`.
    pub run_sandbox: bool,

    pub web_port: u16,
    pub api_port: u16,
    /// Port for the sandbox provider API (`SANDBOX_PORT`, default 3002).
    pub sandbox_port: u16,

    /// Public base URL for the **web** service (no trailing slash). Links, redirects, CORS.
    pub web_server_url: String,
    /// Public base URL for the **API** service (no trailing slash).
    pub api_server_url: String,

    /// Secret used to encrypt Den-managed credentials at rest (`DEN_SECRET_ENCRYPTION_KEY`).
    /// Empty means no new encrypted secret values can be stored.
    pub den_secret_encryption_key: String,

    /// Bifrost gateway base URL (no trailing slash), e.g. `http://bears-bifrost:8080`. Empty = skip HTTP check.
    pub bifrost_base_url: String,
    /// Bifrost management API base URL, e.g. `http://bears-bifrost:8080/api`.
    pub bifrost_management_url: String,
    /// Bifrost management admin username (`BIFROST_ADMIN_USERNAME`).
    pub bifrost_admin_username: String,
    /// Bifrost management admin password (`BIFROST_ADMIN_PASSWORD`).
    pub bifrost_admin_password: String,
    /// Seconds between background Bifrost model-catalog refreshes
    /// (`BIFROST_CATALOG_REFRESH_SECS`, default 300). `0` warms once at startup
    /// with no periodic refresh.
    pub bifrost_catalog_refresh_secs: u64,

    /// OpenAI-compatible inference API (no trailing slash), e.g. `http://bears-bifrost:8080/v1`.
    /// Used by the Den-native agent loop. Defaults from `LLM_API_URL`, then `BIFROST_BASE_URL/v1`.
    pub llm_api_url: String,
    /// Optional bearer token for `llm_api_url` (`LLM_API_KEY` or `OPENAI_API_KEY`).
    pub llm_api_key: String,
    /// Default model handle for native runtime turns (`DEFAULT_LLM_MODEL`).
    pub default_llm_model: String,
    /// Per-model native tool-budget multipliers (`BEARS_MODEL_TOOL_BUDGET_MULTIPLIERS`,
    /// comma-separated `model=multiplier` pairs). Unknown models simply do not match.
    pub model_tool_budget_multipliers: HashMap<String, f64>,
    /// Directory for per-Bear SQLite databases (`BEAR_SQLITE_DATA_DIR`, default `/var/lib/den/bear-sqlite`).
    pub bear_sqlite_data_dir: String,

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

    /// PAT with `read:packages` for `web::status` GHCR comparison (optional; when empty, registry columns show "not configured").
    pub github_packages_token: String,
    /// GitHub org or username that owns BEARS images on GHCR (e.g. `theartificial`).
    pub ghcr_packages_owner: String,
    /// GitHub account type used for GHCR REST API paths.
    pub ghcr_packages_owner_kind: GhcrPackagesOwnerKind,

    /// Search provider for Den web search tools (`DEN_SEARCH_PROVIDER`, e.g. `brave`). Empty disables search.
    pub den_search_provider: String,
    /// Brave Search API key (`BRAVE_SEARCH_API_KEY`) when `DEN_SEARCH_PROVIDER=brave`.
    pub brave_search_api_key: String,
    /// Max results returned by Den search tools (`DEN_SEARCH_MAX_RESULTS`, default 5, clamped 1..10).
    pub den_search_max_results: usize,

    /// Derived recall vector store (Qdrant) base URL, e.g. `http://bears-qdrant:6333`
    /// (`QDRANT_URL`). `None`/empty disables recall — Den falls back to keyword search (ADR-0038).
    pub qdrant_url: Option<String>,
    /// Active platform embedding standard id (`EMBEDDING_STANDARD`, default `bears-embed-v1`).
    pub embedding_standard: String,
    /// Embedding model for the active standard (`EMBEDDING_MODEL`, default `text-embedding-3-small`).
    pub embedding_model: String,
    /// Embedding vector dimensions for the active standard (`EMBEDDING_DIMENSIONS`, default 1536).
    pub embedding_dimensions: u32,

    /// Static bearer token protecting the sandbox provider API (`SANDBOX_SERVICE_TOKEN`).
    /// Empty = auth disabled (provider service configuration; set an actual secret in production).
    pub sandbox_service_token: String,
    /// Directory holding pristine clones, ephemeral sandbox workspaces, and
    /// the persisted Den-managed config
    /// (`SANDBOX_WORKSPACES_DIR`, default `./data/sandbox-workspaces`).
    pub sandbox_workspaces_dir: String,
    /// Directory holding the shipped sandbox-image build context
    /// (`SANDBOX_BUILD_CONTEXT_DIR`). `None` disables the image-build
    /// endpoints; pulls still work.
    pub sandbox_build_context_dir: Option<String>,
    /// Host-side path of `sandbox_workspaces_dir` when the provider itself
    /// runs in a container (`SANDBOX_WORKSPACES_HOST_DIR`). Sibling-sandbox
    /// bind sources are rewritten to this path because the host docker daemon
    /// resolves them host-side. `None` = provider runs directly on the host.
    pub sandbox_workspaces_host_dir: Option<String>,
    /// Default container image for sandboxes (`SANDBOX_IMAGE`).
    pub sandbox_default_image: String,
    /// Maximum concurrently running sandboxes (`SANDBOX_MAX_CONCURRENT`, default 2).
    pub sandbox_max_concurrent: usize,
    /// Per-sandbox wall-clock timeout in seconds (`SANDBOX_DEFAULT_TIMEOUT_SECS`, default 900).
    pub sandbox_default_timeout_secs: u64,
    /// Cap on retained sandbox log bytes (`SANDBOX_MAX_LOG_BYTES`, default 2 MiB).
    pub sandbox_max_log_bytes: u64,

    /// Base URL of the sandbox provider this Den instance dispatches work to
    /// (`SANDBOX_SERVER_URL`, no trailing slash). `None` disables work dispatch.
    pub sandbox_server_url: Option<String>,
    /// Bearer token for calls to `sandbox_server_url` (`SANDBOX_SERVER_TOKEN`).
    pub sandbox_server_token: String,
    /// Den API base URL as reachable from inside sandbox containers
    /// (`SANDBOX_CALLBACK_API_URL`, e.g. `http://host.docker.internal:3001`).
    /// Defaults to `api_server_url`.
    pub sandbox_callback_api_url: String,
    /// Automatically enqueue eligible Jobs for dispatch (`WORK_DISPATCH_AUTO`, default false).
    pub work_dispatch_auto: bool,
    /// Preserve sandbox workspaces of failed runs for debugging (`SANDBOX_PRESERVE_FAILED`).
    pub sandbox_preserve_failed: bool,
    /// Maximum run attempts per task before it stays blocked (`WORK_MAX_ATTEMPTS`, default 2).
    pub work_max_attempts: u32,
    /// Network posture requested for work sandboxes (`WORK_SANDBOX_NETWORK`:
    /// `restricted` default — egress limited to a Den-only relay — or `open`).
    pub work_sandbox_network: String,

    /// Compaction rollout mode (`COMPACTION_MODE`: `observe` default, `active`, `off`).
    pub compaction_mode: String,
    /// When compaction writes run (`COMPACTION_TIMING`: `async` default, `sync`).
    pub compaction_timing: String,
    /// Agent loop control mode (`BEARS_AGENT_LOOP_CONTROL`: `enforce` default, `observe`, `off`).
    pub agent_loop_control_mode: String,
    /// Checkpoint artifact audit retention (`BEARS_CHECKPOINT_AUDIT`: `work` default, `off`, `all`).
    pub checkpoint_audit_mode: String,
    ///
    /// This is a runtime selector, not a generic development mode switch. The named profile is
    /// only honored when the binary is compiled with the `web-ui-fixtures` Cargo feature.
    pub ui_fixture_profile: Option<UiFixtureProfile>,
}

impl Config {
    /// Web origin without trailing slash — use for path suffixes: `{}{path}`.
    pub fn web_public_origin(&self) -> String {
        self.web_server_url.trim_end_matches('/').to_string()
    }

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
                    tracing::warn!("Could not parse URL for CORS origin (value={raw:?}): {e}");
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
        // Deliberately outside the SERVER_MODE shorthand: the sandbox provider is
        // opted into explicitly, never via legacy mode aliases.
        let run_sandbox = parse_bool_env("RUN_SANDBOX", false);

        let ui_fixture_profile = match std::env::var("UI_FIXTURE_PROFILE") {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    match UiFixtureProfile::parse(trimmed) {
                        Some(profile) => Some(profile),
                        None => {
                            tracing::warn!(
                                "Invalid UI_FIXTURE_PROFILE value '{}'. Supported values: bear-details-rich, bear-details-warning, bear-chat-basic, bear-chat-error.",
                                trimmed
                            );
                            None
                        }
                    }
                }
            }
            Err(_) => None,
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

        let sandbox_port = std::env::var("SANDBOX_PORT")
            .unwrap_or_else(|_| "3002".to_string())
            .parse::<u16>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid SANDBOX_PORT environment variable. Defaulting to 3002");
                3002
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

        let den_secret_encryption_key =
            std::env::var("DEN_SECRET_ENCRYPTION_KEY").unwrap_or_default();

        let bifrost_origin = std::env::var("BIFROST_ORIGIN")
            .ok()
            .map(|value| value.trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        let bifrost_base_url = if let Some(origin) = bifrost_origin.clone() {
            origin
        } else {
            std::env::var("BIFROST_BASE_URL")
                .ok()
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_default()
        };
        let bifrost_management_url = if let Some(origin) = bifrost_origin.as_deref() {
            format!("{origin}/api")
        } else {
            std::env::var("BIFROST_MANAGEMENT_URL")
                .ok()
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    if bifrost_base_url.is_empty() {
                        String::new()
                    } else {
                        format!("{bifrost_base_url}/api")
                    }
                })
        };
        let bifrost_admin_username = std::env::var("BIFROST_ADMIN_USERNAME").unwrap_or_default();
        let bifrost_admin_password = std::env::var("BIFROST_ADMIN_PASSWORD").unwrap_or_default();
        let bifrost_catalog_refresh_secs = std::env::var("BIFROST_CATALOG_REFRESH_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(300);

        let llm_api_url = if let Some(origin) = bifrost_origin.as_deref() {
            format!("{origin}/v1")
        } else {
            std::env::var("LLM_API_URL")
                .ok()
                .map(|v| v.trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    if bifrost_base_url.is_empty() {
                        String::new()
                    } else {
                        format!("{bifrost_base_url}/v1")
                    }
                })
        };
        let llm_api_key = std::env::var("LLM_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .unwrap_or_default();
        let default_llm_model =
            std::env::var("DEFAULT_LLM_MODEL").unwrap_or_else(|_| "openai/gpt-4.1".to_string());
        let model_tool_budget_multipliers = parse_model_tool_budget_multipliers_env(
            std::env::var("BEARS_MODEL_TOOL_BUDGET_MULTIPLIERS")
                .ok()
                .as_deref(),
        );
        let bear_sqlite_data_dir = std::env::var("BEAR_SQLITE_DATA_DIR")
            .unwrap_or_else(|_| "/var/lib/den/bear-sqlite".to_string());

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
        let ghcr_packages_owner_kind = match std::env::var("GHCR_PACKAGES_OWNER_KIND") {
            Ok(value) => GhcrPackagesOwnerKind::parse(&value).unwrap_or_else(|| {
                tracing::warn!(
                    "Invalid GHCR_PACKAGES_OWNER_KIND (expected org|user). Leaving empty."
                );
                GhcrPackagesOwnerKind::Unconfigured
            }),
            Err(_) => GhcrPackagesOwnerKind::Unconfigured,
        };

        let den_search_provider = std::env::var("DEN_SEARCH_PROVIDER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let brave_search_api_key = std::env::var("BRAVE_SEARCH_API_KEY").unwrap_or_default();
        let den_search_max_results = std::env::var("DEN_SEARCH_MAX_RESULTS")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<usize>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid DEN_SEARCH_MAX_RESULTS, defaulting to 5");
                5
            })
            .clamp(1, 10);

        let qdrant_port = std::env::var("QDRANT_PORT")
            .unwrap_or_else(|_| DEFAULT_QDRANT_PORT.to_string())
            .parse::<u16>()
            .unwrap_or_else(|_| {
                tracing::warn!(
                    "Invalid QDRANT_PORT environment variable. Defaulting to {DEFAULT_QDRANT_PORT}"
                );
                DEFAULT_QDRANT_PORT
            });
        let instance_suffix = std::env::var("BEARS_INSTANCE_SUFFIX").unwrap_or_default();
        let qdrant_url = qdrant_url_from_env(
            std::env::var("QDRANT_URL").ok(),
            qdrant_port,
            &instance_suffix,
        );
        let embedding_standard = std::env::var("EMBEDDING_STANDARD")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bears-embed-v1".to_string());
        let embedding_model = std::env::var("EMBEDDING_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "text-embedding-3-small".to_string());
        let embedding_dimensions: u32 = std::env::var("EMBEDDING_DIMENSIONS")
            .unwrap_or_else(|_| "1536".to_string())
            .parse()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid EMBEDDING_DIMENSIONS, defaulting to 1536");
                1536
            });

        let sandbox_service_token = std::env::var("SANDBOX_SERVICE_TOKEN").unwrap_or_default();
        if std::env::var("SANDBOX_ROOTS_CONFIG").is_ok_and(|s| !s.trim().is_empty()) {
            tracing::warn!(
                "SANDBOX_ROOTS_CONFIG is deprecated and ignored: sandbox roots and the \
                 image catalog are Den-managed (work surfaces + sandbox catalog, synced \
                 to the provider); the file is no longer read"
            );
        }
        let sandbox_workspaces_dir = std::env::var("SANDBOX_WORKSPACES_DIR")
            .unwrap_or_else(|_| "./data/sandbox-workspaces".to_string());
        let sandbox_build_context_dir = std::env::var("SANDBOX_BUILD_CONTEXT_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let sandbox_workspaces_host_dir = std::env::var("SANDBOX_WORKSPACES_HOST_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let sandbox_default_image = std::env::var("SANDBOX_IMAGE").unwrap_or_default();
        let sandbox_max_concurrent = std::env::var("SANDBOX_MAX_CONCURRENT")
            .unwrap_or_else(|_| "2".to_string())
            .parse::<usize>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid SANDBOX_MAX_CONCURRENT, defaulting to 2");
                2
            })
            .max(1);
        let sandbox_default_timeout_secs = std::env::var("SANDBOX_DEFAULT_TIMEOUT_SECS")
            .unwrap_or_else(|_| "900".to_string())
            .parse::<u64>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid SANDBOX_DEFAULT_TIMEOUT_SECS, defaulting to 900");
                900
            });
        let sandbox_max_log_bytes = std::env::var("SANDBOX_MAX_LOG_BYTES")
            .unwrap_or_else(|_| (2 * 1024 * 1024).to_string())
            .parse::<u64>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid SANDBOX_MAX_LOG_BYTES, defaulting to 2 MiB");
                2 * 1024 * 1024
            });

        let sandbox_server_url = sandbox_server_url_from_env(
            std::env::var("SANDBOX_SERVER_URL").ok(),
            run_web,
            run_workers,
            run_sandbox,
            sandbox_port,
            &instance_suffix,
        );
        let sandbox_server_token = std::env::var("SANDBOX_SERVER_TOKEN").unwrap_or_default();
        let sandbox_callback_api_url = sandbox_callback_api_url_from_env(
            std::env::var("SANDBOX_CALLBACK_API_URL").ok(),
            &api_server_url,
            api_port,
            &instance_suffix,
        );
        let work_dispatch_auto = parse_bool_env("WORK_DISPATCH_AUTO", false);
        let work_sandbox_network = std::env::var("WORK_SANDBOX_NETWORK")
            .map(|value| value.trim().to_lowercase())
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "restricted".to_string());
        let sandbox_preserve_failed = parse_bool_env("SANDBOX_PRESERVE_FAILED", false);
        let work_max_attempts = std::env::var("WORK_MAX_ATTEMPTS")
            .unwrap_or_else(|_| "2".to_string())
            .parse::<u32>()
            .unwrap_or_else(|_| {
                tracing::warn!("Invalid WORK_MAX_ATTEMPTS, defaulting to 2");
                2
            })
            .max(1);

        let compaction_mode =
            std::env::var("COMPACTION_MODE").unwrap_or_else(|_| "observe".to_string());

        let compaction_timing =
            std::env::var("COMPACTION_TIMING").unwrap_or_else(|_| "async".to_string());
        let agent_loop_control_mode = parse_choice_env(
            "BEARS_AGENT_LOOP_CONTROL",
            "enforce",
            &["off", "observe", "enforce"],
        );
        let checkpoint_audit_mode =
            parse_choice_env("BEARS_CHECKPOINT_AUDIT", "work", &["off", "work", "all"]);

        Config {
            templates_dir: std::env::var("TEMPLATES_DIR")
                .unwrap_or_else(|_| "crates/den-web/src/templates".to_string()),
            // Required only when a DB-backed service runs; enforced by the
            // binary's `validate_runtime_config`, not here, so a standalone
            // sandbox server (RUN_SANDBOX only) can boot without Postgres.
            database_url: std::env::var("DATABASE_URL").unwrap_or_default(),

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
            run_sandbox,
            web_port,
            api_port,
            sandbox_port,
            web_server_url,
            api_server_url,

            den_secret_encryption_key,
            bifrost_base_url,
            bifrost_management_url,
            bifrost_admin_username,
            bifrost_admin_password,
            bifrost_catalog_refresh_secs,
            llm_api_url,
            llm_api_key,
            default_llm_model,
            model_tool_budget_multipliers,
            bear_sqlite_data_dir,
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
            den_search_provider,
            brave_search_api_key,
            den_search_max_results,
            qdrant_url,
            embedding_standard,
            embedding_model,
            embedding_dimensions,
            sandbox_service_token,
            sandbox_build_context_dir,
            sandbox_workspaces_dir,
            sandbox_workspaces_host_dir,
            sandbox_default_image,
            sandbox_max_concurrent,
            sandbox_default_timeout_secs,
            sandbox_max_log_bytes,
            sandbox_server_url,
            sandbox_server_token,
            sandbox_callback_api_url,
            work_dispatch_auto,
            sandbox_preserve_failed,
            work_max_attempts,
            work_sandbox_network,
            compaction_mode,
            compaction_timing,
            agent_loop_control_mode,
            checkpoint_audit_mode,
            ui_fixture_profile,
        }
    }
}

fn qdrant_url_from_env(
    raw: Option<String>,
    qdrant_port: u16,
    instance_suffix: &str,
) -> Option<String> {
    match raw.and_then(normalize_auto_url_env) {
        Some(value) => {
            let value = value.trim().trim_end_matches('/').to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        None => Some(format!(
            "http://{DEFAULT_QDRANT_HOST}{}:{qdrant_port}",
            instance_suffix.trim()
        )),
    }
}

fn sandbox_server_url_from_env(
    raw: Option<String>,
    run_web: bool,
    run_workers: bool,
    run_sandbox: bool,
    sandbox_port: u16,
    instance_suffix: &str,
) -> Option<String> {
    match raw.and_then(normalize_auto_url_env) {
        Some(value) => {
            let value = value.trim().trim_end_matches('/').to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        None if (run_web || run_workers) && !run_sandbox => Some(format!(
            "http://{DEFAULT_SANDBOX_SERVER_HOST}{}:{sandbox_port}",
            instance_suffix.trim()
        )),
        None => None,
    }
}

fn sandbox_callback_api_url_from_env(
    raw: Option<String>,
    api_server_url: &str,
    api_port: u16,
    instance_suffix: &str,
) -> String {
    match raw.map(|value| value.trim().trim_end_matches('/').to_string()) {
        Some(value) if value.eq_ignore_ascii_case("auto") => {
            format!("http://bears-den{}:{api_port}", instance_suffix.trim())
        }
        Some(value) if !value.is_empty() => value,
        _ => api_server_url.to_string(),
    }
}

fn normalize_auto_url_env(value: String) -> Option<String> {
    if value.trim().eq_ignore_ascii_case("auto") {
        // ponytail: Compose can warn on nested defaults inside URL defaults in some deployers.
        // `auto` is a local sentinel that lets Rust use the parsed *_PORT fallback.
        None
    } else {
        Some(value)
    }
}

fn parse_model_tool_budget_multipliers_env(raw: Option<&str>) -> HashMap<String, f64> {
    let mut multipliers = HashMap::new();
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return multipliers;
    };

    for entry in raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((model, multiplier)) = entry.split_once('=') else {
            tracing::warn!(
                "Ignoring invalid BEARS_MODEL_TOOL_BUDGET_MULTIPLIERS entry '{}'. Expected model=multiplier.",
                entry
            );
            continue;
        };
        let model = model.trim();
        let Ok(multiplier) = multiplier.trim().parse::<f64>() else {
            tracing::warn!(
                "Ignoring invalid tool-budget multiplier '{}' for model '{}'.",
                multiplier.trim(),
                model
            );
            continue;
        };
        if model.is_empty() || !multiplier.is_finite() || multiplier <= 0.0 || multiplier > 10.0 {
            tracing::warn!(
                "Ignoring invalid tool-budget multiplier entry '{}'. Model must be non-empty and multiplier must be in (0, 10].",
                entry
            );
            continue;
        }
        multipliers.insert(model.to_string(), multiplier);
    }

    multipliers
}

fn parse_choice_env(name: &str, default: &str, allowed: &[&str]) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let normalized = value.trim().to_ascii_lowercase();
    if allowed.contains(&normalized.as_str()) {
        normalized
    } else {
        tracing::warn!(
            "Invalid {} value '{}'. Expected one of {:?}. Defaulting to {}.",
            name,
            value,
            allowed,
            default
        );
        default.to_string()
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

#[cfg(any(test, feature = "test-util"))]
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
            run_sandbox: false,
            web_port: 3000,
            api_port: 3001,
            sandbox_port: 3002,
            web_server_url: "http://localhost:3000".into(),
            api_server_url: "http://localhost:3001".into(),

            den_secret_encryption_key: String::new(),
            bifrost_base_url: String::new(),
            bifrost_management_url: String::new(),
            bifrost_admin_username: String::new(),
            bifrost_admin_password: String::new(),
            bifrost_catalog_refresh_secs: 300,
            llm_api_url: String::new(),
            llm_api_key: String::new(),
            default_llm_model: "gpt-4.1".into(),
            model_tool_budget_multipliers: HashMap::new(),
            bear_sqlite_data_dir: "/var/lib/den/bear-sqlite".into(),
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
            ghcr_packages_owner_kind: GhcrPackagesOwnerKind::Unconfigured,
            den_search_provider: String::new(),
            brave_search_api_key: String::new(),
            den_search_max_results: 5,
            qdrant_url: None,
            embedding_standard: "bears-embed-v1".into(),
            embedding_model: "text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            sandbox_service_token: String::new(),
            sandbox_build_context_dir: None,
            sandbox_workspaces_dir: "./data/sandbox-workspaces".into(),
            sandbox_workspaces_host_dir: None,
            sandbox_default_image: String::new(),
            sandbox_max_concurrent: 2,
            sandbox_default_timeout_secs: 900,
            sandbox_max_log_bytes: 2 * 1024 * 1024,
            sandbox_server_url: None,
            sandbox_server_token: String::new(),
            sandbox_callback_api_url: "http://localhost:3001".into(),
            work_dispatch_auto: false,
            sandbox_preserve_failed: false,
            work_max_attempts: 2,
            work_sandbox_network: "restricted".into(),
            compaction_mode: "observe".into(),
            compaction_timing: "async".into(),
            agent_loop_control_mode: "enforce".into(),
            checkpoint_audit_mode: "work".into(),
            ui_fixture_profile: None,
        }
    }
}

/// Whether startup must require a non-empty `JWT_SECRET`.
///
/// Production builds always require it; dev builds require it only when the API
/// service runs (it mints/validates bearer tokens). Shared by the binary's
/// startup validation and the web edge's stack-health check.
#[must_use]
pub fn requires_jwt_secret(config: &Config) -> bool {
    #[cfg(feature = "production")]
    {
        let _ = config;
        true
    }
    #[cfg(not(feature = "production"))]
    {
        config.run_api
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghcr_packages_owner_kind_preserves_api_path_strings() {
        assert_eq!(
            GhcrPackagesOwnerKind::parse(" org "),
            Some(GhcrPackagesOwnerKind::Organization)
        );
        assert_eq!(
            GhcrPackagesOwnerKind::parse("USER"),
            Some(GhcrPackagesOwnerKind::User)
        );
        assert_eq!(GhcrPackagesOwnerKind::parse("team"), None);
        assert_eq!(GhcrPackagesOwnerKind::Unconfigured.as_str(), "");
        assert_eq!(GhcrPackagesOwnerKind::Organization.as_str(), "org");
        assert_eq!(GhcrPackagesOwnerKind::User.as_str(), "user");
    }

    #[test]
    fn qdrant_url_defaults_to_compose_service_port() {
        assert_eq!(
            qdrant_url_from_env(None, 6334, "").as_deref(),
            Some("http://bears-qdrant:6334")
        );
        assert_eq!(qdrant_url_from_env(Some(String::new()), 6333, ""), None);
        assert_eq!(
            qdrant_url_from_env(Some(" http://qdrant:6333/ ".into()), 6333, "").as_deref(),
            Some("http://qdrant:6333")
        );
        assert_eq!(
            qdrant_url_from_env(Some("auto".into()), 6334, "-test").as_deref(),
            Some("http://bears-qdrant-test:6334")
        );
    }

    #[test]
    fn sandbox_callback_auto_uses_instance_dns_name() {
        assert_eq!(
            sandbox_callback_api_url_from_env(
                Some("auto".into()),
                "https://api.example.com",
                3036,
                "-test",
            ),
            "http://bears-den-test:3036"
        );
        assert_eq!(
            sandbox_callback_api_url_from_env(None, "https://api.example.com", 3001, ""),
            "https://api.example.com"
        );
    }

    #[test]
    fn sandbox_server_url_defaults_for_den_processes() {
        assert_eq!(
            sandbox_server_url_from_env(None, false, true, false, 3137, "").as_deref(),
            Some("http://bears-sandbox-provider:3137")
        );
        assert_eq!(
            sandbox_server_url_from_env(None, true, false, false, 3138, "").as_deref(),
            Some("http://bears-sandbox-provider:3138")
        );
        assert_eq!(
            sandbox_server_url_from_env(Some("auto".into()), true, false, false, 3139, "-test",)
                .as_deref(),
            Some("http://bears-sandbox-provider-test:3139")
        );
        assert_eq!(
            sandbox_server_url_from_env(Some(String::new()), true, true, false, 3002, ""),
            None
        );
        assert_eq!(
            sandbox_server_url_from_env(None, false, false, true, 3002, ""),
            None
        );
        assert_eq!(
            sandbox_server_url_from_env(
                Some(" http://sandbox:3002/ ".into()),
                true,
                true,
                false,
                3002,
                ""
            )
            .as_deref(),
            Some("http://sandbox:3002")
        );
    }

    #[test]
    fn parses_model_tool_budget_multipliers() {
        let parsed = parse_model_tool_budget_multipliers_env(Some(
            "openai/gpt-5=1.5, gpt-4.1 = 0.75, bad, empty=0, huge=99",
        ));
        assert_eq!(parsed.get("openai/gpt-5"), Some(&1.5));
        assert_eq!(parsed.get("gpt-4.1"), Some(&0.75));
        assert!(!parsed.contains_key("empty"));
        assert!(!parsed.contains_key("huge"));
    }
}
