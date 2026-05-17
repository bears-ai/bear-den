//! Feature-gated fixture-backed web data sources.
//!
//! These providers exist only when the `web-ui-fixtures` Cargo feature is enabled. They are
//! intended for explicit browser/UI smoke testing in development. They are never active unless the
//! running configuration selects a supported fixture profile.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;

use crate::{
    config::UiFixtureProfile,
    core::memory_manager_head::{
        MemfsRoleMemoryFileResponse, MemfsRoleMemorySearchResponse,
        MemfsRoleMemoryStatusResponse, MemfsRoleMemoryTreeResponse, MemfsViewHealth,
    },
    errors::CustomError,
    web::data::{
        WebChatTransportDataSource, WebConversationRow, WebConversationSnapshot,
        WebLettaDataSource, WebMemoryDataSource,
    },
};

#[derive(Clone)]
pub struct FixtureWebLettaDataSource {
    profile: UiFixtureProfile,
}

impl FixtureWebLettaDataSource {
    pub fn new(profile: UiFixtureProfile) -> Self {
        Self { profile }
    }
}

#[async_trait]
impl WebLettaDataSource for FixtureWebLettaDataSource {
    fn is_enabled(&self) -> bool {
        true
    }

    async fn fetch_agent(&self, agent_id: &str) -> Result<serde_json::Value, CustomError> {
        let warning = matches!(self.profile, UiFixtureProfile::BearDetailsWarning);
        Ok(json!({
            "id": agent_id,
            "name": if warning { "Fixture agent (warning)" } else { "Fixture agent" },
            "system": "Fixture Letta agent for web UI smoke testing.",
            "memory_blocks": [],
            "tools": if warning { json!(["tool-a"]) } else { json!(["tool-a", "tool-b"]) },
        }))
    }

    async fn filtered_tool_ids(&self, tool_ids: &[String]) -> Result<Vec<String>, CustomError> {
        if matches!(self.profile, UiFixtureProfile::BearDetailsWarning) {
            Ok(tool_ids.iter().take(1).cloned().collect())
        } else {
            Ok(tool_ids.to_vec())
        }
    }

    async fn list_agent_conversations(
        &self,
        agent_id: &str,
    ) -> Result<WebConversationSnapshot, CustomError> {
        let rows = vec![
            WebConversationRow {
                id: "default".to_string(),
                title: if agent_id.contains("pair") {
                    "Pair main thread".to_string()
                } else {
                    "Main chat".to_string()
                },
                last_message_at: Some("2026-05-17T15:30:00Z".to_string()),
                archived: false,
            },
            WebConversationRow {
                id: "conv-fixture-1".to_string(),
                title: "Fixture planning thread".to_string(),
                last_message_at: Some("2026-05-17T14:00:00Z".to_string()),
                archived: false,
            },
            WebConversationRow {
                id: "conv-fixture-archived".to_string(),
                title: "Archived fixture thread".to_string(),
                last_message_at: Some("2026-05-16T12:00:00Z".to_string()),
                archived: true,
            },
        ];
        Ok(WebConversationSnapshot { all: rows })
    }

    async fn list_conversation_messages(
        &self,
        conversation_id: &str,
        _agent_id_for_default: Option<&str>,
        _limit: u32,
        _before: Option<&str>,
        _include_full_tool_payloads: bool,
    ) -> Result<serde_json::Value, CustomError> {
        Ok(json!([
            {
                "id": format!("{conversation_id}-2"),
                "date": "2026-05-17T15:31:00Z",
                "message_type": "assistant_message",
                "content": if matches!(self.profile, UiFixtureProfile::BearChatError) {
                    "We hit a fixture-backed warning path."
                } else {
                    "Here is a fixture-backed assistant reply."
                }
            },
            {
                "id": format!("{conversation_id}-1"),
                "date": "2026-05-17T15:30:00Z",
                "message_type": "user_message",
                "content": "Show me the current work"
            }
        ]))
    }

    async fn patch_conversation_summary(
        &self,
        _conversation_id: &str,
        _title: &str,
    ) -> Result<(), CustomError> {
        Ok(())
    }

    async fn patch_conversation_archived(
        &self,
        _conversation_id: &str,
        _archived: bool,
    ) -> Result<(), CustomError> {
        Ok(())
    }

    async fn delete_conversation(&self, _conversation_id: &str) -> Result<(), CustomError> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct FixtureWebMemoryDataSource {
    profile: UiFixtureProfile,
}

impl FixtureWebMemoryDataSource {
    pub fn new(profile: UiFixtureProfile) -> Self {
        Self { profile }
    }
}

#[async_trait]
impl WebMemoryDataSource for FixtureWebMemoryDataSource {
    fn is_configured(&self) -> bool {
        true
    }

