use den_core::DenError;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::support::truncate_chars;
use den_core::tools::work_surface::{
    work_surface_anchor_paths, work_surface_candidate_slug_from_hints,
    work_surface_projection_status, WorkSurfaceProjectionStatus, WorkSurfaceSessionHints,
};
use den_llm::model_registry;
use den_memory::{
    has_work_surface_canonical_anchor, head_record_for_logical_path,
    list_entity_anchor_head_records, memory_sequence_high_water, record_visible, scoped,
    AccessContext, BearMemoryStore, MemoryRecordRow, MemoryScopeType, MemoryStoreManager,
};
use den_service::bears::{
    managed_blocks::{compile_and_store_managed_config_for_bear, get_compiled_bear_config},
    model::RuntimeContextLabel,
    Bear,
};

const TIER1_SHARED_PATHS: &[&str] = &[
    "core/bear-overview.md",
    "core/bear-glossary.md",
    "core/shared-conventions.md",
];

const TIER4_SITUATION_PATH: &str = "core/situation/briefing.md";
const PROJECTED_MEMORY_TRUNCATION_ELLIPSIS: &str = "...";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMemoryProjectionCacheKey {
    pub bear_id: Uuid,
    pub profile: RuntimeContextLabel,
    pub conversation_id: String,
    pub primary_surface_slug: Option<String>,
    pub sequence_high_water: i64,
    pub compiled_config_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProjectionScope {
    Bound(scoped::MemoryReadGrant),
    SharedOnly,
}

#[derive(Debug, Clone)]
pub struct KeyMemoryProjectionResult {
    pub rendered_text: String,
    pub diagnostic: Value,
    pub cache_key: KeyMemoryProjectionCacheKey,
}

#[derive(Clone)]
pub struct KeyMemoryProjectionInput<'a> {
    pub pool: &'a PgPool,
    pub stores: &'a MemoryStoreManager,
    pub bear: &'a Bear,
    pub profile: RuntimeContextLabel,
    pub conversation_id: &'a str,
    pub session_hints: WorkSurfaceSessionHints,
    pub work_surface_status_override: Option<&'a str>,
    pub native_runtime: bool,
    pub model_for_budget: Option<&'a str>,
    /// Mandatory access gate (ADR-0042 §7): records carrying access-bearing relations are
    /// only projected when this context grants them. An empty context is fail-closed.
    pub access: AccessContext,
}

struct TierBudget {
    max_records: usize,
    per_record_cap: usize,
    tier_soft_cap: usize,
}

