//! Work-surface tools: orientation/hint inference, slug normalization, scaffold
//! request building, and the scaffold executor.
//!
//! All builders are pure (ADR-0040 keeps the "work surface" name). Only scaffold
//! persistence is a capability, behind [`WorkSurfaceOps`]; the `den` crate
//! re-exports this whole surface at `core::tools::work_surface::*`.

pub mod store;

pub use store::{ScaffoldRequest, WorkSurfaceOps, WorkSurfaceScaffoldOutcome};

use crate::{BearProfile, DenError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::context::DenToolInvocationContext;
use crate::tools::support::{clean_optional, validate_bounded_text};

fn profile_uses_work_surfaces(role: BearProfile) -> bool {
    crate::EffectivePolicy::compile(
        role,
        crate::Governance::Interactive,
        crate::ArmatureAvailability::Absent,
    )
    .capabilities
    .contains(crate::BearCapability::UseWorkSurfaces)
}

pub fn infer_work_surface_hint(context: &DenToolInvocationContext, role: BearProfile) -> Value {
    let mut candidates = Vec::new();
    if let Some(runtime_target) = context.runtime_target.as_deref().and_then(clean_optional) {
        candidates.push(json!({
            "kind": "runtime_target",
            "value": runtime_target,
            "confidence": "medium"
        }));
    }
    if let Some(selection) = context
        .conversation_selection
        .as_deref()
        .and_then(clean_optional)
    {
        candidates.push(json!({
            "kind": "conversation_selection",
            "value": selection,
            "confidence": "low"
        }));
    }
    for root in context
        .workspace_roots
        .iter()
        .filter(|root| !root.trim().is_empty())
    {
        candidates.push(json!({
            "kind": "workspace_root",
            "value": root,
            "confidence": "medium"
        }));
    }
    let active_work_surface_roles = profile_uses_work_surfaces(role);
    let has_candidates = !candidates.is_empty();
    json!({
        "workplace": {
            "profile": role.as_str(),
            "memory_surface": format!("{}/", role.as_str()),
        },
        "work_surface": {
            "mode": if active_work_surface_roles { "active" } else { "reference_only" },
            // ponytail: Session metadata is only evidence, never canonical identity.
            // Upgrade this to resolved/confirmed only through the session-resolution record.
            "status": if has_candidates { "candidate" } else { "unresolved" },
            "confidence": if has_candidates { "medium" } else { "none" },
            "needs_user_confirmation": false,
            "note": if !has_candidates {
                if active_work_surface_roles {
                    "No trusted work-surface hint is available yet from this session. Use workspace roots, runtime target, user references, and memory anchors to resolve what the agent may be acting on."
                } else {
                    "No trusted work-surface references are available yet from this session. Use Bear memory anchors and explicit user references to identify relevant work surfaces."
                }
            } else if active_work_surface_roles {
                "Trusted session metadata provides work-surface reference candidates for what the agent may be acting on. Treat these as hints, not canonical identity, until confirmed by anchors or explicit user intent."
            } else {
                "Trusted session metadata provides work-surface reference candidates that may help the agent answer about relevant Bear work surfaces. Treat these as hints, not canonical identity, until confirmed by anchors or explicit user intent."
            },
            "reference_candidates": candidates,
            "agent_guidance": {
                "may_state_assumption": has_candidates,
                "should_ask_user_when": [
                    "multiple plausible work surfaces",
                    "memory or action depends on work-surface scope",
                    "the user asks to continue prior work but the current surface is unclear"
                ],
                "confirmation_examples": [
                    "Should I treat this as the current work surface?",
                    "Which work surface should this work job use?"
                ]
            },
            "recommended_grounding_order": [
                "current conversation",
                "session_info",
                "current work-surface anchors",
                "role-local memory",
                "core memory",
                "workspace artifacts"
            ]
        }
    })
}

#[derive(Debug, Deserialize)]
pub struct MemoryCreateWorkSurfaceScaffoldArguments {
    pub work_surface_slug: String,
    pub work_surface_name: String,
    pub overview: String,
    #[serde(default)]
    pub glossary: Option<String>,
    #[serde(default)]
    pub current_understanding: Option<String>,
}

pub fn normalize_work_surface_slug(value: &str) -> Result<String, DenError> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err(DenError::ValidationError(
            "work_surface_slug must not be empty".to_string(),
        ));
    }
    let normalized: String = trimmed
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let collapsed = normalized
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        return Err(DenError::ValidationError(
            "work_surface_slug must include at least one letter or digit".to_string(),
        ));
    }
    if collapsed.len() > 80 {
        return Err(DenError::ValidationError(
            "work_surface_slug must be 80 characters or fewer after normalization".to_string(),
        ));
    }
    Ok(collapsed)
}

