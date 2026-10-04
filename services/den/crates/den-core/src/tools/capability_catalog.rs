use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{tools::descriptor::DenToolDescriptor, RuntimeContextLabel};

#[derive(Debug, Deserialize)]
pub struct CapabilitySearchArguments {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CapabilityDescribeArguments {
    pub r#ref: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityEntry {
    pub r#ref: String,
    pub kind: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// Stable definition identity. Session-bound providers should expose a separate instance id.
    pub definition_id: String,
    /// Whether this entry describes a durable definition or a connection-bound instance.
    pub descriptor_lifecycle: String,
    /// Connection-scoped identity. Absent for durable definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    pub provider: String,
    pub source: String,
    pub execution_locality: String,
    pub authority: String,
    pub lifetime: String,
    pub surface: String,
    pub availability: String,
    pub applicability: CapabilityApplicability,
    pub risk: String,
    pub good_for: Vec<String>,
    pub not_good_for: Vec<String>,
    pub execution_options: Vec<String>,
    pub code_mode_compatibility: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<CapabilityRelationship>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<CapabilityReplacement>,
    pub tool: Option<CapabilityToolRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityApplicability {
    pub allowed_roles: Vec<String>,
    pub required_scope: String,
    pub policy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityRelationship {
    pub kind: String,
    pub r#ref: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityReplacement {
    pub status: String,
    pub replacement_ref: String,
}

/// A provider connection exposes this capability instance only for its current session.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionCapabilityDescriptor {
    pub instance_id: String,
    pub name: String,
    pub summary: String,
    pub kind: String,
    pub provider: String,
    pub execution_locality: String,
    pub authority: String,
    pub surface: String,
    pub availability: String,
    pub tags: Vec<String>,
}

pub fn session_capability_entries(
    descriptors: &[SessionCapabilityDescriptor],
) -> Vec<CapabilityEntry> {
    descriptors
        .iter()
        .filter(|descriptor| descriptor.availability == "available")
        .cloned()
        .map(session_capability_to_catalog_entry)
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityToolRef {
    pub canonical_name: String,
    pub provider_name: String,
    pub schema_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityRisk {
    ApprovalRequired,
    Mutating,
    ExternalNetwork,
    ReadOnly,
    DependsOnInvokedCapabilities,
}

impl CapabilityRisk {
    fn as_str(self) -> &'static str {
        match self {
            Self::ApprovalRequired => "approval_required",
            Self::Mutating => "mutating",
            Self::ExternalNetwork => "external_network",
            Self::ReadOnly => "read_only",
            Self::DependsOnInvokedCapabilities => "depends_on_invoked_capabilities",
        }
    }
}

impl CapabilityEntry {
    fn searchable_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {} {} {}",
            self.r#ref,
            self.kind,
            self.summary,
            self.tags.join(" "),
            self.provider,
            self.execution_locality,
            self.authority,
            self.lifetime,
            self.surface,
            self.good_for.join(" "),
            self.not_good_for.join(" ")
        )
        .to_ascii_lowercase()
    }
}

pub fn tool_descriptor_to_capability(descriptor: DenToolDescriptor) -> CapabilityEntry {
    let risk = risk_for_tool(&descriptor);
    CapabilityEntry {
        r#ref: format!("tool:{}", descriptor.name),
        kind: "tool".to_string(),
        summary: descriptor.description.to_string(),
        tags: tags_for_tool(&descriptor),
        definition_id: format!("tool-definition:{}", descriptor.name),
        descriptor_lifecycle: "durable_definition".to_string(),
        instance_id: None,
        provider: descriptor.provider.to_string(),
        source: "den_tool_descriptor".to_string(),
        execution_locality: locality_for_tool(&descriptor).to_string(),
        authority: authority_for_tool(&descriptor).to_string(),
        lifetime: lifetime_for_tool(&descriptor).to_string(),
        surface: descriptor.scope.to_string(),
        availability: descriptor.availability.to_string(),
        applicability: CapabilityApplicability {
            allowed_roles: descriptor
                .allowed_roles
                .iter()
                .map(ToString::to_string)
                .collect(),
            required_scope: descriptor.scope.to_string(),
            policy: format!("approval:{}", descriptor.approval_policy),
        },
        risk: risk.as_str().to_string(),
        good_for: good_for_tool(&descriptor),
        not_good_for: not_good_for_tool(&descriptor, risk),
        execution_options: execution_options_for_tool(&descriptor, risk),
        code_mode_compatibility: code_mode_compatibility(&descriptor, risk).to_string(),
        relationships: vec![CapabilityRelationship {
            kind: "schema".to_string(),
            r#ref: format!("tool-schema:{}", descriptor.name),
        }],
        replacement: None,
        tool: Some(CapabilityToolRef {
            canonical_name: descriptor.name.to_string(),
            provider_name: descriptor.provider_name.clone(),
            schema_ref: format!("tool-schema:{}", descriptor.name),
        }),
    }
}

pub fn session_capability_to_catalog_entry(
    descriptor: SessionCapabilityDescriptor,
) -> CapabilityEntry {
    CapabilityEntry {
        r#ref: format!("capability-instance:{}", descriptor.instance_id),
        kind: descriptor.kind,
        summary: descriptor.summary,
        tags: descriptor.tags,
        definition_id: format!("provider-definition:{}", descriptor.name),
        descriptor_lifecycle: "session_instance".to_string(),
        instance_id: Some(descriptor.instance_id),
        provider: descriptor.provider,
        source: "session_provider_descriptor".to_string(),
        execution_locality: descriptor.execution_locality,
        authority: descriptor.authority,
        lifetime: "current provider connection/session".to_string(),
        surface: descriptor.surface.clone(),
        availability: descriptor.availability,
        applicability: CapabilityApplicability {
            allowed_roles: Vec::new(),
            required_scope: descriptor.surface,
            policy: "provider connection and current session policy".to_string(),
        },
        risk: CapabilityRisk::DependsOnInvokedCapabilities
            .as_str()
            .to_string(),
        good_for: vec!["operations exposed by this connected provider".to_string()],
        not_good_for: vec![
            "assuming the instance survives reconnection or applies to another surface".to_string(),
        ],
        execution_options: vec!["direct invocation through the connected provider".to_string()],
        code_mode_compatibility: "requires_explicit_local_provider_mediation".to_string(),
        relationships: vec![CapabilityRelationship {
            kind: "definition".to_string(),
            r#ref: format!("provider-definition:{}", descriptor.name),
        }],
        replacement: None,
        tool: None,
    }
}

pub fn code_mode_capability(role: RuntimeContextLabel) -> CapabilityEntry {
    CapabilityEntry {
        r#ref: "executor:code_mode.den".to_string(),
        kind: "executor".to_string(),
        summary: "Compose allowed Den capabilities with small scripts for loops, batching, filtering, aggregation, joins, retries, or large intermediate state.".to_string(),
        tags: vec![
            "executor.code".to_string(),
            "composition".to_string(),
            "batching".to_string(),
            format!("role.{}", role.as_str()),
        ],
        definition_id: "executor-definition:code_mode.den".to_string(),
        descriptor_lifecycle: "durable_definition".to_string(),
        instance_id: None,
        provider: "den".to_string(),
        source: "curated_catalog_entry".to_string(),
        execution_locality: "den-managed sandbox".to_string(),
        authority: "current Bear/session policy; only explicitly mediated capabilities".to_string(),
        lifetime: "not yet executable; catalog guidance only".to_string(),
        surface: "allowed Den capability calls".to_string(),
        availability: "discoverable; not executable".to_string(),
        applicability: CapabilityApplicability {
            allowed_roles: vec![role.as_str().to_string()],
            required_scope: "allowed Den capability calls".to_string(),
            policy: "mediated capabilities only".to_string(),
        },
        risk: CapabilityRisk::DependsOnInvokedCapabilities
            .as_str()
            .to_string(),
        good_for: vec![
            "more than a few related calls".to_string(),
            "loops or batching".to_string(),
            "parsing/filtering/aggregation/joins/transforms".to_string(),
            "retries or conditional execution".to_string(),
            "large intermediate outputs with compact final results".to_string(),
        ],
        not_good_for: vec![
            "single simple reads or actions".to_string(),
            "risky actions needing individual review".to_string(),
            "direct access to armature-local files, local commands, or adapter/browser session state unless mediated".to_string(),
        ],
        execution_options: vec!["discover now; executable Code Mode lands later".to_string()],
        code_mode_compatibility: "self".to_string(),
        relationships: Vec::new(),
        replacement: None,
        tool: None,
    }
}

pub fn search_capabilities(entries: &[CapabilityEntry], args: CapabilitySearchArguments) -> Value {
    let limit = args.limit.unwrap_or(12).clamp(1, 50);
    let query_terms: Vec<String> = args
        .query
        .unwrap_or_default()
        .split_whitespace()
        .map(|term| term.to_ascii_lowercase())
        .collect();
    let tag_filter = args.tag.map(|tag| tag.to_ascii_lowercase());
    let kind_filter = args.kind.map(|kind| kind.to_ascii_lowercase());

    let mut matches = Vec::new();
    for entry in entries {
        if let Some(kind) = &kind_filter {
            if entry.kind.to_ascii_lowercase() != *kind {
                continue;
            }
        }
        if let Some(tag) = &tag_filter {
            if !entry
                .tags
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(tag))
            {
                continue;
            }
        }
        let haystack = entry.searchable_text();
        if !query_terms.iter().all(|term| haystack.contains(term)) {
            continue;
        }
        matches.push(entry);
    }

