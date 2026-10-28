use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use den_core::DenError;
use den_oauth::auth::ApiError;
use serde_json::json;

#[derive(Debug)]
pub(super) struct Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "conversation_unavailable",
            "Conversation is unavailable.",
        )
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
}
impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::new(error.status, error.error_code, error.message)
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::from(DenError::from(error))
    }
}
impl From<DenError> for Error {
    fn from(error: DenError) -> Self {
        match error {
            DenError::NotFound(_) => Self::unavailable(),
            DenError::Authorization(_) => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "This action is not authorized for this source.",
            ),
            DenError::ValidationError(_) => {
                Self::invalid("The requested action is not valid for this source.")
            }
            DenError::ModelAvailability(_) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "model_unavailable",
                "The configured model is unavailable.",
            ),
            other => {
                tracing::error!(error = %other, "native channel operation failed");
                Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "service_unavailable",
                    "The operation could not be completed. Refresh before retrying.",
                )
            }
        }
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":self.code,"message":self.message})),
        )
            .into_response()
    }
}