#[derive(Debug, Clone, Default)]
pub struct WorkSurfaceSessionHints {
    pub runtime_target: Option<String>,
    pub conversation_selection: Option<String>,
    pub workspace_roots: Vec<String>,
}

impl WorkSurfaceSessionHints {
    pub fn from_invocation(context: &DenToolInvocationContext) -> Self {
        Self {
            runtime_target: context.runtime_target.clone(),
            conversation_selection: context.conversation_selection.clone(),
            workspace_roots: context.workspace_roots.clone(),
        }
    }
}

/// A durable, canonical work-surface identity supplied by a trusted armature
/// session. Candidate strings are intentionally not accepted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkSurfaceSessionAnchor {
    pub surface_id: uuid::Uuid,
    pub status: WorkSurfaceProjectionStatus,
}

impl WorkSurfaceSessionAnchor {
    /// Read only the typed anchor written into trusted adapter session context.
    /// Unknown/malformed values remain unanchored rather than becoming hints.
    pub fn from_adapter_environment(value: Option<&Value>) -> Option<Self> {
        let anchor = value?.get("work_surface_anchor")?;
        let surface_id = anchor.get("surface_id")?.as_str()?.parse().ok()?;
        let status = match anchor.get("status")?.as_str()? {
            "resolved" => WorkSurfaceProjectionStatus::Resolved,
            "confirmed" => WorkSurfaceProjectionStatus::Confirmed,
            _ => return None,
        };
        Some(Self { surface_id, status })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkSurfaceProjectionStatus {
    Unresolved,
    Ambiguous,
    Candidate,
    Resolved,
    Confirmed,
}

impl WorkSurfaceProjectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::Ambiguous => "ambiguous",
            Self::Candidate => "candidate",
            Self::Resolved => "resolved",
            Self::Confirmed => "confirmed",
        }
    }

    pub fn tier2_allowed_without_anchor_proof(self) -> bool {
        matches!(self, Self::Resolved | Self::Confirmed)
    }
}

pub fn work_surface_projection_status(
    hints: &WorkSurfaceSessionHints,
    override_status: Option<&str>,
) -> WorkSurfaceProjectionStatus {
    if let Some(raw) = override_status.map(str::trim).filter(|s| !s.is_empty()) {
        return match raw {
            "confirmed" => WorkSurfaceProjectionStatus::Confirmed,
            "resolved" => WorkSurfaceProjectionStatus::Resolved,
            "ambiguous" => WorkSurfaceProjectionStatus::Ambiguous,
            "candidate" => WorkSurfaceProjectionStatus::Candidate,
            _ => WorkSurfaceProjectionStatus::Unresolved,
        };
    }
    if work_surface_candidate_slug_from_hints(hints).is_some() {
        WorkSurfaceProjectionStatus::Candidate
    } else {
        WorkSurfaceProjectionStatus::Unresolved
    }
}

pub fn work_surface_candidate_slug(context: &DenToolInvocationContext) -> Option<String> {
    work_surface_candidate_slug_from_hints(&WorkSurfaceSessionHints::from_invocation(context))
}