    async fn fetch_role_view_health(
        &self,
        _bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsViewHealth>, CustomError> {
        let warning = matches!(self.profile, UiFixtureProfile::BearDetailsWarning);
        Ok(Some(MemfsViewHealth {
            agent_id: format!("fixture-agent-{role}"),
            bear_id: "fixture-bear".to_string(),
            role: role.to_string(),
            state: if warning { "warning" } else { "ok" }.to_string(),
            canonical_tip: Some("abc123".to_string()),
            view_tip: Some("abc123".to_string()),
            quarantined: warning,
            diagnostic: warning.then(|| "Fixture quarantine warning".to_string()),
            canonical_repo: None,
            view_repo: None,
        }))
    }

    async fn fetch_role_memory_status(
        &self,
        _bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryStatusResponse>, CustomError> {
        let warning = matches!(self.profile, UiFixtureProfile::BearDetailsWarning);
        Ok(Some(MemfsRoleMemoryStatusResponse {
            ok: !warning,
            bear_id: "fixture-bear".to_string(),
            role: role.to_string(),
            canonical_tip: Some("abc123".to_string()),
            allowed_prefixes: vec!["core/".to_string(), format!("{role}/")],
            file_count: if warning { 2 } else { 6 },
            entry_count_by_kind: json!({"note": 2, "summary": 1}),
            registered_view_count: 1,
            recent_activity: json!([
                {"event": "write", "detail": "fixture-note.md"},
                {"event": "merge", "detail": "fixture-summary.md"}
            ]),
            error: warning.then(|| "Fixture warning state".to_string()),
        }))
    }

    async fn fetch_role_memory_tree(
        &self,
        _bear_id: Uuid,
        role: &str,
    ) -> Result<Option<MemfsRoleMemoryTreeResponse>, CustomError> {
        Ok(Some(MemfsRoleMemoryTreeResponse {
            ok: true,
            bear_id: "fixture-bear".to_string(),
            role: role.to_string(),
            canonical_tip: Some("abc123".to_string()),
            files: json!([
                {"path": format!("{role}/notes/fixture-note.md"), "kind": "file"},
                {"path": format!("{role}/summaries/fixture-summary.md"), "kind": "file"}
            ]),
            truncated: false,
            total_file_count: 2,
            limit: 50,
            error: None,
        }))
    }

    async fn search_role_memory(
        &self,
        _bear_id: Uuid,
        role: &str,
        query: &str,
        _limit: Option<usize>,
    ) -> Result<Option<MemfsRoleMemorySearchResponse>, CustomError> {
        Ok(Some(MemfsRoleMemorySearchResponse {
            ok: true,
            bear_id: "fixture-bear".to_string(),
            role: role.to_string(),
            query: query.to_string(),
            canonical_tip: Some("abc123".to_string()),
            results: json!([
                {"path": format!("{role}/notes/fixture-note.md"), "preview": "fixture match"}
            ]),
            result_count: 1,
            scanned_file_count: 2,
            limit: 50,
            error: None,
        }))
    }

    async fn fetch_role_memory_file(
        &self,
        _bear_id: Uuid,
        role: &str,
        path: &str,
    ) -> Result<Option<MemfsRoleMemoryFileResponse>, CustomError> {
        Ok(Some(MemfsRoleMemoryFileResponse {
            ok: true,
            bear_id: "fixture-bear".to_string(),
            role: role.to_string(),
            path: path.to_string(),
            canonical_tip: Some("abc123".to_string()),
            content: format!("# Fixture memory\n\nRole: {role}\nPath: {path}\n"),
            size_bytes: 48,
            error: None,
        }))
    }
}

#[derive(Clone)]
pub struct FixtureWebChatTransportDataSource {
    profile: UiFixtureProfile,
}

impl FixtureWebChatTransportDataSource {
    pub fn new(profile: UiFixtureProfile) -> Self {
        Self { profile }
    }
}

#[async_trait]
impl WebChatTransportDataSource for FixtureWebChatTransportDataSource {
    fn is_enabled(&self) -> bool {
        true
    }

    async fn post_bear_channel_message_streaming(
        &self,
        _session_id: &str,
        _conversation_id: &str,
        _bear: &crate::core::bears::Bear,
        _talk_agent_id: &str,
        _user_id: i32,
        _username: Option<&str>,
        _membership_role: Option<&str>,
        _message: &str,
        _runtime_plan: &serde_json::Value,
        _request_id: uuid::Uuid,
    ) -> Result<reqwest::Response, CustomError> {
        Err(CustomError::System(format!(
            "fixture transport profile '{}' is compiled in but not yet wired for streaming responses",
            self.profile.as_str()
        )))
    }
}

pub type FixtureDataSources = (
    Arc<dyn WebLettaDataSource>,
    Arc<dyn WebMemoryDataSource>,
    Arc<dyn WebChatTransportDataSource>,
);

pub fn build_fixture_data_sources(profile: UiFixtureProfile) -> FixtureDataSources {
    (
        Arc::new(FixtureWebLettaDataSource::new(profile)),
        Arc::new(FixtureWebMemoryDataSource::new(profile)),
        Arc::new(FixtureWebChatTransportDataSource::new(profile)),
    )
}
