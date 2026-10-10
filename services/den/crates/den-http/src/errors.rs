use crate::auth_backend;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use std::fmt;

#[cfg(test)]
mod tests;

pub use den_core::DenError;
use den_core::{ModelAvailabilityFailure, ModelAvailabilityFailureKind};

/// Web-boundary error adapter for the `den` binary.
///
/// `CustomError` is the HTTP-facing error: it adds `axum::IntoResponse`
/// (rendering a safe standalone HTML page) and the auth-layer conversions on top of
/// the shared, web-free [`DenError`] from `den-core`. Service-layer code should
/// prefer `DenError`; it converts here for free via [`From<DenError>`] when it
/// bubbles up through `?` in an HTTP handler.
#[derive(Debug)]
pub enum CustomError {
    Anyhow(anyhow::Error),
    System(String),
    Database(String),
    /// Pool exhaustion or closed — semantically distinct from a query-level Database error.
    DatabaseUnavailable(String),
    Session(String),
    Authentication(String),
    Authorization(String),
    Render(String),
    Parsing(String),
    Email(String),
    NotFound(String),
    ValidationError(String),
    ModelAvailability(ModelAvailabilityFailure),
}

impl CustomError {
    /// Lossless conversion to the web-free [`DenError`] (variants mirror 1:1).
    ///
    /// Used at den-crate boundaries that implement service-layer traits returning
    /// `DenError` (e.g. the `den-tools` capability seams) while still delegating
    /// to existing `CustomError`-returning `core::*` functions. The orphan rule
    /// forbids `impl From<CustomError> for DenError` (both are foreign to the
    /// trait), so this inherent method fills that gap.
    pub fn into_den(self) -> DenError {
        match self {
            CustomError::Anyhow(cause) => DenError::Anyhow(cause),
            CustomError::System(cause) => DenError::System(cause),
            CustomError::Database(cause) => DenError::Database(cause),
            CustomError::DatabaseUnavailable(cause) => DenError::DatabaseUnavailable(cause),
            CustomError::Session(cause) => DenError::Session(cause),
            CustomError::Authentication(cause) => DenError::Authentication(cause),
            CustomError::Authorization(cause) => DenError::Authorization(cause),
            CustomError::Render(cause) => DenError::Render(cause),
            CustomError::Parsing(cause) => DenError::Parsing(cause),
            CustomError::Email(cause) => DenError::Email(cause),
            CustomError::NotFound(cause) => DenError::NotFound(cause),
            CustomError::ValidationError(cause) => DenError::ValidationError(cause),
            CustomError::ModelAvailability(failure) => DenError::ModelAvailability(failure),
        }
    }
}

impl std::error::Error for CustomError {}

// Allow the use of "{}" format specifier
impl fmt::Display for CustomError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CustomError::Anyhow(ref cause) => {
                write!(f, "{cause:?}")
            }
            CustomError::System(ref cause) => {
                write!(f, "Server Error: {cause}")
            }
            CustomError::Database(ref cause) => {
                write!(f, "Database Error: {cause}")
            }
            CustomError::DatabaseUnavailable(ref cause) => {
                write!(f, "Database Unavailable: {cause}")
            }
            CustomError::Session(ref cause) => {
                write!(f, "Session Error: {cause}")
            }
            CustomError::Authentication(ref cause) => {
                write!(f, "Authentication Error: {cause}")
            }
            CustomError::Authorization(ref cause) => {
                write!(f, "Authorization Error: {cause}")
            }
            CustomError::Render(ref cause) => {
                write!(f, "Rendering Error: {cause}")
            }
            CustomError::Parsing(ref cause) => {
                write!(f, "Parsing Error: {cause}")
            }
            CustomError::Email(ref cause) => {
                write!(f, "Email Error: {cause}")
            }
            CustomError::NotFound(ref cause) => write!(f, "Not Found: {cause}"),
            CustomError::ValidationError(ref cause) => {
                write!(f, "Validation Error: {cause}")
            }
            CustomError::ModelAvailability(ref failure) => write!(f, "{failure}"),
        }
    }
}

