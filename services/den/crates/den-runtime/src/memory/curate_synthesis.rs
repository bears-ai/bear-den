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

struct CurateRequestInput<'a> {
    bear_id: Uuid,
    proposal_id: Uuid,
    bear_name: &'a str,
    hat_name: &'a str,
    model: String,
    bifrost_virtual_key: String,
    source_content: &'a str,
    proposal_summary: &'a str,
}

fn curate_request(input: CurateRequestInput<'_>) -> Result<ChatCompletionRequest, DenError> {
    let system = render_turn_fragment(
        repository_prompt_fragment_registry()?.require("curate_hat_synthesis")?,
        &json!({"bear_name": input.bear_name, "hat_name": input.hat_name}),
    )?;
    let evidence = serde_json::to_string(&json!({
        "source_note": input.source_content,
        "proposal_summary": input.proposal_summary,
    }))
    .map_err(|error| DenError::Parsing(format!("serialize Curate source data: {error}")))?;
    let message = |role: &str, content: String| ChatMessage {
        role: role.to_string(),
        content: Some(content),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    };
    Ok(ChatCompletionRequest {
        model: input.model,
        messages: vec![message("system", system), message("user", evidence)],
        tools: Vec::new(),
        stream: false,
        tool_choice: None,
        temperature: None,
        max_tokens: Some(800),
        thinking_effort: None,
        telemetry: Some(LlmRequestTelemetry {
            bear_id: Some(input.bear_id.to_string()),
            stance: Some(BearProfile::Curate.as_str().to_string()),
            operation: Some(LlmOperation::Memory),
            request_id: Some(input.proposal_id.to_string()),
            bifrost_virtual_key: Some(input.bifrost_virtual_key),
            ..Default::default()
        }),
    })
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
    if !hat.auto_curate_enabled {
        return Ok(None);
    }
    let bear = db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("Bear for Curate synthesis not found".into()))?;
    let model =
        db::resolve_model_for_profile(pool, &bear, BearProfile::Curate, llm.default_model())
            .await?;
    let request = curate_request(CurateRequestInput {
        bear_id,
        proposal_id,
        bear_name: &bear.name,
        hat_name: &hat.name,
        model: llm.resolve_model(Some(&model)),
        bifrost_virtual_key: key,
        source_content,
        proposal_summary,
    })?;
    let response = llm.chat_completions_stream(&request).await?;
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
        assert!(rendered.contains("authorized Job run wearing this hat"));
        assert!(rendered.contains("no Markdown"));
        assert!(!rendered.contains("Private untrusted source"));
    }

    #[test]
    fn curate_request_keeps_private_note_out_of_system_and_grants_no_tools() {
        let request = curate_request(CurateRequestInput {
            bear_id: Uuid::new_v4(),
            proposal_id: Uuid::new_v4(),
            bear_name: "Lumen",
            hat_name: "Security",
            model: "openai/test-model".into(),
            bifrost_virtual_key: "test-virtual-key".into(),
            source_content: "private token is untrusted data",
            proposal_summary: "Review without sharing private identifiers",
        })
        .unwrap();
        assert!(request.tools.is_empty());
        let body = request.to_body();
        assert_eq!(body["stream"], false);
        assert!(body.get("tools").is_none());
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(!body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("private token"));
        assert_eq!(body["messages"][1]["role"], "user");
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("private token"));
        assert!(
            !body.to_string().contains("test-virtual-key"),
            "gateway key is a header, not model context"
        );
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
