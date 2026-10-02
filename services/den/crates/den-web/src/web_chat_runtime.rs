use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use uuid::Uuid;

use crate::errors::CustomError;
use den_protocol::RuntimeStreamEvent;

pub type WebChatRuntimeStream =
    Pin<Box<dyn Stream<Item = Result<RuntimeStreamEvent, CustomError>> + Send>>;

#[derive(Debug, Clone)]
pub struct WebChatRuntimeRequest {
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub turn_binding_id: String,
    pub user_id: i32,
    pub username: Option<String>,
    pub membership_role: Option<String>,
    pub conversation_id: String,
    pub session_id: String,
    pub prompt: String,
    pub request_id: Uuid,
}

pub trait WebChatRuntime: Send + Sync {
    fn stream_chat(
        &self,
        state: &crate::web::AppState,
        request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>>;
}

#[derive(Debug, Default)]
pub struct UnavailableWebChatRuntime;

impl WebChatRuntime for UnavailableWebChatRuntime {
    fn stream_chat(
        &self,
        _state: &crate::web::AppState,
        _request: WebChatRuntimeRequest,
    ) -> futures::future::BoxFuture<'static, Result<WebChatRuntimeStream, CustomError>> {
        Box::pin(async {
            Err(CustomError::System(
                "native web chat runtime is not wired into this web app".to_string(),
            ))
        })
    }
}

pub fn unavailable_web_chat_runtime() -> Arc<dyn WebChatRuntime> {
    Arc::new(UnavailableWebChatRuntime)
}