impl IntoResponse for CustomError {
    fn into_response(self) -> Response {
        if let Self::ModelAvailability(failure) = &self {
            return model_availability_response(failure);
        }
        let error_string = self.to_string();
        let (error_name, status_code, title, summary, recovery) = match &self {
            CustomError::Anyhow(_) => (
                "Server",
                StatusCode::INTERNAL_SERVER_ERROR,
                "Request could not be completed",
                "This request could not be completed.",
                "Try opening the page again shortly.",
            ),
            CustomError::System(_) => (
                "Web server",
                StatusCode::UNPROCESSABLE_ENTITY,
                "Request could not be completed",
                "This request could not be completed.",
                "Try opening the page again shortly.",
            ),
            CustomError::Database(_) => (
                "Database",
                StatusCode::UNPROCESSABLE_ENTITY,
                "Request could not be completed",
                "This request could not be completed.",
                "Try opening the page again shortly.",
            ),
            CustomError::DatabaseUnavailable(_) => (
                "Database Unavailable",
                StatusCode::SERVICE_UNAVAILABLE,
                "Temporarily unavailable",
                "The service is temporarily unavailable.",
                "Try opening the page again shortly.",
            ),
            CustomError::Session(_) => (
                "Session",
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session unavailable",
                "Your session could not be used for this request.",
                "Try opening the page again or sign in again.",
            ),
            CustomError::Authentication(_) => (
                "Authentication",
                StatusCode::UNAUTHORIZED,
                "Sign in to continue",
                "You need to sign in to continue.",
                "Sign in, then open the page again.",
            ),
            CustomError::Authorization(_) => (
                "Authorization",
                StatusCode::FORBIDDEN,
                "Access unavailable",
                "You do not have access to this page or action.",
                "Return home to choose an available action.",
            ),
            CustomError::Parsing(_) => (
                "Parsing",
                StatusCode::UNPROCESSABLE_ENTITY,
                "Check your submission",
                "The submitted information could not be accepted.",
                "Return to the form, check your entries, and try again.",
            ),
            CustomError::Render(_) => (
                "Rendering",
                StatusCode::INTERNAL_SERVER_ERROR,
                "Page unavailable",
                "This page could not be displayed.",
                "Try opening the page again shortly.",
            ),
            CustomError::Email(_) => (
                "Email",
                StatusCode::FAILED_DEPENDENCY,
                "Email request could not be completed",
                "The email request could not be completed.",
                "Check your inbox before trying again shortly.",
            ),
            CustomError::NotFound(_) => (
                "Not Found",
                StatusCode::NOT_FOUND,
                "Page not found",
                "This page or item could not be found.",
                "Check the address or return home.",
            ),
            CustomError::ModelAvailability(_) => unreachable!("handled by typed boundary"),
            CustomError::ValidationError(_) => (
                "Validation",
                StatusCode::BAD_REQUEST,
                "Check your submission",
                "The submitted information could not be accepted.",
                "Return to the form, check your entries, and try again.",
            ),
        };

        tracing::error!("{}: {:#}", error_name, error_string);
        // Self-contained error page: `den-http` is the shared edge foundation and
        // deliberately carries no web template tree (that lives in `den-web`), so the
        // boundary error renders standalone HTML rather than the styled `error.html`.
        let code = status_code.as_u16();
        // This boundary has no trusted viewer identity or request reference. Never render raw causes here.
        let sign_in_link = if matches!(
            self,
            CustomError::Authentication(_) | CustomError::Session(_)
        ) {
            "<p><a href=\"/login\">Sign in</a></p>"
        } else {
            ""
        };
        let body = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>{title}</title><link rel=\"stylesheet\" href=\"/assets/css/style.css\">\
             </head><body><main id=\"content\"><h1>{title}</h1>\
             <p role=\"alert\">{summary}</p><p>{recovery}</p>\
             <p>If you submitted a change, check its current state before submitting it again.</p>\
             {sign_in_link}<p><a href=\"/\">Return home</a></p>\
             <p class=\"caption\">HTTP {code}</p></main></body></html>"
        );
        (status_code, Html(body)).into_response()
    }
}

fn model_availability_response(failure: &ModelAvailabilityFailure) -> Response {
    let descriptor = failure.descriptor();
    let status = match failure.kind {
        ModelAvailabilityFailureKind::ModelMissing
        | ModelAvailabilityFailureKind::ModelUnavailable => StatusCode::BAD_REQUEST,
        ModelAvailabilityFailureKind::VirtualKeyMissing
        | ModelAvailabilityFailureKind::VirtualKeyRejected => StatusCode::CONFLICT,
        ModelAvailabilityFailureKind::CatalogUnavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    tracing::error!(reason = descriptor.code, model = ?failure.model, "Bear model availability check failed");
    // All dynamic text comes from the checked model reference, not an upstream cause.
    let message = failure.public_message();
    let recovery = descriptor.recovery;
    let code = descriptor.code;
    let http_code = status.as_u16();
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Model access unavailable</title><link rel=\"stylesheet\" href=\"/assets/css/style.css\">\
         </head><body><main id=\"content\"><h1>Model access unavailable</h1>\
         <p role=\"alert\">{message}</p><p>{recovery}</p>\
         <p>If you submitted a change, check its current state before submitting it again.</p>\
         <p><a href=\"/\">Return home</a></p>\
         <p class=\"caption\">{code} — HTTP {http_code}</p></main></body></html>"
    );
    (status, Html(body)).into_response()
}

// `From<CustomError> for DenError` IS permitted by the orphan rule here: the impl
// lives in the `den` crate where `CustomError` is local, and a local type appearing
// as the trait's type argument satisfies RFC 2451 even though `DenError`/`From` are
// foreign. This lets runtime/service code that returns the web-free `DenError`
// propagate the few remaining `CustomError`-returning callees via `?`.
impl From<CustomError> for DenError {
    fn from(err: CustomError) -> DenError {
        err.into_den()
    }
}