struct ProjectionBudget {
    global_cap: usize,
    tiers: [TierBudget; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BudgetTier {
    SharedCore,
    WorkSurface,
    BoundSource,
    Situation,
}

impl BudgetTier {
    const fn index(self) -> usize {
        match self {
            Self::SharedCore => 0,
            Self::WorkSurface => 1,
            Self::BoundSource => 2,
            Self::Situation => 3,
        }
    }
}

fn projection_budget_for_profile_and_model(
    role: RuntimeContextLabel,
    context_window: Option<u32>,
) -> ProjectionBudget {
    let base_global_cap = match role {
        RuntimeContextLabel::ArmatureConversation
        | RuntimeContextLabel::ChannelConversation
        | RuntimeContextLabel::JobRun => 8_000,
        RuntimeContextLabel::Curation => 6_000,
        RuntimeContextLabel::Observation => 4_000,
    };
    let global_cap = match context_window {
        Some(ctx) if ctx >= 1_000_000 => base_global_cap * 2,
        Some(ctx) if ctx >= 200_000 => base_global_cap + (base_global_cap / 2),
        _ => base_global_cap,
    };
    ProjectionBudget {
        global_cap,
        tiers: [
            TierBudget {
                max_records: 4,
                per_record_cap: 1_500,
                tier_soft_cap: 3_000,
            },
            TierBudget {
                max_records: 6,
                per_record_cap: 1_200,
                tier_soft_cap: 3_500,
            },
            TierBudget {
                max_records: 4,
                per_record_cap: 800,
                tier_soft_cap: 2_000,
            },
            TierBudget {
                max_records: 1,
                per_record_cap: 1_000,
                tier_soft_cap: 1_000,
            },
        ],
    }
}

struct BudgetTracker {
    global_remaining: usize,
    tier_remaining_records: usize,
    tier_char_remaining: usize,
    per_record_cap: usize,
}

impl BudgetTracker {
    fn new(budget: &ProjectionBudget, tier: BudgetTier) -> Self {
        let tier_budget = &budget.tiers[tier.index()];
        Self {
            global_remaining: budget.global_cap,
            tier_remaining_records: tier_budget.max_records,
            tier_char_remaining: tier_budget.tier_soft_cap,
            per_record_cap: tier_budget.per_record_cap,
        }
    }

    fn try_take_record(&mut self, content: &str) -> Option<String> {
        if self.tier_remaining_records == 0
            || self.global_remaining == 0
            || self.tier_char_remaining == 0
        {
            return None;
        }
        let cap = self
            .per_record_cap
            .min(self.tier_char_remaining)
            .min(self.global_remaining);
        let (text, truncated) = truncate_chars(content, cap);
        if text.trim().is_empty() {
            return None;
        }
        let used = text.chars().count()
            + if truncated {
                PROJECTED_MEMORY_TRUNCATION_ELLIPSIS.len()
            } else {
                0
            };
        let rendered = if truncated {
            format!("{text}{PROJECTED_MEMORY_TRUNCATION_ELLIPSIS}")
        } else {
            text
        };
        self.tier_remaining_records -= 1;
        self.tier_char_remaining = self.tier_char_remaining.saturating_sub(used);
        self.global_remaining = self.global_remaining.saturating_sub(used);
        Some(rendered)
    }
}

fn format_record_block(record: &MemoryRecordRow, body: &str) -> String {
    let path = record.logical_path.as_deref().unwrap_or("<unmapped>");
    format!("### {path}\n{body}")
}

/// Running record of what each projection tier included or dropped, surfaced in
/// the projection diagnostic.
#[derive(Default)]
struct ProjectionTallies {
    included: Vec<Value>,
    omitted_budget: Vec<String>,
    omitted_no_surface: Vec<String>,
    omitted_by_access: Vec<String>,
}

/// Shared per-record admission used by every tier: enforce the tier budget, apply
/// the mandatory access gate, and try to fit the record's content. Returns the
/// rendered block body when the record is admitted, recording the omission reason
/// otherwise.
///
/// `omission_path` labels the record in the omission tallies; pass `None` for
/// records with no logical path (Tier 3), which are dropped silently rather than
/// listed. `included_entry` is the diagnostic entry pushed only on admission.
async fn admit_record(
    store: &BearMemoryStore,
    access: &AccessContext,
    tracker: &mut BudgetTracker,
    record: &MemoryRecordRow,
    omission_path: Option<String>,
    included_entry: Value,
    tallies: &mut ProjectionTallies,
) -> Result<Option<String>, DenError> {
    if tracker.tier_remaining_records == 0 {
        if let Some(path) = omission_path {
            tallies.omitted_budget.push(path);
        }
        return Ok(None);
    }
    if !record_visible(store, &record.memory_id, access).await? {
        if let Some(path) = omission_path {
            tallies.omitted_by_access.push(path);
        }
        return Ok(None);
    }
    match tracker.try_take_record(&record.content_text) {
        Some(body) => {
            tallies.included.push(included_entry);
            Ok(Some(format_record_block(record, &body)))
        }
        None => {
            if let Some(path) = omission_path {
                tallies.omitted_budget.push(path);
            }
            Ok(None)
        }
    }
}

pub(crate) async fn compiled_prompt_cache_token(
    pool: &PgPool,
    bear: &Bear,
) -> Result<String, DenError> {
    if bear.context_profile.is_none() {
        return Ok(format!("bound:{}:{}", bear.id, bear.provisioning_version));
    }
    if let Some(compiled) = get_compiled_bear_config(pool, bear.id).await? {
        return Ok(compiled.config_hash);
    }
    Ok(compile_and_store_managed_config_for_bear(pool, bear)
        .await?
        .config_hash)
}

pub async fn project_key_memory(
    input: KeyMemoryProjectionInput<'_>,
) -> Result<KeyMemoryProjectionResult, DenError> {
    project_key_memory_with_scope(input, MemoryProjectionScope::SharedOnly).await
}

pub async fn project_key_memory_with_scope(
    input: KeyMemoryProjectionInput<'_>,
    scope: MemoryProjectionScope,
) -> Result<KeyMemoryProjectionResult, DenError> {
    let store = input.stores.store_for_bear(input.bear.id).await?;
    let sequence_high_water = memory_sequence_high_water(&store).await?;
    let compiled_config_token = compiled_prompt_cache_token(input.pool, input.bear).await?;
    let status =
        work_surface_projection_status(&input.session_hints, input.work_surface_status_override);
    let primary_slug = work_surface_candidate_slug_from_hints(&input.session_hints);
    let cache_key = KeyMemoryProjectionCacheKey {
        bear_id: input.bear.id,
        profile: input.profile,
        conversation_id: input.conversation_id.to_string(),
        primary_surface_slug: primary_slug.clone(),
        sequence_high_water,
        compiled_config_token,
    };

    let model_metadata = input
        .model_for_budget
        .or(input.bear.default_model.as_deref())
        .and_then(model_registry::entry_for_handle);
    let budget = projection_budget_for_profile_and_model(
        input.profile,
        model_metadata.map(|entry| entry.context_window),
    );
    let mut tallies = ProjectionTallies::default();
    let mut sections = Vec::<String>::new();

    // Tier 1 — shared identity anchors
    {
        let mut tracker = BudgetTracker::new(&budget, BudgetTier::SharedCore);
        let mut blocks = Vec::new();
        for path in TIER1_SHARED_PATHS {
            // Pre-check the budget so an exhausted tier records the omission without
            // spending a lookup on the record.
            if tracker.tier_remaining_records == 0 {
                tallies.omitted_budget.push((*path).to_string());
                continue;
            }
            let Some(record) = head_record_for_logical_path(&store, path)
                .await?
                .filter(|record| record.scope_type == MemoryScopeType::Shared)
            else {
                continue;
            };
            let entry = json!({
                "tier": 1,
                "memory_id": record.memory_id,
                "logical_path": path,
            });
            if let Some(body) = admit_record(
                &store,
                &input.access,
                &mut tracker,
                &record,
                Some((*path).to_string()),
                entry,
                &mut tallies,
            )
            .await?
            {
                blocks.push(body);
            }
        }
        if !blocks.is_empty() {
            sections.push(format!("## Shared anchors\n\n{}", blocks.join("\n\n")));
        }
    }

    // Tier 2 — work-surface anchors
    let tier2_active = match status {
        WorkSurfaceProjectionStatus::Unresolved | WorkSurfaceProjectionStatus::Ambiguous => false,
        WorkSurfaceProjectionStatus::Candidate => {
            if let Some(ref slug) = primary_slug {
                has_work_surface_canonical_anchor(&store, slug).await?
            } else {
                false
            }
        }
        WorkSurfaceProjectionStatus::Resolved | WorkSurfaceProjectionStatus::Confirmed => {
            primary_slug.is_some()
        }
    };
    if let Some(slug) = primary_slug.as_deref() {
        if matches!(
            status,
            WorkSurfaceProjectionStatus::Unresolved | WorkSurfaceProjectionStatus::Ambiguous
        ) {
            tallies
                .omitted_no_surface
                .push(format!("tier2:status={}", status.as_str()));
        } else if matches!(status, WorkSurfaceProjectionStatus::Candidate) && !tier2_active {
            tallies
                .omitted_no_surface
                .push(format!("tier2:slug={slug}:anchor_required"));
        } else if tier2_active {
            let (canonical_paths, _) = work_surface_anchor_paths(input.profile, slug);
            let mut tracker = BudgetTracker::new(&budget, BudgetTier::WorkSurface);
            let mut blocks = Vec::new();
            for path in canonical_paths {
                // Pre-check the budget so an exhausted tier records the omission
                // without spending a lookup on the record.
                if tracker.tier_remaining_records == 0 {
                    tallies.omitted_budget.push(path);
                    continue;
                }
                let Some(record) = head_record_for_logical_path(&store, &path)
                    .await?
                    .filter(|record| record.scope_type == MemoryScopeType::Shared)
                else {
                    continue;
                };
                let entry = json!({
                    "tier": 2,
                    "memory_id": record.memory_id,
                    "logical_path": path,
                    "work_surface_slug": slug,
                });
                if let Some(body) = admit_record(
                    &store,
                    &input.access,
                    &mut tracker,
                    &record,
                    Some(path),
                    entry,
                    &mut tallies,
                )
                .await?
                {
                    blocks.push(body);
                }
            }
            if !blocks.is_empty() {
                sections.push(format!(
                    "## Work surface: {slug}\n\n{}",
                    blocks.join("\n\n")
                ));
            }
        }
    }

    // Tier 2b — explicit entity anchors (resolved + salient entities only; no relation fallback).
    {
        let mut tracker = BudgetTracker::new(&budget, BudgetTier::WorkSurface);
        let mut blocks = Vec::new();
        let records = list_entity_anchor_head_records(&store, 6).await?;
        for record in records {
            if record.scope_type != MemoryScopeType::Shared {
                continue;
            }
            let path = record
                .logical_path
                .clone()
                .unwrap_or_else(|| "<unmapped>".to_string());
            let entry = json!({
                "tier": "2b",
                "memory_id": record.memory_id,
                "logical_path": record.logical_path,
            });
            if let Some(body) = admit_record(
                &store,
                &input.access,
                &mut tracker,
                &record,
                Some(path),
                entry,
                &mut tallies,
            )
            .await?
            {
                blocks.push(body);
            }
        }
        if !blocks.is_empty() {
            sections.push(format!("## Entity anchors\n\n{}", blocks.join("\n\n")));
        }
    }

    // Tier 3 — verified source-local and selected-hat highlights
    {
        let mut tracker = BudgetTracker::new(&budget, BudgetTier::BoundSource);
        let mut blocks = Vec::new();

        let records = match scope {
            MemoryProjectionScope::Bound(grant) => scoped::search(
                &store,
                grant,
                &input.access,
                "",
                (budget.tiers[2].max_records + 16) as i64,
            )
            .await?
            .into_iter()
            .filter(|record| record.scope_type != MemoryScopeType::Shared)
            .take(budget.tiers[2].max_records)
            .collect(),
            MemoryProjectionScope::SharedOnly => Vec::new(),
        };
        for record in records {
            let entry = json!({
                "tier": 3,
                "memory_id": record.memory_id,
                "logical_path": record.logical_path,
                "work_surface_ref": record.work_surface_ref,
            });
            if let Some(body) = admit_record(
                &store,
                &input.access,
                &mut tracker,
                &record,
                record.logical_path.clone(),
                entry,
                &mut tallies,
            )
            .await?
            {
                blocks.push(body);
            }
        }
        if !blocks.is_empty() {
            sections.push(format!(
                "## Role highlights ({})\n\n{}",
                input.profile.as_str(),
                blocks.join("\n\n")
            ));
        }
    }

    // Tier 4 — situation briefing
    {
        let mut tracker = BudgetTracker::new(&budget, BudgetTier::Situation);
        if let Some(record) = head_record_for_logical_path(&store, TIER4_SITUATION_PATH)
            .await?
            .filter(|record| record.scope_type == MemoryScopeType::Shared)
        {
            let entry = json!({
                "tier": 4,
                "memory_id": record.memory_id,
                "logical_path": TIER4_SITUATION_PATH,
            });
            if let Some(body) = admit_record(
                &store,
                &input.access,
                &mut tracker,
                &record,
                Some(TIER4_SITUATION_PATH.to_string()),
                entry,
                &mut tallies,
            )
            .await?
            {
                sections.push(format!("## Situation\n\n{body}"));
            }
        }
    }

    let rendered_text = if sections.is_empty() {
        String::new()
    } else {
        format!("# Projected memory\n\n{}", sections.join("\n\n"))
    };
    let diagnostic = json!({
        "source": "key_memory_projection",
        "work_surface_status": status.as_str(),
        "primary_work_surface_slug": primary_slug,
        "tier2_active": tier2_active,
        "sequence_high_water": sequence_high_water,
        "included": tallies.included,
        "omitted_by_budget": tallies.omitted_budget,
        "omitted_because_no_surface": tallies.omitted_no_surface,
        "omitted_by_access": tallies.omitted_by_access,
        "global_char_cap": budget.global_cap,
        "model_metadata": model_metadata.map(|entry| json!({
            "key": entry.key,
            "context_window": entry.context_window,
            "max_output_tokens": entry.max_output_tokens,
        })),
    });
    Ok(KeyMemoryProjectionResult {
        rendered_text,
        diagnostic,
        cache_key,
    })
}

pub fn render_key_memory_projection_block(result: &KeyMemoryProjectionResult) -> Option<String> {
    if result.rendered_text.trim().is_empty() {
        None
    } else {
        Some(result.rendered_text.clone())
    }
}
