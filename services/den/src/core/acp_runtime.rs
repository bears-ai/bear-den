use sqlx::PgPool;

use crate::{
    core::{
        acp_sessions,
        bears::{db as bears_db, model::BearAgentRole, Bear},
        letta::{load_agent_conversations, LettaClient},
        runtime_contracts::{
            AcpConversationRuntime, EnsureConversationRequest, EnsureConversationResult,
            RoleProfileRegistry, RoleRuntimeBinding, RuntimeConversationRef, RuntimeHistoryRecord,
        },
    },
    errors::CustomError,
};

pub fn acp_missing_pair_binding_message(bear_slug: &str) -> String {
    format!(
        "ACP requires this Bear to have a provisioned `pair` role Letta compatibility binding, but none is recorded for bear `{bear_slug}`. Ask an operator to open Admin → Bears → this Bear and click `Provision missing role agents`, then retry."
    )
}

pub async fn require_pair_runtime_binding(
    pool: &PgPool,
    letta: &LettaClient,
    bear: &Bear,
) -> Result<RoleRuntimeBinding, CustomError> {
    if !letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL); ACP pair role cannot run.".to_string(),
        ));
    }
    bears_db::role_runtime_binding_id(pool, bear.id, BearAgentRole::Pair)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|binding_id| RoleRuntimeBinding {
            binding_id,
            compatibility_backend: Some("letta".to_string()),
        })
        .ok_or_else(|| CustomError::ValidationError(acp_missing_pair_binding_message(&bear.slug)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpConversationSelectionSource {
    Explicit,
    Resolved,
    Stored,
    Generated,
}

impl AcpConversationSelectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Resolved => "resolved",
            Self::Stored => "stored",
            Self::Generated => "generated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConversationResolution {
    pub session_selection: String,
    pub resolved_conversation: Option<RuntimeConversationRef>,
    pub upstream_target: String,
    pub selection_source: AcpConversationSelectionSource,
    pub history_target: Option<RuntimeConversationRef>,
    pub archive_target: Option<RuntimeConversationRef>,
    pub requires_belongs_to_bear_check: bool,
}

impl AcpConversationResolution {
    pub fn from_selection(
        session_selection: String,
        selection_source: AcpConversationSelectionSource,
        binding: &RoleRuntimeBinding,
        existing_session: Option<&acp_sessions::AcpSessionRow>,
    ) -> Self {
        let resolved_conversation = if is_acp_history_target(&session_selection) {
            Some(RuntimeConversationRef {
                id: session_selection.clone(),
            })
        } else if existing_session.is_some_and(|s| s.conversation_id.trim() == session_selection) {
            normalized_durable_acp_conversation_id(
                existing_session.and_then(|s| s.resolved_conversation_id.as_deref()),
            )
            .map(|id| RuntimeConversationRef { id })
        } else {
            None
        };
        let upstream_target = if session_selection.starts_with("new-") {
            binding.binding_id.clone()
        } else {
            session_selection.clone()
        };
        let history_target = resolved_conversation
            .as_ref()
            .filter(|c| is_acp_history_target(&c.id))
            .cloned();
        let archive_target = resolved_conversation
            .as_ref()
            .filter(|c| is_acp_archive_target(&c.id))
            .cloned();
        let requires_belongs_to_bear_check = selection_source
            == AcpConversationSelectionSource::Explicit
            && session_selection.starts_with("conv-");

        Self {
            session_selection,
            resolved_conversation,
            upstream_target,
            selection_source,
            history_target,
            archive_target,
            requires_belongs_to_bear_check,
        }
    }
}

pub fn is_valid_pending_acp_conversation_id(conversation_id: &str) -> bool {
    conversation_id.starts_with("new-")
        && conversation_id.len() <= 42
        && normalize_acp_conversation_id(Some(conversation_id)).is_ok()
}

pub fn is_acp_history_target(conversation_id: &str) -> bool {
    conversation_id == "default" || conversation_id.starts_with("conv-")
}

pub fn is_acp_archive_target(conversation_id: &str) -> bool {
    conversation_id.starts_with("conv-")
}

pub fn normalized_durable_acp_conversation_id(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| is_acp_history_target(s))
        .map(str::to_string)
}

pub fn normalize_acp_conversation_id(raw: Option<&str>) -> Result<String, CustomError> {
    let s = raw.unwrap_or("default").trim();
    if s.is_empty() {
        return Ok("default".to_string());
    }
    let ok = s == "default"
        || (s.starts_with("conv-") && s.len() >= 8)
        || (s.starts_with("new-") && s.len() >= 8)
        || s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(CustomError::ValidationError(format!(
            "invalid conversation_id (expected 'default', a Letta conv- id, or a pending new- id): {s}"
        )))
    }
}

