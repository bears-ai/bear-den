use den_core::config::Config;
use den_core::DenError;
use den_docket::{
    task_list_projection_from_session_tasks, DocketService, DocketTaskListFilter, PgDocketService,
    TaskListProjection,
};
use den_memory::MemoryStoreManager;
use den_service::{
    bears::{db as bears_db, model::BearProfile, provision::profile_prompt_text, Bear},
    client_sessions,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::llm::ChatMessage;
use bearwire_protocol::wire::BearWireEvent;
use den_core::tools::work_surface::WorkSurfaceSessionHints;

use super::{
    context::{
        load_transcript_messages, load_transcript_messages_after_seq,
        prune_messages_for_native_conversation_with_diagnostics, repair_tool_call_message_chain,
    },
    key_memory_projection::{
        project_key_memory, render_key_memory_projection_block, KeyMemoryProjectionCacheKey,
        KeyMemoryProjectionInput, KeyMemoryProjectionResult,
    },
    runtime_context::{
        assemble_den_owned_runtime_supplement, render_capability_discovery_guidance,
        runtime_context_already_includes_den_owned_blocks,
    },
    FreeformPolicy, ObjectiveOrientation, ObjectiveOrientationResolutionInput, OrientationTaskRef,
};
use crate::context_budget::AssembledTurnBudgetComponents;
use crate::runtime::compaction::{
    on_turn_assemble_compaction, render_compaction_prompt_context, CompactionMode,
};
use crate::runtime::task_context::orientation_task_ref_from_item;

#[derive(Debug, Clone)]
pub struct AssembleTurnContext<'a> {
    pub pool: &'a PgPool,
    pub config: &'a Config,
    pub stores: &'a MemoryStoreManager,
    pub bear_id: Uuid,
    pub profile: BearProfile,
    pub conversation_id: &'a str,
    pub turn_runtime_context: Option<&'a str>,
    pub human_message: Option<&'a str>,
    pub tool_messages: &'a [ChatMessage],
    pub session_id: Option<&'a str>,
    pub workspace_roots: Option<&'a [String]>,
    pub runtime_target: Option<&'a str>,
    pub conversation_selection: Option<&'a str>,
    pub user_id: Option<i32>,
    pub client_context: Option<&'a serde_json::Value>,
    pub include_prompt_memory: bool,
    pub key_memory_cache: Option<&'a KeyMemoryProjectionCacheKey>,
    pub native_runtime: bool,
}