/// Resolves a trusted session candidate only when it identifies exactly one
/// assigned managed surface after applying the canonical slug normalization.
/// A non-match and a normalized-name collision are intentionally unbound.
pub fn resolve_assigned_work_surface_slug_from_hints(
    hints: &WorkSurfaceSessionHints,
    assigned_surface_names: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<String> {
    let candidate = work_surface_candidate_slug_from_hints(hints)?;
    let mut matches = assigned_surface_names.into_iter().filter_map(|name| {
        let name = name.as_ref();
        (normalize_work_surface_slug(name).ok().as_deref() == Some(candidate.as_str()))
            .then(|| name.to_string())
    });
    let resolved = matches.next()?;
    matches.next().is_none().then_some(resolved)
}

pub fn work_surface_candidate_slug_from_hints(hints: &WorkSurfaceSessionHints) -> Option<String> {
    let mut raw_candidates = Vec::new();
    if let Some(value) = hints.runtime_target.as_deref().and_then(clean_optional) {
        raw_candidates.push(value);
    }
    if let Some(value) = hints
        .conversation_selection
        .as_deref()
        .and_then(clean_optional)
    {
        raw_candidates.push(value);
    }
    raw_candidates.extend(
        hints
            .workspace_roots
            .iter()
            .filter_map(|value| clean_optional(value)),
    );

    for raw in raw_candidates {
        let lowered = raw.to_ascii_lowercase();
        for segment in raw.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
            let trimmed =
                segment.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if trimmed.is_empty() {
                continue;
            }
            let candidate = normalize_work_surface_slug(trimmed).ok();
            if let Some(candidate) = candidate {
                if candidate.len() >= 3
                    && !matches!(
                        candidate.as_str(),
                        "workspace" | "pair" | "core" | "main" | "src" | "repo"
                    )
                {
                    return Some(candidate);
                }
            }
        }
        if let Some(rest) = lowered.strip_prefix("repo:") {
            if let Ok(candidate) = normalize_work_surface_slug(rest) {
                return Some(candidate);
            }
        }
    }
    None
}

struct WorkSurfacePaths {
    index: String,
    overview: String,
    glossary: String,
    current_understanding: Option<String>,
    registry: String,
}

fn work_surface_scaffold_paths(role: BearProfile, slug: &str) -> WorkSurfacePaths {
    WorkSurfacePaths {
        index: format!("core/work_surfaces/{slug}/index.md"),
        overview: format!("core/work_surfaces/{slug}/overview.md"),
        glossary: format!("core/work_surfaces/{slug}/glossary.md"),
        current_understanding: match role {
            BearProfile::Pair | BearProfile::Work => Some(format!(
                "{}/work_surfaces/{slug}/current-understanding.md",
                role.as_str()
            )),
            _ => None,
        },
        registry: "core/work_surfaces/index.md".to_string(),
    }
}

pub fn work_surface_index_file_body() -> &'static str {
    "# Work Surfaces\n\nThis index lists the Bear's registered Work Surfaces.\n"
}

pub fn work_surface_entry_body(slug: &str, name: &str) -> String {
    format!("- [{name}](./{slug}/index.md)")
}

pub fn work_surface_scaffold_requests(
    role: BearProfile,
    slug: &str,
    name: &str,
    overview: &str,
    glossary: Option<&str>,
    current_understanding: Option<&str>,
) -> Vec<ScaffoldRequest> {
    let paths = work_surface_scaffold_paths(role, slug);
    let glossary_body =
        glossary.unwrap_or("Glossary terms for this work surface will be added here.");
    let understanding_body = current_understanding.unwrap_or(match role {
        BearProfile::Work => {
            "Current work understanding for this work surface will be maintained here."
        }
        _ => "Current pair understanding for this work surface will be maintained here.",
    });
    let mut requests = vec![
        ScaffoldRequest {
            target_path: paths.registry,
            mode: "create_file".to_string(),
            title: Some("Work Surfaces".to_string()),
            body: Some(work_surface_index_file_body().to_string()),
            old_text: None,
            new_text: None,
        },
        ScaffoldRequest {
            target_path: "core/work_surfaces/index.md".to_string(),
            mode: "append_section".to_string(),
            title: Some(name.to_string()),
            body: Some(work_surface_entry_body(slug, name)),
            old_text: None,
            new_text: None,
        },
        ScaffoldRequest {
            target_path: paths.index,
            mode: "create_file".to_string(),
            title: Some(name.to_string()),
            body: Some(format!(
                "# {name}\n\n- Slug: `{slug}`\n- Overview: [overview](./overview.md)\n- Glossary: [glossary](./glossary.md)\n"
            )),
            old_text: None,
            new_text: None,
        },
        ScaffoldRequest {
            target_path: paths.overview,
            mode: "create_file".to_string(),
            title: Some(format!("{name} overview")),
            body: Some(overview.trim().to_string()),
            old_text: None,
            new_text: None,
        },
        ScaffoldRequest {
            target_path: paths.glossary,
            mode: "create_file".to_string(),
            title: Some(format!("{name} glossary")),
            body: Some(glossary_body.trim().to_string()),
            old_text: None,
            new_text: None,
        },
    ];
    if let Some(current_understanding_path) = paths.current_understanding {
        requests.push(ScaffoldRequest {
            target_path: current_understanding_path,
            mode: "create_file".to_string(),
            title: Some(format!("{name} current understanding")),
            body: Some(understanding_body.trim().to_string()),
            old_text: None,
            new_text: None,
        });
    }
    requests
}

