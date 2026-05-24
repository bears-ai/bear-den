use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    core::{
        bears::Bear,
        codepool::{BearRuntimeClient, BearRuntimeMessageRequest},
    },
    errors::CustomError,
};

/// Streaming transport required by `/v1/chat/send`.
pub struct WebChatTransportRequest<'a> {
    pub session_id: &'a str,
    pub conversation_id: &'a str,
    pub bear: &'a Bear,
    pub talk_agent_id: &'a str,
    pub user_id: i32,
    pub username: Option<&'a str>,
    pub membership_role: Option<&'a str>,
    pub message: &'a str,
    pub runtime_plan: &'a serde_json::Value,
    pub request_id: Uuid,
}

#[async_trait]
pub trait WebChatTransportDataSource: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn post_bear_channel_message_streaming(
        &self,
        request: WebChatTransportRequest<'_>,
    ) -> Result<reqwest::Response, CustomError>;
}

#[derive(Clone)]
pub struct RealWebChatTransportDataSource {
    codepool: Arc<dyn BearRuntimeClient + Send + Sync>,
    enabled: bool,
}

impl RealWebChatTransportDataSource {
    pub fn new(codepool: Arc<dyn BearRuntimeClient + Send + Sync>, enabled: bool) -> Self {
        Self { codepool, enabled }
    }
}

#[async_trait]
impl WebChatTransportDataSource for RealWebChatTransportDataSource {
    fn is_enabled(&self) -> bool {
        self.enabled
    }

    async fn post_bear_channel_message_streaming(
        &self,
        request: WebChatTransportRequest<'_>,
    ) -> Result<reqwest::Response, CustomError> {
        let WebChatTransportRequest {
            session_id,
            conversation_id,
            bear,
            talk_agent_id,
            user_id,
            username,
            membership_role,
            message,
            runtime_plan,
            request_id,
        } = request;
        self.codepool
            .post_bear_channel_message_streaming(BearRuntimeMessageRequest {
                session_id,
                conversation_id,
                bear,
                role_agent_id: talk_agent_id,
                user_id,
                username,
                membership_role,
                user_input: message,
                runtime_plan,
                request_id,
            })
            .await
    }
}