impl AssembleTurnContext<'_> {
    pub fn should_load_den_owned_runtime_context(&self) -> bool {
        self.include_prompt_memory
            && self.session_id.is_some()
            && !self
                .turn_runtime_context
                .map(runtime_context_already_includes_den_owned_blocks)
                .unwrap_or(false)
    }

    fn session_hints(&self) -> WorkSurfaceSessionHints {
        WorkSurfaceSessionHints {
            runtime_target: self.runtime_target.map(str::to_string),
            conversation_selection: self.conversation_selection.map(str::to_string),
            workspace_roots: self
                .workspace_roots
                .map(|items| items.to_vec())
                .unwrap_or_default(),
        }
    }

    fn work_surface_status_override(&self) -> Option<&str> {
        self.client_context
            .and_then(|ctx| ctx.pointer("/work_surface/status"))
            .and_then(Value::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct AssembledNativeTurn {
    pub messages: Vec<ChatMessage>,
    pub key_memory_projection: Option<KeyMemoryProjectionResult>,
    /// Diagnostic for the derived-recall section (ADR-0038 Phase 2); `None` when recall is
    /// disabled, skipped (e.g. empty query), or failed best-effort.
    pub recall_diagnostic: Option<Value>,
    pub budget_components: AssembledTurnBudgetComponents,
    pub cached_activity_plan_projection: Option<TaskListProjection>,
    pub objective_orientation: ObjectiveOrientation,
}

async fn load_cached_activity_plan_projection(
    ctx: &AssembleTurnContext<'_>,
    service: &PgDocketService,
) -> Result<Option<TaskListProjection>, DenError> {
    load_session_anchored_activity_plan(ctx, service).await
}

async fn load_session_anchored_activity_plan(
    ctx: &AssembleTurnContext<'_>,
    service: &PgDocketService,
) -> Result<Option<TaskListProjection>, DenError> {
    let (Some(user_id), Some(client_session_id)) = (ctx.user_id, ctx.session_id) else {
        return Ok(None);
    };
    let Some(session) = client_sessions::find_for_user_bear_session_id(
        ctx.pool,
        user_id,
        ctx.bear_id,
        client_session_id,
    )
    .await?
    else {
        return Ok(None);
    };
    let session_anchor_id = session.id;
    let tasks = service
        .list_tasks(
            ctx.bear_id,
            DocketTaskListFilter {
                session_anchor_id: Some(session_anchor_id),
                include_descendants: false,
                limit: 100,
                ..DocketTaskListFilter::default()
            },
        )
        .await?;
    Ok(task_list_projection_from_session_tasks(
        ctx.bear_id,
        ctx.profile,
        ctx.conversation_id,
        session_anchor_id,
        Some(client_session_id),
        &tasks,
    ))
}

pub fn projected_memory_session_diagnostic(projection: &KeyMemoryProjectionResult) -> Value {
    let included = projection
        .diagnostic
        .get("included")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let selected_paths = included
        .iter()
        .filter_map(|item| item.get("logical_path").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let prompt_memory = projection
        .diagnostic
        .get("prompt_memory")
        .cloned()
        .unwrap_or(Value::Null);
    let matched_block_ids = prompt_memory
        .get("matched_block_ids")
        .cloned()
        .unwrap_or_else(|| json!([]));
    json!({
        "status": "available",
        "count": included.len(),
        "selected_paths": selected_paths,
        "matched_block_ids": matched_block_ids,
        "reason": Value::Null,
        "next_surface": "projected key memory and prompt memory blocks already included in the model prompt",
    })
}

pub fn recalled_memory_session_diagnostic(recall: Option<&Value>) -> Value {
    match recall {
        Some(value) => {
            let top_paths = value
                .get("hits")
                .and_then(Value::as_array)
                .map(|hits| {
                    hits.iter()
                        .filter_map(|hit| hit.get("logical_path").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let count = value
                .get("hits")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(top_paths.len());
            json!({
                "status": "available",
                "count": count,
                "query": value.get("query_text").cloned().unwrap_or(Value::Null),
                "top_paths": top_paths,
                // Read-time conflict presence (ADR-0041 §8): pair count + record ids, when
                // the recall projection detected contradictory live records.
                "conflicts": value.get("conflicts").cloned().unwrap_or(Value::Null),
                "reason": Value::Null,
                "next_surface": "memory_search for canonical follow-up reads",
            })
        }
        None => json!({
            "status": "unavailable",
            "count": 0,
            "reason": "No recall passages were attached for this turn.",
            "next_surface": "memory_search / future recall diagnostic",
        }),
    }
}

fn objective_orientation_event_payload(
    profile: BearProfile,
    conversation_id: &str,
    orientation: &ObjectiveOrientation,
) -> Result<Value, DenError> {
    Ok(json!({
        "source": "turn_assembly",
        "profile": profile.as_str(),
        "conversation_id": conversation_id,
        "kind": orientation.kind(),
        "orientation": serde_json::to_value(orientation).map_err(|err| {
            DenError::System(format!("serialize objective orientation failed: {err}"))
        })?,
    }))
}

async fn record_objective_orientation_event(
    ctx: &AssembleTurnContext<'_>,
    orientation: &ObjectiveOrientation,
) -> Result<(), DenError> {
    let Some(session_id) = ctx.session_id else {
        return Ok(());
    };
    let payload =
        objective_orientation_event_payload(ctx.profile, ctx.conversation_id, orientation)?;
    let latest = crate::bearwire_events::latest_bearwire_event_of_type(
        ctx.pool,
        session_id,
        "runtime.objective_orientation",
    )
    .await?;
    // ponytail: de-dupe only against the latest same-session orientation event. If we need
    // cross-session coalescing or strict transition semantics, add a conversation-scoped key.
    if latest
        .as_ref()
        .map(|row| row.event.data == payload)
        .unwrap_or(false)
    {
        return Ok(());
    }

    let mut event = BearWireEvent::ephemeral("runtime.objective_orientation", payload);
    event.bear_id = Some(ctx.bear_id.to_string());
    event.role = Some(ctx.profile.as_str().to_string());
    event.human_id = ctx.user_id.map(|id| id.to_string());
    crate::bearwire_events::append_bearwire_event(
        ctx.pool,
        session_id,
        Some(ctx.bear_id),
        ctx.user_id,
        event,
    )
    .await?;
    Ok(())
}

fn objective_orientation_input(
    owns_session_tasks: bool,
    cached_activity_plan_projection: Option<&TaskListProjection>,
    explicit_session_current_task_id: Option<Uuid>,
    work_enabled: bool,
) -> ObjectiveOrientationResolutionInput {
    ObjectiveOrientationResolutionInput {
        docket_job_id: None,
        docket_execution_mutable: true,
        active_task_ref: if owns_session_tasks {
            // A session task tree is prompt context, not continuation authority.
            // Only the persisted explicit selection may orient a session-task loop.
            cached_activity_plan_projection.and_then(|plan| {
                explicit_session_current_task_id.and_then(|current_task_id| {
                    plan.items
                        .iter()
                        .find(|item| item.id == current_task_id.to_string())
                        .map(|item| orientation_task_ref_from_item(plan, item))
                })
            })
        } else {
            cached_activity_plan_projection.and_then(active_orientation_task_ref)
        },
        freeform_policy: if work_enabled {
            FreeformPolicy::task_definition_permitted()
        } else {
            FreeformPolicy::closed()
        },
    }
}

fn active_orientation_task_ref(plan: &TaskListProjection) -> Option<OrientationTaskRef> {
    if plan.status == "planned" {
        return None;
    }

    let item = plan.current_item.as_ref().or_else(|| {
        plan.items.iter().find(|item| {
            matches!(
                item.status,
                den_docket::TaskListItemStatus::InProgress
                    | den_docket::TaskListItemStatus::Pending
            )
        })
    })?;

    Some(orientation_task_ref_from_item(plan, item))
}

/// Best-effort `## Recalled memory` section (ADR-0038 Phase 2). Returns the rendered block and
/// its diagnostic, or `None` when recall is disabled/empty/failed — recall must never fail a turn.
async fn build_recall_section(
    ctx: &AssembleTurnContext<'_>,
    anchor_text: &str,
) -> Option<(String, Value)> {
    // TODO(ADR-0038 Phase 2 follow-up): enrich the recall query beyond the raw human message
    // with session focus + the primary work-surface context (see DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).
    let query_text = ctx.human_message.map(str::trim).filter(|s| !s.is_empty())?;
    let qdrant = crate::recall::QdrantRecall::from_config(ctx.config)?;
    let embedder = den_llm::EmbeddingClient::new(ctx.config);
    if !embedder.is_enabled() {
        return None;
    }
    let mut projection = match crate::recall::recall_for_turn_scoped(
        &qdrant,
        &embedder,
        &ctx.config.embedding_standard,
        ctx.bear_id,
        ctx.profile.as_str(),
        query_text,
        5,
    )
    .await
    {
        Ok(projection) => projection,
        Err(err) => {
            tracing::warn!(
                bear_id = %ctx.bear_id,
                error = %err,
                "recall query failed; continuing without recalled memory"
            );
            return None;
        }
    };
    // Read-time contradiction surfacing (ADR-0041 §8): detect over the retrieved passages,
    // mark counterparts, and emit best-effort `memory_conflict` observations.
    let memory_ids: Vec<String> = projection
        .passages
        .iter()
        .map(|p| p.memory_id.clone())
        .collect();
    let conflicts =
        crate::recall::surface_recall_conflicts(ctx.stores, ctx.bear_id, &memory_ids).await;
    crate::recall::mark_projection_conflicts(&mut projection, &conflicts);

    let mut block = crate::recall::render_recall_block(&projection, anchor_text)?;
    // Recall watermark turn annotation (ADR-0038 §8 Phase A3): one line, best-effort —
    // degraded recall must not read as absent memory. Errors mean no annotation, never a
    // failed turn.
    if let Some(lag_count) = recall_lag_count(ctx).await {
        if lag_count > 0 {
            block.push_str(&format!("\nrecall index {lag_count} records behind\n"));
        }
    }
    Some((block, projection.diagnostic))
}

/// Best-effort recall watermark lag for the turn annotation (ADR-0038 §8 Phase A3): `None`
/// when recall is unconfigured or the watermark cannot be computed (errors are logged at
/// debug and swallowed — the annotation is advisory only).
async fn recall_lag_count(ctx: &AssembleTurnContext<'_>) -> Option<i64> {
    let store = match ctx.stores.store_for_bear(ctx.bear_id).await {
        Ok(store) => store,
        Err(err) => {
            tracing::debug!(
                bear_id = %ctx.bear_id,
                error = %err,
                "recall watermark annotation skipped: store unavailable"
            );
            return None;
        }
    };
    match crate::recall::recall_watermark(ctx.pool, ctx.config, &store).await {
        Ok(watermark) => watermark.map(|wm| wm.lag_count),
        Err(err) => {
            tracing::debug!(
                bear_id = %ctx.bear_id,
                error = %err,
                "recall watermark annotation skipped: watermark unavailable"
            );
            None
        }
    }
}

pub async fn assemble_native_turn_messages(
    ctx: AssembleTurnContext<'_>,
) -> Result<Vec<ChatMessage>, DenError> {
    Ok(assemble_native_turn(ctx).await?.messages)
}

pub async fn assemble_native_turn(
    ctx: AssembleTurnContext<'_>,
) -> Result<AssembledNativeTurn, DenError> {
    let bear = bears_db::get_bear(ctx.pool, ctx.bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;
    assemble_native_turn_for_bear(ctx, &bear).await
}

pub async fn assemble_native_turn_messages_for_bear(
    ctx: AssembleTurnContext<'_>,
    bear: &Bear,
) -> Result<Vec<ChatMessage>, DenError> {
    Ok(assemble_native_turn_for_bear(ctx, bear).await?.messages)
}

pub async fn assemble_native_turn_for_bear(
    ctx: AssembleTurnContext<'_>,
    bear: &Bear,
) -> Result<AssembledNativeTurn, DenError> {
    let compiled_prompt = profile_prompt_text(ctx.pool, bear, ctx.profile).await?;
    let mut budget_components = AssembledTurnBudgetComponents {
        compiled_prompt_chars: compiled_prompt.chars().count() as u32,
        ..Default::default()
    };
    let model_for_profile = bears_db::resolve_model_for_profile(
        ctx.pool,
        bear,
        ctx.profile,
        &ctx.config.default_llm_model,
    )
    .await
    .ok();
    let projection = match project_key_memory(KeyMemoryProjectionInput {
        pool: ctx.pool,
        stores: ctx.stores,
        bear,
        profile: ctx.profile,
        conversation_id: ctx.conversation_id,
        session_hints: ctx.session_hints(),
        work_surface_status_override: ctx.work_surface_status_override(),
        native_runtime: ctx.native_runtime,
        model_for_budget: model_for_profile.as_deref(),
        // Fail-closed default: until session identity is resolved to entities (Phase 6),
        // any access-gated record is hidden. No-op today (no access rules exist yet).
        access: den_memory::AccessContext::empty(),
    })
    .await
    {
        Ok(projection) => projection,
        Err(err) => {
            tracing::warn!(
                bear_id = %ctx.bear_id,
                role = %ctx.profile.as_str(),
                conversation_id = %ctx.conversation_id,
                error = %err,
                "key memory projection failed; continuing without projected memory"
            );
            KeyMemoryProjectionResult {
                rendered_text: String::new(),
                diagnostic: serde_json::json!({
                    "source": "key_memory_projection",
                    "status": "error",
                    "error": err.to_string(),
                }),
                cache_key: KeyMemoryProjectionCacheKey {
                    bear_id: ctx.bear_id,
                    profile: ctx.profile,
                    conversation_id: ctx.conversation_id.to_string(),
                    primary_surface_slug: None,
                    sequence_high_water: 0,
                    compiled_config_token: String::new(),
                },
            }
        }
    };
    if let Some(expected) = ctx.key_memory_cache {
        if &projection.cache_key != expected {
            tracing::debug!(
                bear_id = %ctx.bear_id,
                conversation_id = %ctx.conversation_id,
                "key memory projection cache key changed during turn assembly"
            );
        }
    }

    let compaction_state = on_turn_assemble_compaction(
        ctx.pool,
        ctx.config,
        ctx.bear_id,
        ctx.conversation_id,
        ctx.profile,
    )
    .await?;
    let docket = PgDocketService::from_pool(ctx.pool);
    let cached_activity_plan_projection =
        load_cached_activity_plan_projection(&ctx, &docket).await?;
    let capabilities = den_core::EffectivePolicy::compile(
        ctx.profile,
        den_core::Governance::Interactive,
        if ctx.session_id.is_some() {
            den_core::ArmatureAvailability::Connected
        } else {
            den_core::ArmatureAvailability::Absent
        },
    )
    .capabilities;
    let explicit_session_current_task_id =
        if capabilities.contains(den_core::BearCapability::OwnSessionTasks) {
            match (ctx.user_id, ctx.session_id) {
                (Some(user_id), Some(client_session_id)) => {
                    client_sessions::find_for_user_bear_session_id(
                        ctx.pool,
                        user_id,
                        ctx.bear_id,
                        client_session_id,
                    )
                    .await?
                    .and_then(|session| session.current_task_id)
                }
                _ => None,
            }
        } else {
            None
        };
    let objective_orientation = super::resolve_objective_orientation(objective_orientation_input(
        capabilities.contains(den_core::BearCapability::OwnSessionTasks),
        cached_activity_plan_projection.as_ref(),
        explicit_session_current_task_id,
        bear.work_enabled,
    ));
    record_objective_orientation_event(&ctx, &objective_orientation).await?;

    let mut system_text = compiled_prompt;
    if let Some(block) = render_key_memory_projection_block(&projection) {
        budget_components.key_memory_projection_chars = block.chars().count() as u32;
        system_text.push_str("\n\n");
        system_text.push_str(&block);
    }
    let recall_diagnostic = match build_recall_section(&ctx, &system_text).await {
        Some((recall_block, diagnostic)) => {
            budget_components.recall_chars = recall_block.chars().count() as u32;
            system_text.push_str("\n\n");
            system_text.push_str(&recall_block);
            Some(diagnostic)
        }
        None => None,
    };
    if let Some(runtime) = ctx
        .turn_runtime_context
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        budget_components.runtime_supplement_chars = runtime.chars().count() as u32;
        system_text.push_str("\n\n");
        system_text.push_str(runtime);
    } else if ctx.should_load_den_owned_runtime_context() {
        let session_id = ctx.session_id.expect("session_id checked above");
        let roots = ctx
            .workspace_roots
            .map(|items| items.to_vec())
            .unwrap_or_default();
        let supplement = assemble_den_owned_runtime_supplement(
            ctx.pool,
            ctx.bear_id,
            ctx.profile.as_str(),
            session_id,
            &roots,
            &objective_orientation,
        )
        .await?;
        if !supplement.trim().is_empty() {
            budget_components.runtime_supplement_chars = supplement.chars().count() as u32;
            system_text.push_str("\n\n");
            system_text.push_str(&supplement);
        }
    }
    let capability_discovery = render_capability_discovery_guidance()?;
    if !capability_discovery.trim().is_empty() {
        budget_components.capability_discovery_chars = capability_discovery.chars().count() as u32;
        system_text.push_str("\n\n");
        system_text.push_str(&capability_discovery);
    }
    if ctx.profile == BearProfile::Chat {
        let tool_surface_blurb =
            den_core::tools::descriptor::render_profile_tool_surface_blurb(ctx.profile);
        budget_components.tool_surface_guidance_chars = tool_surface_blurb.chars().count() as u32;
        system_text.push_str("\n\n");
        system_text.push_str(&tool_surface_blurb);
    }

    if let Some(state) = compaction_state.as_ref() {
        let compaction_text = render_compaction_prompt_context(state);
        if !compaction_text.trim().is_empty() {
            budget_components.compaction_chars = compaction_text.chars().count() as u32;
            system_text.push_str("\n\n");
            system_text.push_str(&compaction_text);
        }
    }

    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: Some(system_text),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    }];
    let compaction_active =
        CompactionMode::parse(&ctx.config.compaction_mode) == CompactionMode::Active;
    let transcript_cutoff = compaction_state
        .as_ref()
        .and_then(|state| state.compacted_seq_cutoff);
    let transcript_messages = if compaction_active && transcript_cutoff.is_some() {
        load_transcript_messages_after_seq(
            ctx.pool,
            ctx.bear_id,
            ctx.conversation_id,
            transcript_cutoff,
        )
        .await?
    } else {
        load_transcript_messages(ctx.pool, ctx.bear_id, ctx.conversation_id).await?
    };
    budget_components.transcript_chars = transcript_messages
        .iter()
        .filter_map(|message| message.content.as_deref())
        .map(|value| value.chars().count() as u32)
        .sum();
    messages.extend(transcript_messages);
    if let Some(human) = ctx.human_message.map(str::trim).filter(|s| !s.is_empty()) {
        budget_components.current_user_input_chars = human.chars().count() as u32;
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(human.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        });
    }
    budget_components.tool_message_chars = ctx
        .tool_messages
        .iter()
        .filter_map(|message| message.content.as_deref())
        .map(|value| value.chars().count() as u32)
        .sum();
    messages.extend(ctx.tool_messages.iter().cloned());
    let messages = repair_tool_call_message_chain(messages);
    let messages = if ctx.native_runtime && compaction_active && transcript_cutoff.is_some() {
        messages
    } else if ctx.native_runtime && capabilities.contains(den_core::BearCapability::Converse) {
        let pruned = prune_messages_for_native_conversation_with_diagnostics(messages);
        budget_components.transcript_fallback_pruned_chars =
            pruned.diagnostics.pruned_character_count;
        budget_components.transcript_fallback_pruned_messages =
            pruned.diagnostics.pruned_message_count;
        budget_components.transcript_chars = budget_components
            .transcript_chars
            .saturating_sub(pruned.diagnostics.pruned_character_count);
        if pruned.diagnostics.pruned_message_count > 0 {
            tracing::warn!(
                bear_id = %ctx.bear_id,
                conversation_id = %ctx.conversation_id,
                profile = %ctx.profile.as_str(),
                pruned_message_count = pruned.diagnostics.pruned_message_count,
                pruned_character_count = pruned.diagnostics.pruned_character_count,
                "transcript replay used fallback pruning instead of compaction"
            );
        }
        pruned.messages
    } else {
        messages
    };
    Ok(AssembledNativeTurn {
        messages,
        key_memory_projection: Some(projection),
        recall_diagnostic,
        budget_components,
        cached_activity_plan_projection,
        objective_orientation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recalled_memory_session_diagnostic_surfaces_conflict_presence() {
        let diagnostic = json!({
            "source": "recall_query",
            "status": "ok",
            "hits": [],
            "conflicts": { "pairs": 1, "records": ["m1", "m2"] },
        });
        let session = recalled_memory_session_diagnostic(Some(&diagnostic));
        assert_eq!(session["conflicts"]["pairs"], 1);
        assert_eq!(session["conflicts"]["records"], json!(["m1", "m2"]));

        // No conflict info stays null rather than fabricating an empty object.
        let plain = recalled_memory_session_diagnostic(Some(&json!({ "hits": [] })));
        assert!(plain["conflicts"].is_null());
    }

    #[test]
    fn planned_activity_plan_does_not_orient_to_task() {
        let task_list_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
        let task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap();
        let item = den_docket::TaskListItem {
            id: task_id.to_string(),
            title: "Plan-only task".to_string(),
            summary: None,
            status: den_docket::TaskListItemStatus::Pending,
            blocked_reason: None,
            source_ref: den_docket::TaskListSourceRef::docket_task(
                None,
                task_id.to_string(),
                vec![format!("docket_task:{task_id}")],
            ),
            sync_state: den_docket::TaskListSyncState::Clean,
        };
        let plan = TaskListProjection {
            id: task_list_id,
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000789").unwrap(),
            title: "Session tasks".to_string(),
            summary: "Planned session tasks".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "private_to_profile".to_string(),
            status: "planned".to_string(),
            version: 1,
            source_ref: den_docket::TaskListSourceRef::local(vec![format!(
                "session_anchor:{task_list_id}"
            )]),
            items: vec![item.clone()],
            current_item: Some(item),
            source_conversation_id: Some("conversation-1".to_string()),
            source_client_session_id: Some(task_list_id.to_string()),
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        };

        assert!(active_orientation_task_ref(&plan).is_none());

        // A visible pending session task is context only until Pair explicitly
        // selects it as the current task.
        let orientation = crate::agent_loop::resolve_objective_orientation(
            objective_orientation_input(true, Some(&plan), None, true),
        );
        assert_eq!(
            orientation,
            ObjectiveOrientation::Freeform {
                policy: FreeformPolicy::task_definition_permitted(),
            }
        );

        let orientation = crate::agent_loop::resolve_objective_orientation(
            objective_orientation_input(true, Some(&plan), Some(task_id), true),
        );
        assert!(matches!(orientation, ObjectiveOrientation::Oriented { .. }));
        let runtime_task = crate::runtime::task_context::RuntimeTaskContext {
            source: crate::runtime::task_context::RuntimeTaskSource::SessionCurrentTask,
            current_task_id: Some(task_id),
            cached_activity_plan_projection: Some(plan),
        };
        assert_eq!(runtime_task.focused_orientation(), Some(orientation));
    }
}