    let results: Vec<Value> = matches
        .into_iter()
        .take(limit)
        .map(compact_result)
        .collect();
    json!({
        "results": results,
        "notes": [
            "Search is lexical/tag-based in this first slice; use capability_describe for full details.",
            "Discovery does not grant invocation authority.",
            "Check execution_locality/surface/authority before assuming similarly named capabilities operate on the same state."
        ]
    })
}

pub fn describe_capability(entries: &[CapabilityEntry], reference: &str) -> Option<Value> {
    let entry = entries.iter().find(|entry| {
        entry.r#ref == reference
            || entry.tool.as_ref().is_some_and(|tool| {
                tool.canonical_name == reference || tool.provider_name == reference
            })
    })?;
    Some(json!({
        "capability": entry,
        "notes": [
            "Prefer direct invocation for simple one-off actions.",
            "Prefer Code Mode for loops, batching, transforms, joins, retries, or large intermediate state when an executor is available.",
            "Local/session-bound providers must be mediated explicitly; this Den-managed entry does not imply armature-local access."
        ]
    }))
}

fn compact_result(entry: &CapabilityEntry) -> Value {
    json!({
        "ref": entry.r#ref,
        "kind": entry.kind,
        "summary": entry.summary,
        "tags": entry.tags,
        "definition_id": entry.definition_id,
        "descriptor_lifecycle": entry.descriptor_lifecycle,
        "instance_id": entry.instance_id,
        "provider": entry.provider,
        "source": entry.source,
        "execution_locality": entry.execution_locality,
        "authority": entry.authority,
        "lifetime": entry.lifetime,
        "surface": entry.surface,
        "availability": entry.availability,
        "applicability": entry.applicability,
        "risk": entry.risk,
        "good_for": entry.good_for,
        "not_good_for": entry.not_good_for,
        "code_mode_compatibility": entry.code_mode_compatibility,
        "execution_options": entry.execution_options,
        "relationships": entry.relationships,
        "replacement": entry.replacement,
        "tool": entry.tool,
    })
}

