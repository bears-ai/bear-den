use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use den_core::ids::HatId;

use crate::descriptors;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeType {
    ProfileLocal,
    SourceLocal,
    Hat,
    Shared,
}

impl MemoryScopeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileLocal => "profile_local",
            Self::SourceLocal => "source_local",
            Self::Hat => "hat",
            Self::Shared => "shared",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        raw.parse().ok()
    }
}

impl fmt::Display for MemoryScopeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryScopeType {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "profile_local" | "role_local" => Ok(Self::ProfileLocal),
            "source_local" => Ok(Self::SourceLocal),
            "hat" => Ok(Self::Hat),
            "shared" => Ok(Self::Shared),
            _ => Err(()),
        }
    }
}

/// A durable producer of uncurated notes, never a transient client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Conversation(Uuid),
    WorkRun(Uuid),
    Intake(Uuid),
}

impl MemorySource {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Conversation(_) => "conversation",
            Self::WorkRun(_) => "work_run",
            Self::Intake(_) => "intake",
        }
    }

    pub fn id(self) -> Uuid {
        match self {
            Self::Conversation(id) | Self::WorkRun(id) | Self::Intake(id) => id,
        }
    }
}

/// Stable anchor path projection over SQLite rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalMemoryPath {
    pub scope_type: MemoryScopeType,
    pub scope_profile: Option<String>,
    #[serde(default)]
    pub source: Option<MemorySource>,
    #[serde(default)]
    pub hat_id: Option<HatId>,
    pub work_surface_ref: Option<String>,
    pub kind: String,
}

impl LogicalMemoryPath {
    pub fn profile_local(profile: &str, kind: &str) -> Self {
        Self {
            scope_type: MemoryScopeType::ProfileLocal,
            scope_profile: Some(profile.to_string()),
            source: None,
            hat_id: None,
            work_surface_ref: None,
            kind: kind.to_string(),
        }
    }

    pub fn shared_core(kind: &str) -> Self {
        Self {
            scope_type: MemoryScopeType::Shared,
            scope_profile: None,
            source: None,
            hat_id: None,
            work_surface_ref: None,
            kind: kind.to_string(),
        }
    }

    pub fn source_local(source: MemorySource, kind: &str) -> Self {
        Self {
            scope_type: MemoryScopeType::SourceLocal,
            scope_profile: None,
            source: Some(source),
            hat_id: None,
            work_surface_ref: None,
            kind: kind.to_string(),
        }
    }

    pub fn hat(hat_id: HatId, kind: &str) -> Self {
        Self {
            scope_type: MemoryScopeType::Hat,
            scope_profile: None,
            source: None,
            hat_id: Some(hat_id),
            work_surface_ref: None,
            kind: kind.to_string(),
        }
    }

    /// Logical paths are locators, not access grants; canonical scope is stored in SQLite columns.
    pub fn to_logical_path(&self) -> String {
        if let (MemoryScopeType::SourceLocal, Some(source)) = (self.scope_type, self.source) {
            return format!(
                "source_memory/{}/{}/{}.md",
                source.kind(),
                source.id(),
                self.kind
            );
        }
        if let (MemoryScopeType::Hat, Some(hat_id)) = (self.scope_type, self.hat_id) {
            return format!("hat_memory/{hat_id}/{}.md", self.kind);
        }
        match (
            &self.scope_type,
            &self.scope_profile,
            &self.work_surface_ref,
        ) {
            (MemoryScopeType::Shared, None, Some(ws)) => {
                format!("core/work_surfaces/{ws}/{}.md", self.kind)
            }
            (MemoryScopeType::Shared, None, None) if self.kind == "overview" => {
                "core/bear-overview.md".to_string()
            }
            (MemoryScopeType::Shared, None, None) => format!("core/{}.md", self.kind),
            (MemoryScopeType::ProfileLocal, Some(profile), Some(ws)) => {
                format!("{profile}/work_surfaces/{ws}/{}.md", self.kind)
            }
            (MemoryScopeType::ProfileLocal, Some(profile), None) => {
                format!("{profile}/{}.md", self.kind)
            }
            _ => format!("memory/{}.md", self.kind),
        }
    }

