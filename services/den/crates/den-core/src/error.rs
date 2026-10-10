//! `DenError`: the shared, web-free error type for den crates.
//!
//! This is the canonical error returned by foundation and service-layer code
//! (`den-core`, `den-llm`, `den-memory`, `den-docket`, `den-tools`, ...). It
//! deliberately carries **no** HTTP/web coupling: rendering an error into an
//! HTTP response (status codes, the `error.html` page) is the job of the
//! web-boundary adapter in the `den` crate (`CustomError`), which provides
//! `From<DenError>` and `axum::IntoResponse`.
//!
//! See `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md` (error-decoupling, "option 2").

use std::fmt;

/// Categorized application error shared across den crates.
///
/// Variants mirror the web-boundary `CustomError` one-to-one so conversion is
/// lossless; the difference is purely that `DenError` has no web dependencies.
#[derive(Debug)]
pub enum DenError {
    Anyhow(anyhow::Error),
    System(String),
    RunStateConflict {
        operation: &'static str,
        run_id: String,
        expected_state: &'static str,
        actual_state: Option<String>,
    },
    Database(String),
    /// Pool exhaustion or closed — semantically distinct from a query-level Database error.
    DatabaseUnavailable(String),
    /// A loop-control decision could not be persisted after model execution completed.
    LoopControlLedgerPersistence(String),
    /// Pair could not begin an otherwise eligible technical-budget continuation.
    TechnicalBudgetContinuation(String),
    /// Another continuation worker already owns this Pair technical-budget successor.
    TechnicalBudgetContinuationAlreadyClaimed {
        run_id: String,
    },
    Session(String),
    Authentication(String),
    Authorization(String),
    Render(String),
    Parsing(String),
    Email(String),
    NotFound(String),
    ValidationError(String),
    ModelAvailability(crate::model_availability::ModelAvailabilityFailure),
}

impl std::error::Error for DenError {}

impl fmt::Display for DenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DenError::Anyhow(ref cause) => write!(f, "{cause:?}"),
            DenError::System(ref cause) => write!(f, "Server Error: {cause}"),
            DenError::RunStateConflict {
                operation,
                ref run_id,
                expected_state,
                ref actual_state,
            } => write!(
                f,
                "Run state conflict during {operation}: run {run_id} expected state {expected_state}, observed {}",
                actual_state.as_deref().unwrap_or("missing")
            ),
            DenError::Database(ref cause) => write!(f, "Database Error: {cause}"),
            DenError::DatabaseUnavailable(ref cause) => write!(f, "Database Unavailable: {cause}"),
            DenError::LoopControlLedgerPersistence(ref cause) => {
                write!(f, "Loop-control ledger persistence failed: {cause}")
            }
            DenError::TechnicalBudgetContinuation(ref cause) => {
                write!(f, "Technical-budget continuation failed: {cause}")
            }
            DenError::TechnicalBudgetContinuationAlreadyClaimed { ref run_id } => {
                write!(f, "Technical-budget continuation already claimed for run {run_id}")
            }
            DenError::Session(ref cause) => write!(f, "Session Error: {cause}"),
            DenError::Authentication(ref cause) => write!(f, "Authentication Error: {cause}"),
            DenError::Authorization(ref cause) => write!(f, "Authorization Error: {cause}"),
            DenError::Render(ref cause) => write!(f, "Rendering Error: {cause}"),
            DenError::Parsing(ref cause) => write!(f, "Parsing Error: {cause}"),
            DenError::Email(ref cause) => write!(f, "Email Error: {cause}"),
            DenError::NotFound(ref cause) => write!(f, "Not Found: {cause}"),
            DenError::ValidationError(ref cause) => write!(f, "Validation Error: {cause}"),
            DenError::ModelAvailability(ref failure) => write!(f, "{failure}"),
        }
    }
}

impl From<anyhow::Error> for DenError {
    fn from(err: anyhow::Error) -> DenError {
        DenError::Anyhow(err)
    }
}

impl From<std::io::Error> for DenError {
    fn from(err: std::io::Error) -> DenError {
        DenError::System(err.to_string())
    }
}

impl From<sqlx::Error> for DenError {
    fn from(err: sqlx::Error) -> DenError {
        match &err {
            sqlx::Error::PoolTimedOut => {
                tracing::error!(
                    "Connection pool exhausted — all connections are busy or broken. \
                     Consider raising DB_MAX_CONNECTIONS (currently hardcoded at build time \
                     or via env) or DB_ACQUIRE_TIMEOUT_SECS. If this repeats, check for \
                     long-running queries or Postgres availability."
                );
                DenError::DatabaseUnavailable(
                    "pool exhausted: all database connections are busy (pool timed out). \
                     The server cannot handle this request right now."
                        .into(),
                )
            }
            sqlx::Error::PoolClosed => {
                tracing::error!("Database connection pool is closed — the server is shutting down or the pool was dropped.");
                DenError::DatabaseUnavailable(
                    "database pool closed — the server may be shutting down".into(),
                )
            }
            _ => DenError::Database(err.to_string()),
        }
    }
}

impl From<sqlx::types::uuid::Error> for DenError {
    fn from(err: sqlx::types::uuid::Error) -> DenError {
        DenError::Parsing(err.to_string())
    }
}

impl From<serde_json::Error> for DenError {
    fn from(err: serde_json::Error) -> DenError {
        DenError::Parsing(err.to_string())
    }
}

impl From<reqwest::Error> for DenError {
    fn from(err: reqwest::Error) -> DenError {
        DenError::System(err.to_string())
    }
}