fn risk_for_tool(descriptor: &DenToolDescriptor) -> CapabilityRisk {
    if descriptor.approval_policy != "never" {
        return CapabilityRisk::ApprovalRequired;
    }
    if descriptor
        .permissions
        .iter()
        .any(|permission| permission.ends_with(".write") || permission.contains("write"))
    {
        CapabilityRisk::Mutating
    } else if descriptor.domain == "web" {
        CapabilityRisk::ExternalNetwork
    } else {
        CapabilityRisk::ReadOnly
    }
}

fn tags_for_tool(descriptor: &DenToolDescriptor) -> Vec<String> {
    let mut tags = vec![
        descriptor.domain.to_string(),
        descriptor.scope.to_string(),
        format!("provider.{}", descriptor.provider),
        format!("locality.{}", descriptor.execution_target),
    ];
    tags.extend(descriptor.permissions.iter().map(|item| item.to_string()));
    tags.sort();
    tags.dedup();
    tags
}

fn locality_for_tool(descriptor: &DenToolDescriptor) -> &str {
    match descriptor.execution_target {
        "den" => "den-managed runtime",
        "adapter" => "session-local adapter",
        "client" => "client-provided session",
        _ => descriptor.execution_target,
    }
}

fn authority_for_tool(descriptor: &DenToolDescriptor) -> &str {
    match descriptor.provider {
        "den" => "current Bear/user/session policy",
        "armature" => "user local adapter session",
        "mcp" => "connected MCP server policy",
        _ => "provider policy",
    }
}

