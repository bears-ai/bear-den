use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    core::letta::{load_agent_conversations, LettaClient},
    errors::CustomError,
};

/// Web-friendly conversation row used by both the details pages and `/v1/chat/*` routes.
#[derive(Debug, Clone)]
pub struct WebConversationRow {
    pub id: String,
    pub title: String,
    pub last_message_at: Option<String>,
    pub archived: bool,
}

/// Web-facing snapshot of Letta conversations for one agent.
#[derive(Debug, Clone)]
pub struct WebConversationSnapshot {
    pub all: Vec<WebConversationRow>,
}

/// Letta-backed data required by Den's web UI.
///
/// This port is intentionally lower-level than page/view-model services so the real handlers,
/// branching, filtering, and template rendering remain in play when fixture data is enabled.
#[async_trait]
pub trait WebLettaDataSource: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn fetch_agent(&self, agent_id: &str) -> Result<Value, CustomError>;

    async fn filtered_tool_ids(&self, tool_ids: &[String]) -> Result<Vec<String>, CustomError>;

    async fn list_agent_conversations(
        &self,
        agent_id: &str,
    ) -> Result<WebConversationSnapshot, CustomError>;

    async fn list_conversation_messages(
        &self,
        conversation_id: &str,
        agent_id_for_default: Option<&str>,
        limit: u32,
        before: Option<&str>,
        include_full_tool_payloads: bool,
    ) -> Result<Value, CustomError>;

    async fn patch_conversation_summary(
        &self,
        conversation_id: &str,
        title: &str,
    ) -> Result<(), CustomError>;

    async fn patch_conversation_archived(
        &self,
        conversation_id: &str,
        archived: bool,
    ) -> Result<(), CustomError>;

    async fn delete_conversation(&self, conversation_id: &str) -> Result<(), CustomError>;
}

#[derive(Clone)]
pub struct RealWebLettaDataSource {
    letta: Arc<LettaClient>,
}

impl RealWebLettaDataSource {
    pub fn new(letta: Arc<LettaClient>) -> Self {
        Self { letta }
    }
}

#[async_trait]
impl WebLettaDataSource for RealWebLettaDataSource {
    fn is_enabled(&self) -> bool {
        self.letta.is_enabled()
    }

    async fn fetch_agent(&self, agent_id: &str) -> Result<Value, CustomError> {
        self.letta.fetch_agent(agent_id).await
    }

    async fn filtered_tool_ids(&self, tool_ids: &[String]) -> Result<Vec<String>, CustomError> {
        self.letta.filtered_tool_ids(tool_ids).await
    }

    async fn list_agent_conversations(
        &self,
        agent_id: &str,
    ) -> Result<WebConversationSnapshot, CustomError> {
        let snapshot = load_agent_conversations(self.letta.as_ref(), agent_id).await;
        Ok(WebConversationSnapshot {
            all: snapshot
                .all
                .into_iter()
                .map(|row| WebConversationRow {
                    id: row.id,
                    title: row.title,
                    last_message_at: row.last_message_at,
                    archived: row.archived,
                })
                .collect(),
        })
    }

    async fn list_conversation_messages(
        &self,
        conversation_id: &str,
        agent_id_for_default: Option<&str>,
        limit: u32,
        before: Option<&str>,
        include_full_tool_payloads: bool,
    ) -> Result<Value, CustomError> {
        self.letta
            .list_conversation_messages(
                conversation_id,
                agent_id_for_default,
                limit,
                before,
                include_full_tool_payloads,
            )
            .await
    }

    async fn patch_conversation_summary(
        &self,
        conversation_id: &str,
        title: &str,
    ) -> Result<(), CustomError> {
        self.letta
            .patch_conversation_summary(conversation_id, title)
            .await
    }

    async fn patch_conversation_archived(
        &self,
        conversation_id: &str,
        archived: bool,
    ) -> Result<(), CustomError> {
        self.letta
            .patch_conversation_archived(conversation_id, archived)
            .await
    }

    async fn delete_conversation(&self, conversation_id: &str) -> Result<(), CustomError> {
        self.letta.delete_conversation(conversation_id).await
    }
}
