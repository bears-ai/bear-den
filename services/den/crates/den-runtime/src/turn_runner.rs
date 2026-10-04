//! Runtime-side client turn contracts: the start/continue request inputs the native runtime
//! consumes, the stream context, and conversation materialization.
//!
//! The adapter edge orchestration (retry wrappers, stale-state cleanup) stays in `den`'s
//! adapter turn-runner glue, which re-exports these types for existing call sites.

use sqlx::PgPool;
use uuid::Uuid;

use den_core::{
    config::Config,
    ids::{BearId, UserId},
    DenError,
};

use den_memory::MemoryStoreManager;
use den_protocol::{
    CheckpointAuditContext, RuntimeContinuation, RuntimeConversationBackend, RuntimeConversationRef,
};
use den_service::conversation::{
    persistence::get_conversation_for_external_id, viewer::ConversationViewer,
};

use crate::llm::LlmApiStyle;

/// Shown to the model when stale-approval recovery auto-denies an expired tool approval.
pub const STALE_APPROVAL_RECOVERY_DENIAL_REASON: &str = "BEARS closed an expired client approval request during stale-approval recovery. This denial applies only to that stale request; it is not a user or web policy block. Retry the tool if it is still needed.";

pub struct TurnStartRequest<'a> {
    pub sqlx_pool: &'a PgPool,
    pub config: &'a Config,
    pub memory_stores: &'a MemoryStoreManager,
    pub request_id: Uuid,
    pub run_id: Option<&'a str>,
    /// Opaque adapter-resolved Work/Job correlation persisted only with checkpoints.
    pub checkpoint_audit_context: Option<CheckpointAuditContext>,
    pub user_id: i32,
    pub session_id: &'a str,
    pub bear_id: Uuid,
    pub bear_slug: &'a str,
    pub client: &'a str,
    pub cwd: Option<&'a str>,
    pub workspace_roots: Option<&'a [String]>,
    pub binding: &'a den_protocol::RoleRuntimeBinding,
    pub conversation_selection: &'a str,
    pub upstream_target: &'a str,
    pub prompt: &'a str,
    pub prompt_context: Option<serde_json::Value>,
    pub client_tools: Option<serde_json::Value>,
    pub runtime_context: Option<&'a str>,
    pub runtime_context_len: usize,
    /// Sanitized adapter-owned inputs used only to recover a technical-budget
    /// continuation after process loss.
    pub technical_budget_recovery_start_payload: Option<serde_json::Value>,
    pub stream_tokens: bool,
    pub api_style: Option<LlmApiStyle>,
    /// Catalog-authoritative support for optional reasoning effort request metadata.
    /// Unknown stays `None` so runtime request construction omits the override.
    pub supports_reasoning_effort: Option<bool>,
}

pub struct TurnContinueRequest<'a> {
    pub sqlx_pool: &'a PgPool,
    pub config: &'a Config,
    pub memory_stores: &'a MemoryStoreManager,
    pub request_id: Uuid,
    pub run_id: Option<&'a str>,
    pub client_session_id: &'a str,
    pub conversation: RuntimeConversationRef,
    pub binding: &'a den_protocol::RoleRuntimeBinding,
    pub continuation: RuntimeContinuation,
    pub stream_context: TurnStreamContext,
}

pub fn default_tool_continue_stream_context() -> TurnStreamContext {
    TurnStreamContext {
        client_tools: None,
        stream_tokens: false,
        max_steps: 4,
        run_recovery: RunRecoveryDisposition::None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunRecoveryDisposition {
    #[default]
    None,
    RetryFinalDelivery,
    ResumeEligible {
        attempts: u8,
    },
    Exhausted {
        attempts: u8,
    },
}

impl RunRecoveryDisposition {
    pub fn for_interrupted_run(attempts: u8) -> Self {
        if attempts == 0 {
            Self::ResumeEligible { attempts }
        } else {
            Self::Exhausted { attempts }
        }
    }

    pub fn emits_recovery_context(self) -> bool {
        matches!(self, Self::ResumeEligible { .. })
    }
}

#[derive(Debug, Clone)]
pub struct TurnStreamContext {
    pub client_tools: Option<serde_json::Value>,
    pub stream_tokens: bool,
    pub max_steps: u32,
    pub run_recovery: RunRecoveryDisposition,
}

pub fn looks_like_runtime_waiting_for_approval_error(err: &DenError) -> bool {
    den_protocol::runtime_error_is_conflict_pending_approval(err)
}

pub struct RuntimeMaterializationResult {
    pub conversation_id: String,
    pub created: bool,
}

/// Reuse the canonical source admitted by the edge, regardless of its external ID.
/// Native assembly, persistence, and tools must not create a second memory identity.
pub async fn materialize_runtime_conversation_if_needed<B: RuntimeConversationBackend>(
    _runtime_conversations: &B,
    request: &TurnStartRequest<'_>,
) -> Result<RuntimeMaterializationResult, DenError> {
    let conversation = get_conversation_for_external_id(
        request.sqlx_pool,
        request.bear_id,
        request.upstream_target,
    )
    .await?
    .ok_or_else(|| {
        DenError::Authorization("native turn has no canonical conversation source".into())
    })?;
    let viewer = ConversationViewer::resolve(
        request.sqlx_pool,
        BearId::new(request.bear_id),
        UserId::new(request.user_id),
    )
    .await?
    .ok_or_else(|| DenError::Authorization("not a member of this bear".to_string()))?;
    if !viewer
        .may_read_own_source(request.sqlx_pool, conversation.id)
        .await?
    {
        return Err(DenError::Authorization(
            "native turn actor does not own its canonical conversation".into(),
        ));
    }
    Ok(RuntimeMaterializationResult {
        conversation_id: request.upstream_target.to_string(),
        created: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_run_recovery_is_one_shot() {
        assert_eq!(
            RunRecoveryDisposition::for_interrupted_run(0),
            RunRecoveryDisposition::ResumeEligible { attempts: 0 }
        );
        assert_eq!(
            RunRecoveryDisposition::for_interrupted_run(1),
            RunRecoveryDisposition::Exhausted { attempts: 1 }
        );
        assert!(RunRecoveryDisposition::for_interrupted_run(0).emits_recovery_context());
        assert!(!RunRecoveryDisposition::for_interrupted_run(1).emits_recovery_context());
    }

    #[test]
    fn default_tool_continuation_has_no_run_recovery_prompt() {
        assert_eq!(
            default_tool_continue_stream_context().run_recovery,
            RunRecoveryDisposition::None
        );
    }
}