pub fn resolve_acp_prompt_conversation(
    requested_raw: Option<&str>,
    existing_session: Option<&acp_sessions::AcpSessionRow>,
    binding: &RoleRuntimeBinding,
    generated_pending_id: String,
) -> Result<AcpConversationResolution, CustomError> {
    let requested = requested_raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| normalize_acp_conversation_id(Some(s)))
        .transpose()?
        .filter(|id| id != "default");

    let (session_selection, source) = if let Some(id) = requested {
        (id, AcpConversationSelectionSource::Explicit)
    } else if let Some(id) = existing_session
        .and_then(|s| normalized_durable_acp_conversation_id(s.resolved_conversation_id.as_deref()))
    {
        (id, AcpConversationSelectionSource::Resolved)
    } else if let Some(id) = existing_session
        .map(|s| s.conversation_id.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| s.starts_with("conv-") || is_valid_pending_acp_conversation_id(s))
        .map(str::to_string)
    {
        (id, AcpConversationSelectionSource::Stored)
    } else {
        (
            generated_pending_id,
            AcpConversationSelectionSource::Generated,
        )
    };

    Ok(AcpConversationResolution::from_selection(
        session_selection,
        source,
        binding,
        existing_session,
    ))
}

pub async fn ensure_acp_session_conversation(
    letta: &LettaClient,
    request: EnsureConversationRequest,
    existing_session: Option<&acp_sessions::AcpSessionRow>,
    generated_pending_id: String,
) -> Result<(AcpConversationResolution, EnsureConversationResult), CustomError> {
    let mut resolution = resolve_acp_prompt_conversation(
        request.requested_selection.as_deref(),
        existing_session,
        &request.binding,
        generated_pending_id,
    )?;
    let mut created = false;
    if resolution.session_selection.starts_with("new-")
        && resolution.resolved_conversation.is_none()
    {
        let created_response = letta
            .create_conversation_for_agent(&request.binding.binding_id)
            .await?;
        let conv_id =
            letta_conversation_id_from_create_response(&created_response).ok_or_else(|| {
                CustomError::System(format!(
                "Letta create conversation response did not contain a conv-* id: {created_response}"
            ))
            })?;
        let conversation = RuntimeConversationRef {
            id: conv_id.clone(),
        };
        resolution.resolved_conversation = Some(conversation.clone());
        resolution.history_target = Some(conversation.clone());
        resolution.archive_target = Some(conversation.clone());
        resolution.upstream_target = conv_id;
        created = true;
    }
    let conversation =
        resolution
            .resolved_conversation
            .clone()
            .unwrap_or_else(|| RuntimeConversationRef {
                id: resolution.upstream_target.clone(),
            });
    Ok((
        resolution,
        EnsureConversationResult {
            conversation,
            created,
        },
    ))
}

pub async fn verify_acp_conversation_belongs_to_binding(
    letta: &LettaClient,
    binding: &RoleRuntimeBinding,
    conversation_id: &str,
) -> Result<(), CustomError> {
    if conversation_id == "default" || conversation_id.starts_with("new-") {
        return Ok(());
    }
    if !conversation_id.starts_with("conv-") {
        return Err(CustomError::ValidationError(format!(
            "invalid conversation_id: {conversation_id}"
        )));
    }
    if !letta.is_enabled() {
        return Err(CustomError::System(
            "Letta is not configured (set LETTA_BASE_URL)".to_string(),
        ));
    }
    let binding_id = binding.binding_id.trim();
    if binding_id.is_empty() {
        return Err(CustomError::ValidationError(
            "this bear role is not linked to a runtime compatibility binding".to_string(),
        ));
    }
    let snap = load_agent_conversations(letta, binding_id).await;
    let found = snap.all.iter().any(|row| row.id == conversation_id);
    if found {
        Ok(())
    } else {
        Err(CustomError::Authorization(
            "conversation not found for this bear".to_string(),
        ))
    }
}

fn letta_conversation_id_from_create_response(value: &serde_json::Value) -> Option<String> {
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| s.starts_with("conv-"))
        .map(str::to_string)
}

pub struct LettaAcpConversationRuntime<'a> {
    pub letta: &'a LettaClient,
}

impl RoleProfileRegistry for LettaAcpConversationRuntime<'_> {
    async fn resolve_compatibility_binding(
        &self,
        _bear_id: uuid::Uuid,
        _role: &str,
    ) -> Result<Option<RoleRuntimeBinding>, CustomError> {
        Ok(None)
    }
}

impl AcpConversationRuntime for LettaAcpConversationRuntime<'_> {
    async fn ensure_session_conversation(
        &self,
        request: EnsureConversationRequest,
    ) -> Result<EnsureConversationResult, CustomError> {
        let (resolution, result) = ensure_acp_session_conversation(
            self.letta,
            request,
            None,
            "new-acp-runtime-placeholder".to_string(),
        )
        .await?;
        let _ = resolution;
        Ok(result)
    }

    async fn load_history(
        &self,
        _binding: &RoleRuntimeBinding,
        _conversation: &RuntimeConversationRef,
    ) -> Result<Vec<RuntimeHistoryRecord>, CustomError> {
        Ok(Vec::new())
    }
}
