//! Safe JSON failures for chat APIs; page and task error boundaries stay separate.

use axum::{
    extract::rejection::JsonRejection,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::QueryRejection;
use den_core::DenError;
use serde::Serialize;
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug)]
pub(super) enum ChatApiError {
    InvalidQuery,
    InvalidBody(StatusCode),
    ConversationReadOnly,
    ConversationArchived,
    ConversationUnavailable,
    Service(DenError),
}

struct ErrorDescriptor {
    status: StatusCode,
    code: &'static str,
    message: String,
    model: Option<String>,
    recovery: Option<&'static str>,
}

impl ChatApiError {
    pub(super) fn ordinary_source(error: DenError) -> Self {
        match error {
            DenError::Authorization(_) | DenError::NotFound(_) => Self::ConversationReadOnly,
            error => error.into(),
        }
    }

    fn descriptor(&self) -> ErrorDescriptor {
        if let Self::Service(DenError::ModelAvailability(failure)) = self {
            let descriptor = failure.descriptor();
            return ErrorDescriptor {
                status: crate::model_availability::availability_status(failure),
                code: descriptor.code,
                message: format!(
                    "{} {} See Bear → Models.",
                    failure.public_message(),
                    descriptor.recovery
                ),
                model: failure
                    .model
                    .as_ref()
                    .map(|model| model.as_str().to_owned()),
                recovery: Some(descriptor.recovery),
            };
        }
        let (status, code, message) = match self {
            Self::InvalidQuery => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Check the chat request parameters.",
            ),
            Self::InvalidBody(status) => (
                *status,
                "invalid_request",
                "Submit a valid JSON chat request.",
            ),
            Self::ConversationReadOnly => (
                StatusCode::FORBIDDEN,
                "conversation_read_only",
                "This action requires your own active conversation bound to a named hat. Start a new chat with a hat.",
            ),
            Self::ConversationArchived => (
                StatusCode::FORBIDDEN,
                "conversation_archived",
                "Unarchive this conversation before continuing.",
            ),
            Self::ConversationUnavailable => (
                StatusCode::NOT_FOUND,
                "conversation_unavailable",
                "The requested conversation is unavailable. Choose an available chat.",
            ),
            Self::Service(error) => match error {
                DenError::Authentication(_) => (
                    StatusCode::UNAUTHORIZED,
                    "authentication_required",
                    "Sign in to continue.",
                ),
                DenError::Authorization(_) => (
                    StatusCode::FORBIDDEN,
                    "access_unavailable",
                    "You do not have access to this chat or action.",
                ),
                DenError::NotFound(_) => (
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "The requested chat or item could not be found.",
                ),
                DenError::Parsing(_) | DenError::ValidationError(_) => (
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "The submitted chat information could not be accepted.",
                ),
                DenError::DatabaseUnavailable(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "temporarily_unavailable",
                    "Chat is temporarily unavailable. Try again shortly.",
                ),
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "This chat request could not be completed. Check its current state before retrying.",
                ),
            },
        };
        ErrorDescriptor {
            status,
            code,
            message: message.into(),
            model: None,
            recovery: None,
        }
    }

    pub(super) fn response(self, request_id: Uuid) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
            error: String,
            message: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            recovery: Option<&'static str>,
            request_id: Uuid,
        }
        let descriptor = self.descriptor();
        tracing::error!(%request_id, error = ?self, "chat API rejected");
        let mut response = (
            descriptor.status,
            Json(ErrorBody {
                code: descriptor.code,
                error: descriptor.message.clone(),
                message: descriptor.message,
                model: descriptor.model,
                recovery: descriptor.recovery,
                request_id,
            }),
        )
            .into_response();
        response.headers_mut().insert(
            "x-request-id",
            HeaderValue::from_str(&request_id.to_string()).expect("UUID is a valid header"),
        );
        response
    }
}

impl IntoResponse for ChatApiError {
    fn into_response(self) -> Response {
        self.response(Uuid::new_v4())
    }
}

impl From<DenError> for ChatApiError {
    fn from(error: DenError) -> Self {
        Self::Service(error)
    }
}

impl From<CustomError> for ChatApiError {
    fn from(error: CustomError) -> Self {
        Self::Service(error.into_den())
    }
}

impl From<sqlx::Error> for ChatApiError {
    fn from(error: sqlx::Error) -> Self {
        DenError::from(error).into()
    }
}

impl From<QueryRejection> for ChatApiError {
    fn from(_: QueryRejection) -> Self {
        Self::InvalidQuery
    }
}

impl From<JsonRejection> for ChatApiError {
    fn from(error: JsonRejection) -> Self {
        Self::InvalidBody(error.status())
    }
}
