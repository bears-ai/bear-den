//! Prompt-memory domain types (runtime prompt blocks, distinct from semantic memory).
//!
//! Shared by the `prompt_memory` tool executors here and the Postgres-backed
//! store + prompt-assembly code in the `den` crate.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMemoryBlockType {
    #[serde(rename = "profile_guidance", alias = "role_guidance")]
    RoleGuidance,
    WorkSurfaceContext,
    SessionFocus,
    UserInstruction,
}

impl PromptMemoryBlockType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoleGuidance => "profile_guidance",
            Self::WorkSurfaceContext => "work_surface_context",
            Self::SessionFocus => "session_focus",
            Self::UserInstruction => "user_instruction",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMemoryBlockScope {
    BearWide,
    #[serde(rename = "profile_local", alias = "role_local")]
    RoleLocal,
    WorkSurface,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMemoryBlockState {
    Draft,
    Active,
    Superseded,
    Archived,
}

/// A stored prompt-memory block as projected for listing/compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMemoryBlock {
    pub id: String,
    pub block_type: PromptMemoryBlockType,
    pub scope: PromptMemoryBlockScope,
    pub state: PromptMemoryBlockState,
    pub role: Option<String>,
    pub work_surface: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub priority: i32,
}

impl PromptMemoryBlock {
    /// Bound runs do not inherit profile- or surface-wide standing context.
    pub fn visible_in_bound_session(&self, session_id: &str) -> bool {
        match self.scope {
            PromptMemoryBlockScope::BearWide => true,
            PromptMemoryBlockScope::Session => self.session_id.as_deref() == Some(session_id),
            PromptMemoryBlockScope::RoleLocal | PromptMemoryBlockScope::WorkSurface => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMemoryVisibility {
    Legacy,
    BoundSession,
    SharedOnly,
}

impl PromptMemoryVisibility {
    pub fn allows(self, block: &PromptMemoryBlock, session_id: &str) -> bool {
        match self {
            Self::Legacy => true,
            Self::BoundSession => block.visible_in_bound_session(session_id),
            Self::SharedOnly => block.scope == PromptMemoryBlockScope::BearWide,
        }
    }
}

/// Full write (insert/upsert) of a prompt-memory block.
#[derive(Debug, Clone)]
pub struct PromptMemoryBlockWrite {
    pub block_id: String,
    pub bear_id: Option<Uuid>,
    pub profile_slug: Option<String>,
    pub scope: PromptMemoryBlockScope,
    pub block_type: PromptMemoryBlockType,
    pub state: PromptMemoryBlockState,
    pub work_surface: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub priority: i32,
    pub created_by_user_id: Option<i32>,
    pub supersedes_block_id: Option<String>,
    pub metadata: Value,
}

/// In-place patch of an existing prompt-memory block.
#[derive(Debug, Clone)]
pub struct PromptMemoryBlockPatch {
    pub state: PromptMemoryBlockState,
    pub title: String,
    pub body: String,
    pub priority: i32,
    pub supersedes_block_id: Option<String>,
    pub metadata: Value,
}

#[cfg(test)]
mod tests {
    use super::{
        PromptMemoryBlock, PromptMemoryBlockScope, PromptMemoryBlockState, PromptMemoryBlockType,
        PromptMemoryVisibility,
    };

    #[test]
    fn prompt_memory_block_type_preserves_wire_strings() {
        assert_eq!(
            PromptMemoryBlockType::RoleGuidance.as_str(),
            "profile_guidance"
        );
        assert_eq!(
            PromptMemoryBlockType::WorkSurfaceContext.as_str(),
            "work_surface_context"
        );
        assert_eq!(
            PromptMemoryBlockType::SessionFocus.as_str(),
            "session_focus"
        );
        assert_eq!(
            PromptMemoryBlockType::UserInstruction.as_str(),
            "user_instruction"
        );
    }

    #[test]
    fn bound_session_does_not_inherit_profile_or_surface_prompt_blocks() {
        let mut block = PromptMemoryBlock {
            id: "b1".into(),
            block_type: PromptMemoryBlockType::UserInstruction,
            scope: PromptMemoryBlockScope::BearWide,
            state: PromptMemoryBlockState::Active,
            role: Some("pair".into()),
            work_surface: None,
            session_id: None,
            title: "shared".into(),
            body: "shared guidance".into(),
            priority: 1,
        };
        assert!(block.visible_in_bound_session("session-a"));
        block.scope = PromptMemoryBlockScope::RoleLocal;
        assert_eq!(serde_json::to_value(block.scope).unwrap(), "profile_local");
        assert_eq!(
            serde_json::from_str::<PromptMemoryBlockScope>("\"role_local\"").unwrap(),
            block.scope
        );
        assert!(!block.visible_in_bound_session("session-a"));
        block.scope = PromptMemoryBlockScope::WorkSurface;
        assert!(!block.visible_in_bound_session("session-a"));
        block.scope = PromptMemoryBlockScope::Session;
        assert!(!block.visible_in_bound_session("session-a"));
        block.session_id = Some("session-a".into());
        assert!(block.visible_in_bound_session("session-a"));
        assert!(!block.visible_in_bound_session("session-b"));
        assert!(PromptMemoryVisibility::BoundSession.allows(&block, "session-a"));
        assert!(!PromptMemoryVisibility::SharedOnly.allows(&block, "session-a"));
        block.scope = PromptMemoryBlockScope::BearWide;
        assert!(PromptMemoryVisibility::SharedOnly.allows(&block, "session-b"));
    }
}
