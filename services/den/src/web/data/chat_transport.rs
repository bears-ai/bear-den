use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    core::{bears::Bear, codepool::BearRuntimeClient},
    errors::CustomError,
};

/// Streaming transport required by `/v1/chat/send`.
#[async_trait]
pub trait WebChatTransportDataSource: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn post_bear_channel_message_streaming(
        &self,
        session_id: &str,
        conversation_id: &str,
        bear: &Bear,
        talk_agent_id: &str,
        user_id: i32,
        username: Option<&str>,
        membership_role: Option<&str>,
        message: &str,
        runtime_plan: &serde_json::Value,
        request_id: Uuid,
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
        session_id: &str,
        conversation_id: &str,
        bear: &Bear,
        talk_agent_id: &str,
        user_id: i32,
        username: Option<&str>,
        membership_role: Option<&str>,
        message: &str,
        runtime_plan: &serde_json::Value,
        request_id: Uuid,
    ) -> Result<reqwest::Response, CustomError> {
        self.codepool
            .post_bear_channel_message_streaming(
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
            )
            .await
    }
}
