//! Pure tool-support helpers: argument validators, content-classification
//! heuristics, scope tables, SSRF URL checks, and the memory-write
//! semantic-confirmation token machinery.
//!
//! Runtime-free (errors are `DenError`). Re-exported at `core::tools::support::*`
//! in the `den` crate so existing `support::*` callers keep resolving.

use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;

use crate::{BearProfile, DenError};

use crate::tools::{
    constants::DEN_MEMORY_WRITE_ENTRY,
    context::DenToolInvocationContext,
    memory::MemoryWriteEntryArguments,
    preflight::{ToolPreflight, ToolSemanticWarning},
};

// Validators that live in sibling modules, re-exported so the `den` shim can
// glob-import the whole `support` surface from one place.
pub use crate::tools::prompt_memory::validate_prompt_memory_scope;
pub use crate::tools::validation::{validate_bounded_text, validate_optional_object};
pub use crate::tools::web::text::truncate_chars;

pub fn validate_memory_write_entry_semantics(
    args: &MemoryWriteEntryArguments,
    _context: &DenToolInvocationContext,
) -> Result<String, DenError> {
    let kind = validate_memory_kind(&args.kind)?;
    if kind == "plan" {
        return Err(DenError::ValidationError(
            "This content appears to be a workplan; use update_task_list for visible session task lists and exit_plan_mode for submitted implementation plans instead of memory_write_entry.".to_string(),
        ));
    }
    let title_lower = args.title.trim().to_ascii_lowercase();
    let body_lower = args.body.trim().to_ascii_lowercase();
    let haystack = format!("{}\n{}", title_lower, body_lower);
    let lines = args.body.lines().map(str::trim).collect::<Vec<_>>();
    let task_like = looks_like_activity_or_task_content(&haystack, &title_lower, &lines);
    let plan_like = looks_like_workplan_content(&haystack, &title_lower, &lines);
    let explicit_plan_title =
        contains_any(&title_lower, &["implementation plan", "execution plan"]);
    if task_like && !plan_like {
        return Err(DenError::ValidationError(
            "This content appears to be task tracking or a task intent; use update_task_list or request_task_list_handoff instead of memory_write_entry.".to_string(),
        ));
    }
    if plan_like && explicit_plan_title {
        return Err(DenError::ValidationError(
            "This content appears to be an active workplan or implementation plan; use enter_plan_mode/exit_plan_mode for approval plans or update_task_list for visible session task-list tracking instead of memory_write_entry.".to_string(),
        ));
    }
    if let Some(domain) = args.domain.as_deref() {
        match domain {
            "memory" => {}
            "workplan" => return Err(DenError::ValidationError(
                "This content appears to be a workplan artifact; use plan-mode tools instead of memory_write_entry.".to_string(),
            )),
            "activity" => return Err(DenError::ValidationError(
                "This content appears to be live activity state; use update_task_list or related task-list tools instead of memory_write_entry.".to_string(),
            )),
            "execution" => {
                let title = args.title.trim().to_ascii_lowercase();
                let body = args.body.trim().to_ascii_lowercase();
                if title.contains("observation") || body.contains("alert") || body.contains("telemetry") || body.contains("incident") || body.contains("detected") || body.contains("observed") {
                    return Err(DenError::ValidationError(
                        "This content appears to be an observation; use the observation tool instead of memory_write_entry.".to_string(),
                    ));
                }
                return Err(DenError::ValidationError(
                    "This content appears to describe execution or run output rather than semantic memory; use the appropriate execution/result tool instead of memory_write_entry.".to_string(),
                ));
            }
            _ => {}
        }
    }
    if let Some(content_class) = args.content_class.as_deref() {
        match content_class {
            "semantic_memory" => {}
            "workplan_artifact" => return Err(DenError::ValidationError(
                "This content appears to be a workplan artifact; use plan-mode tools instead of memory_write_entry.".to_string(),
            )),
            "activity_status" => return Err(DenError::ValidationError(
                "This content appears to be live activity state; use update_task_list instead of memory_write_entry.".to_string(),
            )),
            "task_intent" => return Err(DenError::ValidationError(
                "This content appears to be a task intent; use request_task_list_handoff or task-intent tools instead of memory_write_entry.".to_string(),
            )),
            "run_result" => return Err(DenError::ValidationError(
                "This content appears to be a run result; use the run-result tool instead of memory_write_entry.".to_string(),
            )),
            "observation" => return Err(DenError::ValidationError(
                "This content appears to be an observation; use the observation tool instead of memory_write_entry.".to_string(),
            )),
            "core_update" => return Err(DenError::ValidationError(
                "This content appears to be a core update; use memory review or core-update tools instead of memory_write_entry.".to_string(),
            )),
            "cabinet_write" => return Err(DenError::ValidationError(
                "This content appears to be a Cabinet write; use the appropriate Cabinet or reviewed update path instead of memory_write_entry.".to_string(),
            )),
            _ => {}
        }
    }
    if looks_like_activity_or_task_content(&haystack, &title_lower, &lines) {
        return Err(DenError::ValidationError(
            "This content appears to be task tracking or a task intent; use update_task_list or request_task_list_handoff instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_run_result_content(&haystack) {
        return Err(DenError::ValidationError(
            "This content appears to be a run result or command output; use the appropriate execution/result tool instead of memory_write_entry.".to_string(),
        ));
    }
    if title_lower.contains("observation")
        || body_lower.contains("alert")
        || body_lower.contains("telemetry")
        || body_lower.contains("incident")
        || body_lower.contains("detected")
        || body_lower.contains("observed")
    {
        return Err(DenError::ValidationError(
            "This content appears to be an observation; use the observation tool instead of memory_write_entry.".to_string(),
        ));
    }
    Ok(kind)
}

pub fn assess_unlabeled_memory_misuse(
    args: &MemoryWriteEntryArguments,
    context: &DenToolInvocationContext,
) -> Result<ToolPreflight, DenError> {
    let title = args.title.trim();
    let body = args.body.trim();
    let haystack = format!("{}\n{}", title, body).to_ascii_lowercase();
    let title_lower = title.to_ascii_lowercase();
    let lines = body.lines().map(str::trim).collect::<Vec<_>>();
    let explicit_plan_title =
        contains_any(&title_lower, &["implementation plan", "execution plan"]);
    if looks_like_workplan_content(&haystack, &title_lower, &lines) {
        return if explicit_plan_title {
            Err(DenError::ValidationError(
                "This content appears to be an active workplan or implementation plan; use enter_plan_mode/exit_plan_mode for approval plans or update_task_list for visible session task-list tracking instead of memory_write_entry.".to_string(),
            ))
        } else if confirm_suspicious_memory_write(args, context, "plan_like_memory")? {
            Ok(ToolPreflight::Proceed)
        } else {
            Ok(ToolPreflight::Warning(memory_semantic_warning(
                args,
                context,
                "plan_like_memory",
                "This entry resembles a planning artifact. If you intend durable role-local memory rather than a live plan, retry with the provided semantic_confirmation_token.",
            )))
        };
    }
    if looks_like_activity_or_task_content(&haystack, &title_lower, &lines) {
        return Err(DenError::ValidationError(
            "This content appears to be task tracking or a task intent; use update_task_list or request_task_list_handoff instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_run_result_content(&haystack) {
        return Err(DenError::ValidationError(
            "This content appears to be a run result or command output; use the appropriate execution/result tool instead of memory_write_entry.".to_string(),
        ));
    }
    if looks_like_observation_content(&haystack) {
        return Err(DenError::ValidationError(
            "This content appears to be an operational observation; use the observation tool instead of memory_write_entry.".to_string(),
        ));
    }
    Ok(ToolPreflight::Proceed)
}

pub fn clean_limited_strings(
    values: Vec<String>,
    max_items: usize,
    max_chars: usize,
) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(max_chars).collect::<String>())
        .take(max_items)
        .collect()
}

pub fn memory_read_scopes(role: BearProfile) -> Vec<&'static str> {
    match role {
        BearProfile::Pair => vec!["pair/", "core/"],
        BearProfile::Chat => vec!["chat/", "core/"],
        BearProfile::Curate => vec!["chat/", "pair/", "curate/", "work/", "watch/", "core/"],
        BearProfile::Work => vec!["work/", "core/"],
        BearProfile::Watch => vec!["watch/", "core/"],
    }
}

pub fn memory_write_scopes(role: BearProfile) -> Vec<&'static str> {
    match role {
        BearProfile::Pair => vec![
            "pair/notes/",
            "pair/logs/",
            "pair/decisions/",
            "pair/reflections/",
            "pair/scratch/",
            "pair/summaries/",
        ],
        BearProfile::Chat => vec![
            "chat/notes/",
            "chat/logs/",
            "chat/decisions/",
            "chat/reflections/",
            "chat/scratch/",
            "chat/summaries/",
        ],
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedPublicHttpTarget {
    pub url: url::Url,
    pub resolved_addrs: Vec<SocketAddr>,
}

#[cfg(test)]
#[path = "support/tests.rs"]
mod tests;

/// Resolve once and retain the vetted addresses for the HTTP client. Checking
/// DNS and then resolving again at connection time permits DNS rebinding.
pub fn resolve_public_http_target(raw: &str) -> Result<ValidatedPublicHttpTarget, DenError> {
    let url = url::Url::parse(raw.trim())
        .map_err(|e| DenError::ValidationError(format!("url must be a valid HTTP(S) URL: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(DenError::ValidationError(
                "url scheme must be http or https".to_string(),
            ));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| DenError::ValidationError("url must include a host".to_string()))?;
    let lower_host = host.trim_end_matches('.').to_ascii_lowercase();
    if lower_host == "localhost" || lower_host.ends_with(".localhost") {
        return Err(DenError::ValidationError(
            "localhost URLs are not allowed for den.web.fetch".to_string(),
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| DenError::ValidationError("URL has no supported HTTP(S) port".into()))?;
    let ip_host = lower_host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = if let Ok(ip) = ip_host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        (host, port)
            .to_socket_addrs()
            .map_err(|e| {
                DenError::ValidationError(format!("url host could not be resolved safely: {e}"))
            })?
            .collect()
    };
    if addrs.is_empty() {
        return Err(DenError::ValidationError(
            "URL host has no resolved addresses".into(),
        ));
    }
    for addr in &addrs {
        if !is_public_ip(addr.ip()) {
            return Err(DenError::ValidationError(format!(
                "url host resolves to a non-public address: {}",
                addr.ip()
            )));
        }
    }
    Ok(ValidatedPublicHttpTarget {
        url,
        resolved_addrs: addrs,
    })
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.segments()[0] & 0xfe00 == 0xfc00
                || ip.segments()[0] & 0xffc0 == 0xfe80)
        }
    }
}

