//! Strip Letta / model harness text that should not appear in end-user chat or derived titles.
//!
//! This includes `<system-reminder>…` (and `system_reminder` variants), role-local resource
//! payloads, ACP workflow scaffolding, and plain-text subagent fork notices the primary thread
//! may still receive as message content.

use regex::Regex;
use std::sync::OnceLock;

static SYSTEM_REMINDER_BLOCKS: OnceLock<Regex> = OnceLock::new();
static SUBAGENT_FORK_BLOB: OnceLock<Regex> = OnceLock::new();

/// Human-visible assistant (and title-derived) text: remove reminder markup and subagent fork noise.
pub fn strip_letta_harness_for_user(s: &str) -> String {
    sanitize_visible_transcript_text(s)
}

/// Shared display-transcript sanitizer for Den chat surfaces.
///
/// Prompt assembly may legitimately add runtime scaffolding to model input, but transcript UIs
/// should render only human/assistant-authored content. Apply this before returning or replaying
/// user-visible chat messages across ACP, web chat, and future Workplace chat surfaces.
pub fn sanitize_visible_transcript_text(s: &str) -> String {
    let blocks = SYSTEM_REMINDER_BLOCKS.get_or_init(|| {
        Regex::new(r"(?is)<\s*system[-_]reminder\b[^>]*>.*?</\s*system[-_]reminder\s*>")
            .expect("system[-_]reminder block regex")
    });
    let mut t = blocks.replace_all(s, "").to_string();

    if let Some(start) = find_ascii_case_insensitive(&t, "<system-reminder") {
        let rest = &t[start..];
        if !has_system_reminder_close(rest) {
            t.truncate(start);
        }
    } else if let Some(start) = find_ascii_case_insensitive(&t, "<system_reminder") {
        let rest = &t[start..];
        if !has_system_reminder_close(rest) {
            t.truncate(start);
        }
    }

    let fork = SUBAGENT_FORK_BLOB.get_or_init(|| {
        Regex::new(
            r"(?is)(?:\r?\n|^)\s*You have been forked from the primary conversational thread to run as an independent subagent\..*?provided\s+upfront\.?",
        )
        .expect("subagent fork blob regex")
    });
    t = fork.replace_all(t.trim_end(), "").trim().to_string();

    let t = strip_prompt_scaffolding_prefix(&t);
    strip_hidden_resource_blocks(&t)
}

fn strip_prompt_scaffolding_prefix(s: &str) -> String {
    let trimmed = s.trim_start();
    let is_scaffold = find_ascii_case_insensitive(trimmed, "ACP workflow state for this session:")
        == Some(0)
        || find_ascii_case_insensitive(trimmed, "AUTHORITATIVE WORKFLOW STATE for this turn:")
            == Some(0);
    if !is_scaffold {
        return s.trim().to_string();
    }
    let Some(split_at) = trimmed.find("\n\n").or_else(|| trimmed.find("\r\n\r\n")) else {
        return String::new();
    };
    trimmed[split_at..].trim().to_string()
}

fn strip_hidden_resource_blocks(raw: &str) -> String {
    strip_tagged_block(raw, "<bears-acp-resource", "</bears-acp-resource>")
}

fn strip_tagged_block(raw: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        let Some(start) = find_ascii_case_insensitive(rest, open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        let Some(end) = find_ascii_case_insensitive(after_start, close) else {
            break;
        };
        rest = &after_start[end + close.len()..];
    }
    out.trim().to_string()
}

fn has_system_reminder_close(s: &str) -> bool {
    find_ascii_case_insensitive(s, "</system-reminder>").is_some()
        || find_ascii_case_insensitive(s, "</system_reminder>").is_some()
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let hn = needle.len();
    if hn == 0 || haystack.len() < hn {
        return None;
    }
    let nb = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(hn)
        .position(|w| w.eq_ignore_ascii_case(nb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_subagent_fork_in_system_reminder() {
        let s = "If you want, I can help.\n<system-reminder>\nYou have been forked from the primary conversational thread to run as an independent subagent.\nYou CANNOT ask questions mid-execution - all instructions are provided upfront.\n</system-reminder>";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out.trim(), "If you want, I can help.");
    }

    #[test]
    fn strips_system_underscore_tag() {
        let s = "Hi\n<system_reminder>x</system_reminder>";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out, "Hi");
    }

    #[test]
    fn truncates_unclosed_opener() {
        let s = "Hello <system-reminder>oops";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out, "Hello");
    }

    #[test]
    fn subagent_blob_plaintext_without_tags() {
        let s = "Line one.\nYou have been forked from the primary conversational thread to run as an independent subagent.\nYou CANNOT ask questions mid-execution - all instructions are provided upfront.\n";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out.trim(), "Line one.");
    }

    #[test]
    fn sanitizes_hidden_resource_blocks() {
        let s = "Please read this.\n<bears-acp-resource uri=\"file:///secret\">hidden context</bears-acp-resource>";
        let out = sanitize_visible_transcript_text(s);
        assert_eq!(out, "Please read this.");
    }

    #[test]
    fn strips_acp_prompt_scaffolding_prefix_when_tags_are_lost() {
        let s = "ACP workflow state for this session: workflow_id=123 workflow_state=submitted submitted_plan_present=true approval_status=awaiting_human_approval execution_unlocked=false. Workflow state is authoritative; artifact path is audit context only.\n\nPlease implement the fix.";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out, "Please implement the fix.");
    }

    #[test]
    fn strips_authoritative_workflow_state_prefix_when_tags_are_lost() {
        let s = "AUTHORITATIVE WORKFLOW STATE for this turn: permission_mode=`Plan`; tool_classes=read_only; workplan.state=`submitted`; state_authority=current turn capabilities override prior-turn assumptions.\n\nWhat changed?";
        let out = strip_letta_harness_for_user(s);
        assert_eq!(out, "What changed?");
    }
}
