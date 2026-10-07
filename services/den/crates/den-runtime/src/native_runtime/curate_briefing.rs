//! One bounded, tool-free completion over the rule-based Curate briefing only.

use den_core::{ids::BearId, DenError};
use den_llm::{
    ChatCompletionRequest, ChatMessage, LlmApiStyle, LlmClient, LlmOperation, LlmRequestTelemetry,
};
use den_service::bears::{
    db, render_turn_fragment, repository_prompt_fragment_registry, RuntimeContextLabel,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::turn::NativeRuntimeDeps;
use crate::reflection::briefing_source::{
    resolve_curate_briefing_source, CurateBriefingSource, CurateBriefingText, ReflectionRunId,
};

const MAX_BRIEFING_BYTES: usize = 64_000;
const MAX_RESPONSE_BYTES: usize = 64_000;
const MAX_TEXT_BYTES: usize = 16_000;
const MAX_OUTPUT_TOKENS: u32 = 800;

struct BriefingRequestInput<'a> {
    bear_id: Uuid,
    run_id: ReflectionRunId,
    bear_name: &'a str,
    model: String,
    thinking_effort: Option<den_core::ThinkingEffort>,
    bifrost_virtual_key: String,
    briefing: &'a str,
}

fn briefing_request(input: BriefingRequestInput<'_>) -> Result<ChatCompletionRequest, DenError> {
    if input.briefing.trim().is_empty() || input.briefing.len() > MAX_BRIEFING_BYTES {
        return Err(DenError::ValidationError(
            "invalid Curate briefing size".into(),
        ));
    }
    let system = render_turn_fragment(
        repository_prompt_fragment_registry()?.require("curate_briefing")?,
        &json!({"bear_name": input.bear_name}),
    )?;
    let message = |role: &str, content: String| ChatMessage {
        role: role.into(),
        content: Some(content),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    };
    Ok(ChatCompletionRequest {
        model: input.model,
        messages: vec![
            message("system", system),
            message("user", input.briefing.into()),
        ],
        tools: Vec::new(),
        stream: false,
        tool_choice: None,
        temperature: None,
        max_tokens: Some(MAX_OUTPUT_TOKENS),
        thinking_effort: input.thinking_effort,
        telemetry: Some(LlmRequestTelemetry {
            bear_id: Some(input.bear_id.to_string()),
            stance: Some(RuntimeContextLabel::Curation.as_str().into()),
            operation: Some(LlmOperation::Memory),
            request_id: Some(input.run_id.as_uuid().to_string()),
            run_id: Some(input.run_id.as_uuid().to_string()),
            bifrost_virtual_key: Some(input.bifrost_virtual_key),
            ..Default::default()
        }),
    })
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: AssistantMessage,
    finish_reason: FinishReason,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    FunctionCall,
    ContentFilter,
}
#[derive(Deserialize)]
struct AssistantMessage {
    role: AssistantRole,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<den_llm::ChatToolCall>>,
    #[serde(default)]
    function_call: Option<LegacyFunctionCall>,
    #[serde(default)]
    refusal: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AssistantRole {
    Assistant,
}
#[derive(Deserialize)]
struct LegacyFunctionCall {
    #[serde(rename = "name")]
    _name: String,
    #[serde(rename = "arguments")]
    _arguments: String,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResponseStatus {
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    InProgress,
    Queued,
}
#[derive(Deserialize)]
struct ResponsesCompletion {
    status: ResponseStatus,
    output: Vec<ResponseOutput>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseOutput {
    Message {
        role: AssistantRole,
        status: ResponseStatus,
        content: Vec<ResponseContent>,
    },
    Reasoning {},
    // Unknown output types (including every tool-call type) fail closed.
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseContent {
    OutputText { text: String },
}

fn parse_briefing(bytes: &[u8], api_style: LlmApiStyle) -> Result<String, DenError> {
    let malformed = || DenError::Parsing("invalid tool-free Curate completion".into());
    let text = match api_style {
        LlmApiStyle::ChatCompletionsStream => {
            let mut completion: Completion =
                serde_json::from_slice(bytes).map_err(|_| malformed())?;
            if completion.choices.len() != 1 {
                return Err(malformed());
            }
            let choice = completion.choices.remove(0);
            let message = choice.message;
            let AssistantRole::Assistant = message.role;
            if message.tool_calls.is_some_and(|calls| !calls.is_empty())
                || message.function_call.is_some()
            {
                return Err(DenError::Authorization(
                    "Curate briefing may not request tools".into(),
                ));
            }
            if choice.finish_reason != FinishReason::Stop || message.refusal.is_some() {
                return Err(malformed());
            }
            message.content.ok_or_else(malformed)?
        }
        LlmApiStyle::ResponsesStream => {
            let completion: ResponsesCompletion =
                serde_json::from_slice(bytes).map_err(|_| malformed())?;
            if completion.status != ResponseStatus::Completed {
                return Err(malformed());
            }
            let mut text = String::new();
            let mut messages = 0;
            for output in completion.output {
                match output {
                    ResponseOutput::Message {
                        role,
                        status,
                        content,
                    } => {
                        let AssistantRole::Assistant = role;
                        if status != ResponseStatus::Completed {
                            return Err(malformed());
                        }
                        messages += 1;
                        for ResponseContent::OutputText { text: part } in content {
                            text.push_str(&part);
                        }
                    }
                    ResponseOutput::Reasoning {} => {}
                }
            }
            if messages != 1 {
                return Err(malformed());
            }
            text
        }
    };
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(DenError::ValidationError(
            "invalid Curate briefing text size".into(),
        ));
    }
    Ok(text)
}

async fn complete_briefing(
    llm: &LlmClient,
    request: &ChatCompletionRequest,
    api_style: LlmApiStyle,
) -> Result<String, DenError> {
    // Do not propagate provider bodies/URLs into the conductor's error log.
    let mut response = match api_style {
        LlmApiStyle::ChatCompletionsStream => llm.chat_completions_stream(request).await,
        LlmApiStyle::ResponsesStream => llm.responses_stream(request).await,
    }
    .map_err(|_| DenError::System("Curate briefing inference failed".into()))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DenError::Parsing("Curate briefing response transport failed".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(DenError::ValidationError(
                "Curate completion exceeded its byte limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_briefing(&bytes, api_style)
}

pub(super) async fn collect_assistant_text(
    deps: &NativeRuntimeDeps<'_>,
    bear_id: Uuid,
    reflection_run_id: ReflectionRunId,
    briefing: &str,
) -> Result<CurateBriefingText, DenError> {
    let source =
        resolve_curate_briefing_source(deps.pool, BearId::new(bear_id), reflection_run_id).await?;
    let llm = LlmClient::new(deps.config);
    if !llm.is_enabled() {
        return Err(DenError::System(
            "Curate briefing inference is not configured".into(),
        ));
    }
    let key = db::bifrost_virtual_key_for_inference(
        deps.pool,
        bear_id,
        &deps.config.den_secret_encryption_key,
    )
    .await
    .map_err(|_| DenError::System("Curate briefing Bear key resolution failed".into()))?
    .ok_or_else(|| DenError::Authorization("Curate briefing requires a Bear Bifrost key".into()))?;
    let bear = db::get_bear(deps.pool, bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("Curate briefing Bear not found".into()))?;
    // Internal Curate is Bear-scoped, not a human/Work hat turn.
    let primary = den_service::bears::model_configurations::resolve_primary(
        deps.pool,
        bear.id.into(),
        None,
        None,
        llm.default_model(),
    )
    .await?;
    let model = primary.model_handle.clone();
    let request = briefing_request(BriefingRequestInput {
        bear_id,
        run_id: reflection_run_id,
        bear_name: &bear.name,
        model: model.clone(),
        thinking_effort: primary.thinking_effort,
        bifrost_virtual_key: key,
        briefing,
    })?;
    source.require_live(deps.pool).await?;
    let api_style = crate::primary_model::execution_api_style(
        deps.pool,
        deps.config,
        bear_id.into(),
        &primary,
        crate::primary_model::transport_preference(
            den_core::TurnExecutionOrigin::InternalCuration,
            primary.thinking_effort,
        ),
    )
    .await?;
    let capabilities = den_service::bears::model_configurations::validate_model_configuration(
        deps.pool,
        &primary.model_handle,
        primary.thinking_effort,
    )
    .await?;
    let profile = den_core::ModelRequestProfile {
        approved_model_ref: primary.model_handle.clone(),
        supports_reasoning_effort: capabilities.supports_reasoning_effort,
        thinking_effort: primary.thinking_effort,
        ..Default::default()
    };
    crate::primary_model::persist_progress(
        deps.pool,
        bear.id.into(),
        None,
        source.session_id().as_str(),
        Some(&reflection_run_id.as_uuid().to_string()),
        crate::primary_model::configuration_progress_event(
            &primary,
            &profile,
            request.thinking_effort,
            api_style,
        ),
    )
    .await?;
    verified_completion(
        deps.pool,
        source,
        complete_briefing(&llm, &request, api_style),
    )
    .await
}

async fn verified_completion(
    pool: &sqlx::PgPool,
    source: CurateBriefingSource,
    completion: impl std::future::Future<Output = Result<String, DenError>>,
) -> Result<CurateBriefingText, DenError> {
    // The completion future must not be polled until authority is rechecked.
    source.require_live(pool).await?;
    // One completion, no agent retries/continuations/overflow calls. Transport retries
    // remain bounded by LlmClient, and the whole completion has a deadline.
    let text = tokio::time::timeout(std::time::Duration::from_secs(90), completion)
        .await
        .map_err(|_| DenError::System("Curate briefing inference timed out".into()))??;
    source.require_live(pool).await?;
    Ok(CurateBriefingText { source, text })
}

#[cfg(test)]
#[path = "curate_briefing/tests.rs"]
mod tests;