pub fn clean_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn format_rfc3339(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

fn validate_memory_kind(value: &str) -> Result<String, DenError> {
    let kind = value.trim().to_ascii_lowercase();
    match kind.as_str() {
        "note" | "log" | "decision" | "reflection" | "scratch" | "summary" | "plan" => Ok(kind),
        _ => Err(DenError::ValidationError(
            "kind must be one of note, log, decision, reflection, scratch, summary, plan"
                .to_string(),
        )),
    }
}

fn looks_like_workplan_content(haystack: &str, title: &str, lines: &[&str]) -> bool {
    let explicit_plan_title = contains_any(title, &["implementation plan", "execution plan"]);
    if explicit_plan_title {
        return true;
    }
    let suspicious_title = contains_any(
        title,
        &[
            "approval plan",
            "proposed plan",
            "next steps",
            "plan concepts",
        ],
    );
    let plan_terms = contains_any(
        haystack,
        &[
            "implementation plan",
            "execution plan",
            "workplan",
            "work plan",
            "plan of record",
            "approval plan",
            "proposed plan",
        ],
    );
    let approval_or_execution_cues = contains_any(
        haystack,
        &[
            "submit this plan",
            "once approved",
            "awaiting approval",
            "we will",
            "i will",
            "next i will",
            "first i will",
            "then i will",
        ],
    );
    let structured_action_list = checkbox_or_numbered_item_count(lines) >= 3
        && contains_any(
            haystack,
            &[
                "inspect",
                "edit",
                "implement",
                "fix",
                "run",
                "validate",
                "update",
                "create",
            ],
        );
    let expository = contains_any(
        haystack,
        &[
            "summary",
            "concept",
            "architecture",
            "orientation",
            "difference between",
            "distinguishes",
            "the docs describe",
            "means",
            "refers to",
        ],
    );
    suspicious_title
        || (!expository && plan_terms && (approval_or_execution_cues || structured_action_list))
}

fn looks_like_activity_or_task_content(haystack: &str, title: &str, lines: &[&str]) -> bool {
    let suspicious_title = contains_any(title, &["current tasks", "task list", "next steps"]);
    let explicit_task_language = contains_any(
        haystack,
        &[
            "todo:",
            "to-do:",
            "task list",
            "tasks:",
            "current task",
            "current item",
            "in progress",
            "blocked:",
            "next steps:",
            "handoff request",
            "task intent",
            "request work handoff",
        ],
    );
    let structured_action_list = checkbox_or_numbered_item_count(lines) >= 3
        && contains_any(
            haystack,
            &[
                "inspect",
                "edit",
                "implement",
                "fix",
                "run",
                "update",
                "create",
                "validate",
                "complete",
                "blocked",
                "pending",
            ],
        );
    suspicious_title || explicit_task_language || structured_action_list
}

fn looks_like_run_result_content(haystack: &str) -> bool {
    contains_any(
        haystack,
        &[
            "command output",
            "run result",
            "test result",
            "test results",
            "cargo test",
            "cargo check",
            "npm test",
            "pytest",
            "exit code",
            "exit status",
            "stdout",
            "stderr",
            "stack trace",
            "failed tests",
            "test failed",
            "tests passed",
        ],
    )
}

fn looks_like_observation_content(haystack: &str) -> bool {
    contains_any(
        haystack,
        &[
            "observation",
            "observation:",
            "observed:",
            "i observed",
            "watch observed",
            "monitoring observed",
            "detected",
            "detected:",
            "incident",
            "incident:",
            "alert",
            "alert:",
            "telemetry",
            "metric spike",
        ],
    )
}

fn checkbox_or_numbered_item_count(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("- [ ]")
                || line.starts_with("- [x]")
                || line.starts_with("* [ ]")
                || line.starts_with("* [x]")
                || line.starts_with("- todo")
                || line.starts_with("- task")
                || line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && line.contains(". ")
        })
        .count()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn memory_semantic_warning(
    args: &MemoryWriteEntryArguments,
    context: &DenToolInvocationContext,
    category: &'static str,
    message: &str,
) -> ToolSemanticWarning {
    ToolSemanticWarning {
        code: "semantic_confirmation_required",
        category,
        message: message.to_string(),
        confirmation_token: ConfirmationToken::issue(args, context, category),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmationToken {
    category: String,
    expires_at_secs: u64,
    payload_hash: String,
}

impl ConfirmationToken {
    const INVALID_MESSAGE: &'static str =
        "semantic_confirmation_token is invalid; retry the warning flow to get a fresh token";
    const TTL_SECS: u64 = 15 * 60;

    fn issue(
        args: &MemoryWriteEntryArguments,
        context: &DenToolInvocationContext,
        category: &str,
    ) -> String {
        let expires_at_secs = current_unix_secs().saturating_add(Self::TTL_SECS);
        let payload_hash =
            memory_confirmation_payload_hash(args, context, category, expires_at_secs);
        Self {
            category: category.to_string(),
            expires_at_secs,
            payload_hash,
        }
        .encode()
    }

    fn decode(raw: &str) -> Result<Self, DenError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(raw.trim())
            .map_err(|_| invalid_confirmation_token())?;
        let decoded = String::from_utf8(decoded).map_err(|_| invalid_confirmation_token())?;
        let mut parts = decoded.splitn(3, ':');
        let category = parts.next().unwrap_or_default();
        let exp_raw = parts.next().unwrap_or_default();
        let payload_hash = parts.next().unwrap_or_default();
        let expires_at_secs = exp_raw
            .parse::<u64>()
            .map_err(|_| invalid_confirmation_token())?;
        Ok(Self {
            category: category.to_string(),
            expires_at_secs,
            payload_hash: payload_hash.to_string(),
        })
    }

    fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(format!(
            "{}:{}:{}",
            self.category, self.expires_at_secs, self.payload_hash
        ))
    }

    fn verifies(
        &self,
        args: &MemoryWriteEntryArguments,
        context: &DenToolInvocationContext,
        category: &str,
    ) -> bool {
        self.category == category
            && self.expires_at_secs >= current_unix_secs()
            && self.payload_hash
                == memory_confirmation_payload_hash(args, context, category, self.expires_at_secs)
    }
}