    pub fn from_logical_path(path: &str) -> Self {
        let trimmed = path.trim().trim_start_matches('/');
        if let Some(rest) = trimmed.strip_prefix("core/work_surfaces/") {
            let (ws, kind) = parse_work_surface_path(rest);
            return Self {
                scope_type: MemoryScopeType::Shared,
                scope_profile: None,
                source: None,
                hat_id: None,
                work_surface_ref: Some(ws),
                kind,
            };
        }
        if trimmed.starts_with("core/") {
            let kind = trimmed
                .trim_start_matches("core/")
                .trim_end_matches(".md")
                .to_string();
            return Self::shared_core(&kind);
        }
        if let Some((profile, rest)) = trimmed.split_once('/') {
            if let Some(rest) = rest.strip_prefix("work_surfaces/") {
                let (ws, kind) = parse_work_surface_path(rest);
                return Self {
                    scope_type: MemoryScopeType::ProfileLocal,
                    scope_profile: Some(profile.to_string()),
                    source: None,
                    hat_id: None,
                    work_surface_ref: Some(ws),
                    kind,
                };
            }
            let kind = rest.trim_end_matches(".md").to_string();
            return Self::profile_local(profile, &kind);
        }
        Self::shared_core("note")
    }
}

fn parse_work_surface_path(path: &str) -> (String, String) {
    let mut parts = path.split('/');
    let work_surface = parts.next().unwrap_or("unknown").to_string();
    let file = parts.next().unwrap_or("index.md");
    let kind = file.trim_end_matches(".md").to_string();
    (work_surface, kind)
}

fn sanitize_anchor_segment(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() || ch == '/' || ch == ':' {
            out.push('-');
        }
    }
    let collapsed = out
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "unknown".to_string()
    } else {
        collapsed
    }
}

fn entity_anchor_collection(entity_type: &str) -> Option<&'static str> {
    let descriptor = descriptors::entity_type(entity_type)?;
    if !descriptor.anchor_eligible {
        return None;
    }
    match entity_type {
        "person" => Some("people"),
        "org" => Some("orgs"),
        "mission" => Some("missions"),
        "domain" => Some("domains"),
        "work_surface" => Some("work_surfaces"),
        _ => None,
    }
}

/// Stable shared-memory anchor path for resolved, salient entities (ADR-0042 Phase 5).
///
/// This helper only encodes the descriptor-owned path shape. Callers remain responsible for the
/// policy decision that an entity is resolved and salient enough to deserve a canonical anchor.
pub fn entity_anchor_path(entity_type: &str, anchor_ref: &str, kind: &str) -> Option<String> {
    let collection = entity_anchor_collection(entity_type)?;
    let anchor = sanitize_anchor_segment(anchor_ref);
    let kind = sanitize_anchor_segment(kind);
    Some(format!("core/{collection}/{anchor}/{kind}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_work_surface_core_path() {
        let path = "core/work_surfaces/my-repo/architecture.md";
        let logical = LogicalMemoryPath::from_logical_path(path);
        assert_eq!(logical.work_surface_ref.as_deref(), Some("my-repo"));
        assert_eq!(logical.kind, "architecture");
        assert_eq!(logical.to_logical_path(), path);
    }

    #[test]
    fn builds_descriptor_owned_entity_anchor_paths() {
        assert_eq!(
            entity_anchor_path("person", "Ryan Dahl", "profile").as_deref(),
            Some("core/people/ryan-dahl/profile.md")
        );
        assert_eq!(
            entity_anchor_path("mission", "Cabinet:Launch", "overview").as_deref(),
            Some("core/missions/cabinet-launch/overview.md")
        );
        assert!(entity_anchor_path("event", "standup", "overview").is_none());
    }
}
