//! Shared guidance snippets for model-facing tool descriptors.
//!
//! The goal is to keep scope, side-effect, and orientation language consistent across Den tools,
//! ACP-local tools, future pair channels, and agentic skills without moving runtime context into
//! persisted user messages.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolScopeKind {
    AcpClientWorkspace,
    BearRoleMemory,
    BrowserSession,
    Conversation,
    ExternalWeb,
    GitRepository,
    ProcessWorkspace,
    CurrentSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSideEffectKind {
    ReadOnly,
    WritesMemory,
    WritesWorkspace,
    DeletesWorkspace,
    GitMutation,
    ExecutesCode,
    BrowserInteraction,
    ActiveWorkState,
    ConversationMetadata,
    ExternalNetwork,
    SkillGovernance,
}

impl ToolSideEffectKind {
    pub fn is_mutating_or_sensitive(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOrientationPolicy {
    UseSessionInfoIfScopeUnclear,
    UseSessionInfoAndReadBeforeMutation,
    UseSessionInfoAndInspectGitFirst,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolDescriptorGuidance {
    pub scope: ToolScopeKind,
    pub side_effect: ToolSideEffectKind,
    pub orientation: ToolOrientationPolicy,
}

pub fn render_tool_descriptor_guidance(guidance: ToolDescriptorGuidance) -> String {
    let scope = match guidance.scope {
        ToolScopeKind::AcpClientWorkspace => {
            "Scope: local files in the current ACP client workspace roots only."
        }
        ToolScopeKind::BearRoleMemory => {
            "Scope: Bear memory for the current role/Workplace and, when known, current work surface."
        }
        ToolScopeKind::BrowserSession => {
            "Scope: configured local browser/DevTools session for this client."
        }
        ToolScopeKind::Conversation => "Scope: current conversation/thread metadata.",
        ToolScopeKind::ExternalWeb => "Scope: external HTTP(S)/web information, bounded by Den policy and provider configuration.",
        ToolScopeKind::GitRepository => {
            "Scope: git repositories under the current workspace roots."
        }
        ToolScopeKind::ProcessWorkspace => {
            "Scope: commands run in an explicit cwd under the current workspace roots."
        }
        ToolScopeKind::CurrentSession => "Scope: current Bear role session/thread.",
    };

    let side_effect = match guidance.side_effect {
        ToolSideEffectKind::ReadOnly => "Side effect: read-only.",
        ToolSideEffectKind::WritesMemory => {
            "Side effect: writes role-local semantic memory; not for active plans, tasks, observations, run results, Cabinet writes, or direct core updates."
        }
        ToolSideEffectKind::WritesWorkspace => {
            "Side effect: mutates workspace files and requires approval."
        }
        ToolSideEffectKind::DeletesWorkspace => {
            "Side effect: destructive workspace mutation and requires approval."
        }
        ToolSideEffectKind::GitMutation => {
            "Side effect: mutates git/worktree state and requires approval."
        }
        ToolSideEffectKind::ExecutesCode => {
            "Side effect: executes local commands and requires approval."
        }
        ToolSideEffectKind::BrowserInteraction => {
            "Side effect: inspects or manipulates the configured browser session."
        }
        ToolSideEffectKind::ActiveWorkState => {
            "Side effect: updates active work state, not semantic memory."
        }
        ToolSideEffectKind::ConversationMetadata => {
            "Side effect: updates conversation metadata, not Bear memory or conversation content."
        }
        ToolSideEffectKind::ExternalNetwork => {
            "Side effect: may call external network/search providers through Den policy."
        }
        ToolSideEffectKind::SkillGovernance => {
            "Side effect: updates skill proposal/governance state for review, not immediate agent behavior unless explicitly approved and reconciled."
        }
    };

    let orientation = match guidance.orientation {
        ToolOrientationPolicy::UseSessionInfoIfScopeUnclear => {
            "Use session_info first if current Bear, role/Workplace, work surface, workspace, repository, channel, or policy scope is unclear."
        }
        ToolOrientationPolicy::UseSessionInfoAndReadBeforeMutation => {
            "Use session_info first if scope is unclear; inspect/read current state before proposing mutations."
        }
        ToolOrientationPolicy::UseSessionInfoAndInspectGitFirst => {
            "Use session_info first if repository/work-surface scope is unclear; inspect git status/diff before git mutations."
        }
    };

    format!("{scope} {side_effect} {orientation}")
}