pub fn work_surface_anchor_paths(role: BearProfile, slug: &str) -> (Vec<String>, Vec<String>) {
    let canonical = vec![
        format!("core/work_surfaces/{slug}/index.md"),
        format!("core/work_surfaces/{slug}/overview.md"),
        format!("core/work_surfaces/{slug}/glossary.md"),
        format!("core/work_surfaces/{slug}/architecture.md"),
        format!("core/work_surfaces/{slug}/decisions.md"),
        format!("core/work_surfaces/{slug}/conventions.md"),
    ];
    let profile_local = match role {
        BearProfile::Pair | BearProfile::Work => vec![
            format!(
                "{}/work_surfaces/{slug}/current-understanding.md",
                role.as_str()
            ),
            format!("{}/work_surfaces/{slug}/recent-findings.md", role.as_str()),
            format!("{}/work_surfaces/{slug}/open-questions.md", role.as_str()),
        ],
        _ => Vec::new(),
    };
    (canonical, profile_local)
}

pub fn collect_memory_tree_paths(files: &Value, out: &mut Vec<String>) {
    match files {
        Value::String(path) => out.push(path.clone()),
        Value::Array(items) => {
            for item in items {
                collect_memory_tree_paths(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(path) = map.get("path").and_then(|v| v.as_str()) {
                out.push(path.to_string());
            }
            for value in map.values() {
                collect_memory_tree_paths(value, out);
            }
        }
        _ => {}
    }
}

pub async fn orient_work_surface(
    ops: &impl WorkSurfaceOps,
    context: &DenToolInvocationContext,
    role: BearProfile,
) -> Result<Value, DenError> {
    ops.orient(context, role).await
}

pub async fn create_work_surface_scaffold(
    ops: &impl WorkSurfaceOps,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    crate::EffectivePolicy::compile(
        role,
        crate::Governance::Interactive,
        if context.client_session_id.is_some() {
            crate::ArmatureAvailability::Connected
        } else {
            crate::ArmatureAvailability::Absent
        },
    )
    .capabilities
    .require(crate::BearCapability::ManageWorkSurfaces)?;
    let args: MemoryCreateWorkSurfaceScaffoldArguments = serde_json::from_value(arguments)?;
    let work_surface_slug = normalize_work_surface_slug(&args.work_surface_slug)?;
    let work_surface_name =
        validate_bounded_text("work_surface_name", &args.work_surface_name, 1, 200)?;
    let overview = validate_bounded_text("overview", &args.overview, 1, 20_000)?;
    let glossary = args
        .glossary
        .as_deref()
        .map(|value| validate_bounded_text("glossary", value, 1, 20_000))
        .transpose()?;
    let current_understanding = args
        .current_understanding
        .as_deref()
        .map(|value| validate_bounded_text("current_understanding", value, 1, 20_000))
        .transpose()?;
    let requests = work_surface_scaffold_requests(
        role,
        &work_surface_slug,
        &work_surface_name,
        &overview,
        glossary.as_deref(),
        current_understanding.as_deref(),
    );
    let outcome = ops
        .write_scaffold(
            context.bear_id,
            role,
            &work_surface_slug,
            &work_surface_name,
            requests,
        )
        .await?;
    let paths = work_surface_scaffold_paths(role, &work_surface_slug);
    let mut payload = json!({
        "ok": true,
        "bear_id": context.bear_id,
        "work_surface": {
            "slug": work_surface_slug,
            "name": work_surface_name,
            "paths": {
                "registry": paths.registry,
                "index": paths.index,
                "overview": paths.overview,
                "glossary": paths.glossary,
                "current_understanding": paths.current_understanding,
            }
        },
        "updates": outcome.updates,
    });
    if let Some(storage) = outcome.storage {
        if let Some(map) = payload.as_object_mut() {
            map.insert("storage".to_string(), json!(storage));
        }
    }
    Ok(payload)
}

pub fn build_work_surface_orientation_payload(
    role: BearProfile,
    hint_payload: &Value,
    files: &[String],
    candidate_slug: Option<String>,
) -> Value {
    let mut sorted_files = files.to_vec();
    sorted_files.sort();
    sorted_files.dedup();
    let slug = candidate_slug;
    let (canonical_paths, profile_local_paths) = slug
        .as_deref()
        .map(|slug| work_surface_anchor_paths(role, slug))
        .unwrap_or_else(|| (Vec::new(), Vec::new()));
    let existing_canonical = canonical_paths
        .iter()
        .filter(|path| sorted_files.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    let existing_profile_local = profile_local_paths
        .iter()
        .filter(|path| sorted_files.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    let missing_expected_paths = canonical_paths
        .iter()
        .chain(profile_local_paths.iter())
        .filter(|path| !sorted_files.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    let active_work_surface_roles = profile_uses_work_surfaces(role);
    let status = if slug.is_none() {
        "unresolved"
    } else if existing_canonical.is_empty() && existing_profile_local.is_empty() {
        "candidate_without_anchors"
    } else {
        "oriented"
    };
    let mut recommended_read_order = Vec::new();
    recommended_read_order.extend(existing_canonical.iter().cloned());
    recommended_read_order.extend(existing_profile_local.iter().cloned());
    json!({
        "workplace": hint_payload["workplace"].clone(),
        "work_surface": {
            "mode": if active_work_surface_roles { "active" } else { "reference_only" },
            "status": status,
            "slug": slug,
            "confidence": if status == "oriented" { "medium" } else if status == "candidate_without_anchors" { "low" } else { "unknown" },
            "basis": hint_payload["work_surface"]["reference_candidates"].clone(),
            "note": match status {
                "oriented" if active_work_surface_roles => "Trusted hints and existing memory anchors provide a usable work-surface orientation for what the agent may be acting on.",
                "oriented" => "Trusted hints and existing canonical memory anchors provide a usable orientation for answering about this Bear work surface.",
                "candidate_without_anchors" if active_work_surface_roles => "Trusted hints suggest a work surface the agent may be acting on, but no canonical or role-local anchors were found yet.",
                "candidate_without_anchors" => "Trusted hints suggest a relevant Bear work surface, but no canonical anchors were found yet.",
                _ if active_work_surface_roles => "A current work surface the agent may be acting on could not be resolved from trusted hints alone.",
                _ => "A relevant Bear work surface could not be resolved from trusted hints alone.",
            }
        },
        "canonical_paths": existing_canonical,
        "profile_local_paths": existing_profile_local,
        "recommended_read_order": recommended_read_order,
        "missing_expected_paths": missing_expected_paths,
        "notes": if active_work_surface_roles {
            json!([
                "Use canonical work-surface anchors before broader Bear memory search when available.",
                "Use role-local work-surface memory as supporting working memory for what the agent is acting on.",
                "Treat the resolved slug as a working orientation hint unless explicit user intent or stronger anchors disagree."
            ])
        } else {
            json!([
                "Use canonical work-surface anchors before broader Bear memory search when available.",
                "This role is orienting about Bear work surfaces rather than claiming role-local ownership of one.",
                "Treat the resolved slug as a working orientation hint unless explicit user intent or stronger anchors disagree."
            ])
        }
    })
}

#[cfg(test)]
mod tests;
