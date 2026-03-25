#![allow(dead_code)]

#[cfg(not(feature = "production"))]
static URL_PREFIX: &str = "https://redirectmeto.com/http://localhost:3000/";
#[cfg(feature = "production")]
static URL_PREFIX: &str = "https://newapp.example/";

#[derive(Clone, Debug)]
pub struct Config {
    pub templates_dir: String,
    pub database_url: String,

    pub mailgun_api_key: String,
    pub mailgun_domain: String,

    pub telemetry_url_prefix: String,
    pub email_verify_url_prefix: String,

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

    pub web_server_url: String,
    pub api_server_url: String,
}

impl Config {
    /// Load configuration from process environment (and optional `.env` via dotenvy).
    ///
    /// Call once at process startup; thread an [`std::sync::Arc`] through services that need it.
    pub fn load() -> Self {
        if dotenvy::dotenv().is_ok() {
            tracing::info!("Loaded .env file");
        }

        let email_verify_url_prefix = format!("{URL_PREFIX}settings/email/verify/");

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
                "https://newapp.example".to_string()
            }
            #[cfg(not(feature = "production"))]
            {
                format!("http://localhost:{web_port}")
            }
        });

        let api_server_url = std::env::var("API_SERVER_URL").unwrap_or_else(|_| {
            #[cfg(feature = "production")]
            {
                "https://api.newapp.example".to_string()
            }
            #[cfg(not(feature = "production"))]
            {
                format!("http://localhost:{api_port}")
            }
        });

        let session_cookie_domain = std::env::var("SESSION_COOKIE_DOMAIN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Config {
            templates_dir: std::env::var("TEMPLATES_DIR")
                .unwrap_or("src/web/templates".to_string()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL"),

            mailgun_api_key: std::env::var("MAILGUN_API_KEY").unwrap_or_default(),
            mailgun_domain: std::env::var("MAILGUN_DOMAIN").unwrap_or_default(),

            telemetry_url_prefix: format!("{URL_PREFIX}telemetry/"),
            email_verify_url_prefix,

            session_cookie_domain,

            run_web,
            run_api,
            run_workers,
            web_port,
            api_port,
            web_server_url,
            api_server_url,
        }
    }
}