fn lifetime_for_tool(descriptor: &DenToolDescriptor) -> &str {
    match descriptor.provider {
        "den" => "durable while Den service and role policy expose it",
        "armature" | "adapter" | "mcp" => "current session/provider connection",
        _ => "provider-defined",
    }
}

fn good_for_tool(descriptor: &DenToolDescriptor) -> Vec<String> {
    vec![format!(
        "{} via {}",
        descriptor.label, descriptor.provider_name
    )]
}

fn not_good_for_tool(descriptor: &DenToolDescriptor, risk: CapabilityRisk) -> Vec<String> {
    let mut notes = vec!["assuming access to a different surface with a similar name".to_string()];
    if risk != CapabilityRisk::ReadOnly {
        notes.push("unreviewed bulk execution; consider direct review or Code Mode only with explicit approval".to_string());
    }
    if descriptor.provider == "den" {
        notes.push(
            "direct armature-local filesystem, process, browser, or MCP server state".to_string(),
        );
    }
    notes
}

fn execution_options_for_tool(
    _descriptor: &DenToolDescriptor,
    risk: CapabilityRisk,
) -> Vec<String> {
    let mut options = vec!["direct invocation".to_string()];
    if matches!(
        risk,
        CapabilityRisk::ReadOnly | CapabilityRisk::ExternalNetwork
    ) {
        options.push("Code Mode when batching/composition is available".to_string());
    }
    options
}

