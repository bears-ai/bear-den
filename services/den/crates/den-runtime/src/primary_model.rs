//! Primary model policy resolved from canonical sources, never runtime-target text.

use std::sync::{Arc, OnceLock};

use den_core::{config::Config, ids::BearId, DenError, ThinkingEffort, TurnExecutionOrigin};
use den_llm::{primary_api_style_for_catalog_support, PrimaryTransportPreference};
use den_memory::{scoped::MemoryReadGrant, MemorySource};
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
use den_service::bears::{
    hats::{memory_binding, turn_binding::NativeTurnSource},
    model_configurations::{self, PrimaryModelSource, ResolvedPrimaryModel},
};
use den_service::bifrost::{BifrostCatalogEntry, BifrostCatalogSnapshot, BifrostClient};
use sqlx::PgPool;

use crate::llm::LlmApiStyle;

pub(crate) async fn resolve_for_grant(
    pool: &PgPool,
    bear_id: BearId,
    grant: MemoryReadGrant,
    deployment_default: &str,
) -> Result<ResolvedPrimaryModel, DenError> {
    let pin = match grant.source() {
        MemorySource::Conversation(id) => {
            den_service::model_selection::conversation_model_pin(pool, id).await?
        }
        MemorySource::WorkRun(_) => None,
        MemorySource::Intake(_) => {
            return Err(DenError::Authorization(
                "intake is not a primary turn source".into(),
            ));
        }
    };
    model_configurations::resolve_primary(
        pool,
        bear_id,
        grant.hat_id(),
        pin.as_deref(),
        deployment_default,
    )
    .await
}

pub(crate) async fn resolve_for_source(
    pool: &PgPool,
    bear_id: BearId,
    source: NativeTurnSource,
    deployment_default: &str,
) -> Result<ResolvedPrimaryModel, DenError> {
    let memory_binding::ResolvedMemoryBinding::Bound(grant) = match source {
        NativeTurnSource::Conversation(id) => {
            memory_binding::for_conversation(pool, bear_id, id).await?
        }
        NativeTurnSource::WorkRun(id) => memory_binding::for_work_run(pool, bear_id, id).await?,
    };
    resolve_for_grant(pool, bear_id, grant, deployment_default).await
}

static BIFROST_CLIENT: OnceLock<Arc<BifrostClient>> = OnceLock::new();

/// Install DenState's process-owned client before runtime workers/edges start.
/// Cloning this Arc shares the HTTP pool and Bear catalog cache; first install wins.
pub fn set_bifrost_client(client: Arc<BifrostClient>) {
    if BIFROST_CLIENT.set(client).is_err() {
        tracing::warn!(
            "runtime Bifrost client already initialized; keeping the process-owned instance"
        );
    }
}

fn bifrost_client(config: &Config) -> Result<Arc<BifrostClient>, DenError> {
    if let Some(client) = BIFROST_CLIENT.get() {
        return Ok(Arc::clone(client));
    }
    // Isolated unit/fixture harnesses do not run application startup. This local
    // fallback is never installed globally; cache-sensitive tests pass one client
    // explicitly to execution_api_style_with_client instead of using this path.
    #[cfg(any(test, feature = "test-util"))]
    {
        Ok(Arc::new(BifrostClient::new(config)))
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        let _ = config;
        Err(DenError::System(
            "runtime process-owned Bifrost client is not initialized".into(),
        ))
    }
}

pub(crate) fn transport_preference(
    origin: TurnExecutionOrigin,
    thinking_effort: Option<ThinkingEffort>,
) -> PrimaryTransportPreference {
    if matches!(
        origin,
        TurnExecutionOrigin::ArmatureConversation(_) | TurnExecutionOrigin::AuthorizedWorkRun(_)
    ) || thinking_effort.is_some()
    {
        PrimaryTransportPreference::ResponsesWhenUnknown
    } else {
        PrimaryTransportPreference::ChatCompletionsWhenUnknown
    }
}

/// Adapter hints can refer to a lower-precedence model. Resolve the actual
/// selection using the same client/cache as BearWire, without provider inference.
pub(crate) async fn execution_api_style(
    pool: &PgPool,
    config: &Config,
    bear_id: BearId,
    primary: &ResolvedPrimaryModel,
    preference: PrimaryTransportPreference,
) -> Result<LlmApiStyle, DenError> {
    // Disabled-client unit/fixture tests intentionally do not contact a gateway.
    // This is a test-local routing fixture, not production availability authority.
    #[cfg(any(test, feature = "test-util"))]
    if config.llm_api_url.trim().is_empty() {
        let support = den_llm::model_registry::entry_for_handle(&primary.model_handle)
            .map(|entry| entry.supports_responses_api);
        return Ok(primary_api_style_for_catalog_support(support, preference));
    }
    let client = bifrost_client(config)?;
    execution_api_style_with_client(&client, pool, config, bear_id, primary, preference).await
}

async fn execution_api_style_with_client(
    client: &BifrostClient,
    pool: &PgPool,
    config: &Config,
    bear_id: BearId,
    primary: &ResolvedPrimaryModel,
    preference: PrimaryTransportPreference,
) -> Result<LlmApiStyle, DenError> {
    let catalog = client
        .bear_catalog_snapshot(pool, bear_id.as_uuid(), &config.den_secret_encryption_key)
        .await;
    catalog_api_style(client, bear_id, primary, preference, catalog)
}

