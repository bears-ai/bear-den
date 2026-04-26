//! Strip Letta / model harness text that should not appear in end-user chat or derived titles.
//!
//! This includes `<system-reminder>…` (and `system_reminder` variants) and plain-text subagent
//! fork notices the primary thread may still receive as `assistant_message` content.

use regex::Regex;
use std::sync::OnceLock;

static SYSTEM_REMINDER_BLOCKS: OnceLock<Regex> = OnceLock::new();
static SUBAGENT_FORK_BLOB: OnceLock<Regex> = OnceLock::new();

/// Human-visible assistant (and title-derived) text: remove reminder markup and subagent fork noise.
pub fn strip_letta_harness_for_user(s: &str) -> String {
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

    t
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
}