impl From<DenError> for CustomError {
    fn from(err: DenError) -> CustomError {
        match err {
            DenError::Anyhow(cause) => CustomError::Anyhow(cause),
            DenError::System(cause) => CustomError::System(cause),
            DenError::RunStateConflict {
                operation,
                run_id,
                expected_state,
                actual_state,
            } => CustomError::System(format!(
                "Run state conflict during {operation}: run {run_id} expected state {expected_state}, observed {}",
                actual_state.as_deref().unwrap_or("missing")
            )),
            DenError::Database(cause) => CustomError::Database(cause),
            DenError::DatabaseUnavailable(cause) => CustomError::DatabaseUnavailable(cause),
            DenError::LoopControlLedgerPersistence(cause) => CustomError::System(format!(
                "Loop-control ledger persistence failed: {cause}"
            )),
            DenError::TechnicalBudgetContinuation(cause) => CustomError::System(format!(
                "Technical-budget continuation failed: {cause}"
            )),
            DenError::TechnicalBudgetContinuationAlreadyClaimed { run_id } => {
                CustomError::System(format!(
                    "Technical-budget continuation already claimed for run {run_id}"
                ))
            }
            DenError::Session(cause) => CustomError::Session(cause),
            DenError::Authentication(cause) => CustomError::Authentication(cause),
            DenError::Authorization(cause) => CustomError::Authorization(cause),
            DenError::Render(cause) => CustomError::Render(cause),
            DenError::Parsing(cause) => CustomError::Parsing(cause),
            DenError::Email(cause) => CustomError::Email(cause),
            DenError::NotFound(cause) => CustomError::NotFound(cause),
            DenError::ValidationError(cause) => CustomError::ValidationError(cause),
            DenError::ModelAvailability(failure) => CustomError::ModelAvailability(failure),
        }
    }
}

impl From<anyhow::Error> for CustomError {
    fn from(err: anyhow::Error) -> CustomError {
        CustomError::Anyhow(err)
    }
}

impl From<std::io::Error> for CustomError {
    fn from(err: std::io::Error) -> CustomError {
        CustomError::System(err.to_string())
    }
}

impl From<axum::http::uri::InvalidUri> for CustomError {
    fn from(err: axum::http::uri::InvalidUri) -> CustomError {
        CustomError::System(err.to_string())
    }
}

impl From<sqlx::Error> for CustomError {
    fn from(err: sqlx::Error) -> CustomError {
        match &err {
            sqlx::Error::PoolTimedOut => {
                tracing::error!(
                    "Connection pool exhausted — all connections are busy or broken. \
                     Consider raising DB_MAX_CONNECTIONS (currently hardcoded at build time \
                     or via env) or DB_ACQUIRE_TIMEOUT_SECS. If this repeats, check for \
                     long-running queries or Postgres availability."
                );
                CustomError::DatabaseUnavailable(
                    "pool exhausted: all database connections are busy (pool timed out). \
                     The server cannot handle this request right now."
                        .into(),
                )
            }
            sqlx::Error::PoolClosed => {
                tracing::error!("Database connection pool is closed — the server is shutting down or the pool was dropped.");
                CustomError::DatabaseUnavailable(
                    "database pool closed — the server may be shutting down".into(),
                )
            }
            _ => CustomError::Database(err.to_string()),
        }
    }
}

impl From<axum_login::tower_sessions::session::Error> for CustomError {
    fn from(err: axum_login::tower_sessions::session::Error) -> CustomError {
        CustomError::Session(err.to_string())
    }
}

impl From<auth_backend::Error> for CustomError {
    fn from(err: auth_backend::Error) -> CustomError {
        CustomError::Authentication(err.to_string())
    }
}

impl From<axum_login::Error<auth_backend::Backend>> for CustomError {
    fn from(err: axum_login::Error<auth_backend::Backend>) -> CustomError {
        CustomError::Authentication(err.to_string())
    }
}

impl From<serde_json::Error> for CustomError {
    fn from(err: serde_json::Error) -> CustomError {
        CustomError::Parsing(err.to_string())
    }
}

impl From<sqlx::types::uuid::Error> for CustomError {
    fn from(err: sqlx::types::uuid::Error) -> CustomError {
        CustomError::Parsing(err.to_string())
    }
}

impl From<mailgun_rs::SendError> for CustomError {
    fn from(err: mailgun_rs::SendError) -> CustomError {
        CustomError::Email(err.to_string())
    }
}

impl From<validator::ValidationErrors> for CustomError {
    fn from(err: validator::ValidationErrors) -> CustomError {
        CustomError::ValidationError(err.to_string())
    }
}

impl From<reqwest::Error> for CustomError {
    fn from(err: reqwest::Error) -> CustomError {
        CustomError::System(err.to_string())
    }
}
