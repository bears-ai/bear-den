//! Browser task choices are a projection of human-authorized canonical session tasks.

use crate::errors::CustomError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use den_core::DenError;
use den_docket::{
    task_list_projection_from_session_tasks_with_current_task, DocketTaskProjection,
    TaskListItemStatus,
};
use den_service::{bears::RuntimeContextLabel, client_sessions::ClientSessionRow};
use serde::Serialize;
use uuid::Uuid;

pub(super) struct ChatTaskError(CustomError);

impl From<CustomError> for ChatTaskError {
    fn from(error: CustomError) -> Self {
        Self(error)
    }
}

impl From<DenError> for ChatTaskError {
    fn from(error: DenError) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for ChatTaskError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            message: String,
        }
        let status = match &self.0 {
            CustomError::Authentication(_) => StatusCode::UNAUTHORIZED,
            CustomError::Authorization(_) => StatusCode::FORBIDDEN,
            CustomError::NotFound(_) => StatusCode::NOT_FOUND,
            CustomError::ValidationError(_) => StatusCode::BAD_REQUEST,
            CustomError::DatabaseUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                message: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ChatTaskChoice {
    pub id: Uuid,
    pub title: String,
    pub status: TaskListItemStatus,
}

pub(super) fn task_choices(
    bear_id: Uuid,
    session: &ClientSessionRow,
    tasks: &[DocketTaskProjection],
) -> Vec<ChatTaskChoice> {
    let Some(projection) = task_list_projection_from_session_tasks_with_current_task(
        bear_id,
        RuntimeContextLabel::ArmatureConversation,
        &session.conversation_id,
        session.id,
        Some(&session.client_session_id),
        tasks,
        None,
    ) else {
        return Vec::new();
    };
    projection
        .items
        .into_iter()
        .filter_map(|item| {
            let task = tasks
                .iter()
                .find(|task| task.task.id.to_string() == item.id)?;
            Some(ChatTaskChoice {
                id: task.task.id,
                title: item.title,
                status: item.status,
            })
        })
        .collect()
}
