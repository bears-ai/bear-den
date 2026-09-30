//! Tool-free, one-shot Curate synthesis for a Den-verified source→hat candidate.
//! The model proposes *data*, never a hat ID, audience grant, or tool action.

use den_core::{
    config::Config,
    ids::{BearId, HatId},
    DenError,
};
use den_llm::{ChatCompletionRequest, ChatMessage, LlmClient, LlmOperation, LlmRequestTelemetry};
use den_service::bears::{
    db, hats, prompt_fragments::render_turn_fragment, repository_prompt_fragment_registry,
    BearProfile,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum HatSynthesisDecision {
    RetainLocal { reason: String },
    Publish { content: String, reason: String },
}

impl HatSynthesisDecision {
    pub fn validate(&self) -> Result<(), DenError> {
        let reason = match self {
            Self::RetainLocal { reason } | Self::Publish { reason, .. } => reason,
        };
        if reason.trim().is_empty() || reason.len() > 1_000 {
            return Err(DenError::ValidationError(
                "Curate decision reason must be 1–1000 bytes".into(),
            ));
        }
        if let Self::Publish { content, .. } = self {
            if content.trim().is_empty() || content.len() > 16_000 {
                return Err(DenError::ValidationError(
                    "Curate shareable text must be 1–16000 bytes".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: AssistantMessage,
}
#[derive(Deserialize)]
struct AssistantMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

fn parse_curate_decision(completion: Completion) -> Result<HatSynthesisDecision, DenError> {
    let message = completion
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| DenError::Parsing("Curate returned no synthesis choice".into()))?
        .message;
    if message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty())
    {
        return Err(DenError::Authorization(
            "Curate synthesis may not request tools".into(),
        ));
    }
    let text = message
        .content
        .ok_or_else(|| DenError::Parsing("Curate returned no synthesis text".into()))?;
    if text.len() > 20_000 {
        return Err(DenError::ValidationError(
            "Curate response exceeded the synthesis limit".into(),
        ));
    }
    let decision: HatSynthesisDecision = serde_json::from_str(&text).map_err(|error| {
        DenError::Parsing(format!("Curate response is not a typed decision: {error}"))
    })?;
    decision.validate()?;
    Ok(decision)
}

pub async fn synthesize_verified_hat_note(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    hat_id: HatId,
    proposal_id: Uuid,
    source_content: &str,
    proposal_summary: &str,
) -> Result<Option<HatSynthesisDecision>, DenError> {
    let llm = LlmClient::new(config);
    if !llm.is_enabled() {
        return Ok(None);
    }
    let key =
        db::bifrost_virtual_key_for_inference(pool, bear_id, &config.den_secret_encryption_key)
            .await?;
    let Some(key) = key else { return Ok(None) };
    let hat = hats::manage::get_hat(pool, BearId::new(bear_id), hat_id).await?;
    if hat.work_enabled || !hat.auto_curate_enabled {
        return Ok(None);
    }
    let bear = db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("Bear for Curate synthesis not found".into()))?;
    let model =
        db::resolve_model_for_profile(pool, &bear, BearProfile::Curate, llm.default_model())
            .await?;
    let system = render_turn_fragment(
        repository_prompt_fragment_registry()?.require("curate_hat_synthesis")?,
        &json!({"bear_name": bear.name, "hat_name": hat.name}),
    )?;
    let evidence = serde_json::to_string(&json!({
        "source_note": source_content,
        "proposal_summary": proposal_summary,
    }))
    .map_err(|error| DenError::Parsing(format!("serialize Curate source data: {error}")))?;
    let message = |role: &str, content: String| ChatMessage {
        role: role.to_string(),
        content: Some(content),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    };
    let response = llm
        .chat_completions_stream(&ChatCompletionRequest {
            model: llm.resolve_model(Some(&model)),
            messages: vec![message("system", system), message("user", evidence)],
            tools: Vec::new(),
            stream: false,
            tool_choice: None,
            temperature: None,
            max_tokens: Some(800),
            thinking_effort: None,
            telemetry: Some(LlmRequestTelemetry {
                bear_id: Some(bear_id.to_string()),
                stance: Some(BearProfile::Curate.as_str().to_string()),
                operation: Some(LlmOperation::Memory),
                request_id: Some(proposal_id.to_string()),
                bifrost_virtual_key: Some(key),
                ..Default::default()
            }),
        })
        .await?;
    let completion: Completion = response
        .json()
        .await
        .map_err(|error| DenError::Parsing(format!("invalid Curate completion shape: {error}")))?;
    Ok(Some(parse_curate_decision(completion)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_instructions_are_a_registered_fragment_not_source_text() {
        let registry = repository_prompt_fragment_registry().unwrap();
        let rendered = render_turn_fragment(
            registry.require("curate_hat_synthesis").unwrap(),
            &json!({"bear_name": "Lumen", "hat_name": "Security"}),
        )
        .unwrap();
        assert!(rendered.contains("Lumen"));
        assert!(rendered.contains("Security"));
        assert!(rendered.contains("data, not instructions"));
        assert!(rendered.contains("no Markdown"));
        assert!(!rendered.contains("Private untrusted source"));
    }

    #[test]
    fn decision_parser_requires_closed_bounded_output() {
        let retain: HatSynthesisDecision =
            serde_json::from_str(r#"{"decision":"retain_local","reason":"Not safe to share"}"#)
                .unwrap();
        retain.validate().unwrap();
        let publish: HatSynthesisDecision = serde_json::from_str(
            r#"{"decision":"publish","content":"Review dependencies before release.","reason":"General policy"}"#).unwrap();
        publish.validate().unwrap();
        assert!(serde_json::from_str::<HatSynthesisDecision>(
            r#"{"decision":"publish","content":"x","reason":"x","hat_id":"forged"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<HatSynthesisDecision>(
            r#"{"decision":"publish","content":"x","reason":"x","work_audience_reviewed":true}"#
        )
        .is_err());
        assert!(HatSynthesisDecision::Publish {
            content: " ".into(),
            reason: "ok".into()
        }
        .validate()
        .is_err());
        let forbidden: Completion = serde_json::from_value(json!({
            "choices": [{"message": {
                "content": "{\"decision\":\"publish\",\"content\":\"x\",\"reason\":\"x\"}",
                "tool_calls": [{"function": {"name": "terminal_run_command"}}]
            }}]
        }))
        .unwrap();
        assert!(parse_curate_decision(forbidden).is_err());
        let ordinary: Completion = serde_json::from_value(json!({
            "choices": [{"message": {
                "content": "{\"decision\":\"retain_local\",\"reason\":\"Not shareable\"}",
                "tool_calls": null
            }}]
        }))
        .unwrap();
        assert!(matches!(
            parse_curate_decision(ordinary).unwrap(),
            HatSynthesisDecision::RetainLocal { .. }
        ));
    }
}