fn invalid_confirmation_token() -> DenError {
    DenError::ValidationError(ConfirmationToken::INVALID_MESSAGE.to_string())
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn confirm_suspicious_memory_write(
    args: &MemoryWriteEntryArguments,
    context: &DenToolInvocationContext,
    category: &str,
) -> Result<bool, DenError> {
    let Some(token) = args.semantic_confirmation_token.as_deref() else {
        return Ok(false);
    };
    Ok(ConfirmationToken::decode(token)?.verifies(args, context, category))
}

fn memory_confirmation_payload_hash(
    args: &MemoryWriteEntryArguments,
    context: &DenToolInvocationContext,
    category: &str,
    exp: u64,
) -> String {
    let confirmation_context = json!({
        "tool": DEN_MEMORY_WRITE_ENTRY,
        "category": category,
        "bear_id": context.bear_id,
        "binding_id": context.binding_id,
        "profile": context.profile.map(|role| role.as_str()),
        "conversation_id": context.conversation_id,
        "session_id": context.session_id,
        "client_session_id": context.client_session_id,
        "request_id": context.request_id,
        "kind": args.kind,
        "title": args.title,
        "body": args.body,
        "tags": args.tags,
        "refs": args.refs,
        "lifecycle": args.lifecycle,
        "content_class": args.content_class,
        "domain": args.domain,
        "exp": exp,
    });
    let digest = Sha256::digest(confirmation_context.to_string().as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}