fn catalog_api_style(
    client: &BifrostClient,
    bear_id: BearId,
    primary: &ResolvedPrimaryModel,
    preference: PrimaryTransportPreference,
    catalog: Result<BifrostCatalogSnapshot, DenError>,
) -> Result<LlmApiStyle, DenError> {
    match catalog {
        Ok(snapshot) => {
            let entry = snapshot.resolve(&primary.model_handle).ok_or_else(|| {
                DenError::ValidationError(format!(
                    "selected primary model is missing from the Bifrost catalog: {}",
                    primary.model_handle
                ))
            })?;
            available_entry_api_style(entry, preference)
        }
        Err(error) if primary.source == PrimaryModelSource::ConversationPin => {
            // Match BearWire's explicit-pin outage continuity: use this client's
            // cached entry if present, otherwise attempt the same pinned model via
            // Responses. A successful catalog never permits missing-entry continuity.
            if let Some(snapshot) = client.cached_bear_catalog_snapshot(bear_id.as_uuid()) {
                if let Some(entry) = snapshot.resolve(&primary.model_handle) {
                    let style = available_entry_api_style(
                        entry,
                        PrimaryTransportPreference::ResponsesWhenUnknown,
                    )?;
                    tracing::warn!(bear_id = %bear_id, model = %primary.model_handle, error = %error,
                        "Bifrost catalog unavailable; retaining explicit pin with the shared cached transport");
                    return Ok(style);
                }
            }
            tracing::warn!(bear_id = %bear_id, model = %primary.model_handle, error = %error,
                "Bifrost catalog unavailable without a usable cached entry; retaining explicit pin via Responses");
            Ok(primary_api_style_for_catalog_support(
                None,
                PrimaryTransportPreference::ResponsesWhenUnknown,
            ))
        }
        Err(error) => Err(error),
    }
}

fn available_entry_api_style(
    entry: &BifrostCatalogEntry,
    preference: PrimaryTransportPreference,
) -> Result<LlmApiStyle, DenError> {
    if !entry.available {
        return Err(DenError::ValidationError(
            "selected primary model is unavailable".into(),
        ));
    }
    Ok(primary_api_style_for_catalog_support(
        entry.supports_responses_api,
        preference,
    ))
}

pub(crate) fn compatible_effort(
    api_style: LlmApiStyle,
    has_function_tools: bool,
    effort: Option<ThinkingEffort>,
) -> Option<ThinkingEffort> {
    // Bifrost's Chat Completions function-tool bridge cannot carry this setting
    // safely (notably Anthropic). Do not claim the configured effort was sent.
    if api_style == LlmApiStyle::ChatCompletionsStream && has_function_tools {
        None
    } else {
        effort
    }
}

pub(crate) fn configuration_progress_event(
    primary: &ResolvedPrimaryModel,
    profile: &den_core::ModelRequestProfile,
    effective_effort: Option<ThinkingEffort>,
    api_style: LlmApiStyle,
) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "model_request_profile_resolved".into(),
        text: None,
        phase: Some("model_routing".into()),
        detail: Some(serde_json::json!({
            "configuration_id": primary.configuration_id,
            "configuration_name": primary.configuration_name,
            "source": primary.source,
            "model": primary.model_handle,
            "approved_model_ref": profile.approved_model_ref,
            "configured_thinking_effort": primary.thinking_effort.map(ThinkingEffort::as_str),
            "agent_primary_step": profile.agent_primary_step.as_str(),
            "supports_reasoning_effort": profile.supports_reasoning_effort,
            "thinking_effort": profile.thinking_effort.map(ThinkingEffort::as_str),
            "effective_request_effort": effective_effort.map(ThinkingEffort::as_str),
            "api_style": api_style.as_str(),
            "reasoning_disposition": if profile.thinking_effort.is_some() && effective_effort.is_none() {
                "skipped_api_incompatible"
            } else if effective_effort.is_some() {
                "applied"
            } else {
                "model_default"
            },
        })),
    })
}

/// Channels and tool-free internal calls do not pass through BearWire's run
/// event publisher. Persist their model diagnostic through the same event log.
pub(crate) async fn persist_progress(
    pool: &PgPool,
    bear_id: BearId,
    user_id: Option<i32>,
    session_id: &str,
    run_id: Option<&str>,
    event: RuntimeStreamEvent,
) -> Result<(), DenError> {
    for event in
        crate::runtime::bearwire_projection::wire::runtime_stream_event_to_bearwire_events(event)
    {
        let mut event = event
            .with_run_id(run_id.map(str::to_owned))
            .with_session(session_id.to_owned());
        event.bear_id = Some(bear_id.to_string());
        event.human_id = user_id.map(|id| id.to_string());
        crate::bearwire_events::append_bearwire_event(
            pool,
            session_id,
            Some(bear_id.as_uuid()),
            user_id,
            event,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "primary_model_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "primary_model_tests.rs"]
pub(crate) mod tests;