fn code_mode_compatibility(descriptor: &DenToolDescriptor, risk: CapabilityRisk) -> &'static str {
    if descriptor.execution_target != "den" {
        return "requires_explicit_local_provider_mediation";
    }
    if matches!(
        risk,
        CapabilityRisk::ReadOnly | CapabilityRisk::ExternalNetwork
    ) {
        "eligible_when_executor_is_available"
    } else {
        "direct_invocation_or_explicit_approval"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::descriptor::builtin_den_tool_descriptors_for_profile;

    #[test]
    fn capability_risk_preserves_wire_strings() {
        assert_eq!(
            CapabilityRisk::ApprovalRequired.as_str(),
            "approval_required"
        );
        assert_eq!(CapabilityRisk::Mutating.as_str(), "mutating");
        assert_eq!(CapabilityRisk::ExternalNetwork.as_str(), "external_network");
        assert_eq!(CapabilityRisk::ReadOnly.as_str(), "read_only");
        assert_eq!(
            CapabilityRisk::DependsOnInvokedCapabilities.as_str(),
            "depends_on_invoked_capabilities"
        );
    }

    #[test]
    fn search_finds_memory_tool_by_tag_and_query() {
        let mut entries: Vec<_> =
            builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation)
                .into_iter()
                .map(tool_descriptor_to_capability)
                .collect();
        entries.push(code_mode_capability(
            RuntimeContextLabel::ArmatureConversation,
        ));
        let result = search_capabilities(
            &entries,
            CapabilitySearchArguments {
                query: Some("memory search".to_string()),
                tag: Some("memory.search".to_string()),
                kind: Some("tool".to_string()),
                limit: Some(10),
            },
        );
        let refs: Vec<_> = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["ref"].as_str())
            .collect();
        assert!(refs.contains(&"tool:den.memory.search"));
    }

    #[test]
    fn describe_accepts_provider_name() {
        let entries: Vec<_> =
            builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation)
                .into_iter()
                .map(tool_descriptor_to_capability)
                .collect();
        let result = describe_capability(&entries, "memory_search").unwrap();
        assert_eq!(result["capability"]["ref"], "tool:den.memory.search");
        assert_eq!(
            result["capability"]["execution_locality"],
            "den-managed runtime"
        );
    }

    #[test]
    fn code_mode_entry_names_locality_limits() {
        let entry = code_mode_capability(RuntimeContextLabel::ArmatureConversation);
        assert_eq!(entry.r#ref, "executor:code_mode.den");
        assert!(entry
            .not_good_for
            .iter()
            .any(|note| note.contains("armature-local")));
    }

    #[test]
    fn session_instance_preserves_connection_bound_identity_and_locality() {
        let entry = session_capability_to_catalog_entry(SessionCapabilityDescriptor {
            instance_id: "acp-123:mcp:filesystem.read".to_string(),
            name: "filesystem.read".to_string(),
            summary: "Read a file through the connected MCP server.".to_string(),
            kind: "tool".to_string(),
            provider: "mcp".to_string(),
            execution_locality: "armature-local workspace".to_string(),
            authority: "connected MCP server policy".to_string(),
            surface: "workspace:/repo".to_string(),
            availability: "available".to_string(),
            tags: vec!["mcp".to_string(), "filesystem.read".to_string()],
        });

        assert_eq!(entry.descriptor_lifecycle, "session_instance");
        assert_eq!(
            entry.instance_id.as_deref(),
            Some("acp-123:mcp:filesystem.read")
        );
        assert_eq!(entry.lifetime, "current provider connection/session");
        assert_eq!(
            entry.code_mode_compatibility,
            "requires_explicit_local_provider_mediation"
        );
        assert_eq!(entry.relationships[0].kind, "definition");
    }

    #[test]
    fn unavailable_session_instances_are_not_discoverable() {
        let entries = session_capability_entries(&[SessionCapabilityDescriptor {
            instance_id: "disconnected:mcp__filesystem__read".to_string(),
            name: "mcp__filesystem__read".to_string(),
            summary: "Stale provider instance".to_string(),
            kind: "tool".to_string(),
            provider: "mcp".to_string(),
            execution_locality: "connected MCP provider".to_string(),
            authority: "current client connection".to_string(),
            surface: "current client session".to_string(),
            availability: "unavailable".to_string(),
            tags: vec!["session-bound".to_string()],
        }]);

        assert!(entries.is_empty());
    }

    #[test]
    fn tool_projection_exposes_phase_zero_contract() {
        let descriptor =
            builtin_den_tool_descriptors_for_profile(RuntimeContextLabel::ArmatureConversation)
                .into_iter()
                .find(|descriptor| descriptor.name == "den.memory.search")
                .unwrap();
        let entry = tool_descriptor_to_capability(descriptor);

        assert_eq!(entry.definition_id, "tool-definition:den.memory.search");
        assert_eq!(entry.descriptor_lifecycle, "durable_definition");
        assert_eq!(entry.source, "den_tool_descriptor");
        assert_eq!(entry.applicability.required_scope, "bear.memory");
        assert_eq!(
            entry.code_mode_compatibility,
            "eligible_when_executor_is_available"
        );
        assert_eq!(entry.relationships[0].kind, "schema");
    }
}
