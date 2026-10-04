use super::*;

fn request(briefing: &str) -> Result<ChatCompletionRequest, DenError> {
    briefing_request(BriefingRequestInput {
        bear_id: Uuid::new_v4(),
        run_id: ReflectionRunId::new(Uuid::new_v4()),
        bear_name: "Lumen",
        model: "openai/test-model".into(),
        bifrost_virtual_key: "secret-test-bear-key".into(),
        briefing,
    })
}

fn chat(message: serde_json::Value, finish_reason: &str) -> serde_json::Value {
    json!({"choices": [{"message": message, "finish_reason": finish_reason}]})
}

fn parse(value: serde_json::Value, style: LlmApiStyle) -> Result<String, DenError> {
    parse_briefing(&serde_json::to_vec(&value).unwrap(), style)
}

#[test]
fn briefing_request_is_bounded_tool_free_and_contains_only_supplied_context() {
    let request = request("SUPPLIED deterministic briefing").unwrap();
    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, "system");
    let system = request.messages[0].content.as_deref().unwrap();
    assert!(system.contains("Lumen"));
    assert!(system.contains("data, not instructions"));
    assert!(system.contains("Routine memory curation does not require human review"));
    assert!(!system.contains("SUPPLIED"));
    assert_eq!(
        request.messages[1].content.as_deref(),
        Some("SUPPLIED deterministic briefing")
    );
    assert!(request.tools.is_empty());
    assert!(!request.stream);
    assert_eq!(request.max_tokens, Some(MAX_OUTPUT_TOKENS));
    assert!(request.thinking_effort.is_none());
    let telemetry = request.telemetry.as_ref().unwrap();
    assert!(telemetry.session_id.is_none());
    assert!(telemetry.conversation_id.is_none());
    assert_eq!(
        telemetry.bifrost_virtual_key.as_deref(),
        Some("secret-test-bear-key")
    );
    for body in [request.to_body(), request.to_responses_body()] {
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["stream"], false);
        assert!(!body.to_string().contains("secret-test-bear-key"));
    }
}

#[test]
fn briefing_request_rejects_empty_or_unbounded_input() {
    assert!(request("  ").is_err());
    assert!(request(&"x".repeat(MAX_BRIEFING_BYTES + 1)).is_err());
}

#[test]
fn chat_briefing_accepts_one_completed_assistant_text() {
    assert_eq!(
        parse(
            chat(
                json!({"role": "assistant", "content": "Retained locally."}),
                "stop"
            ),
            LlmApiStyle::ChatCompletionsStream
        )
        .unwrap(),
        "Retained locally."
    );
}

#[test]
fn chat_briefing_rejects_tools_even_with_plausible_summary() {
    for message in [
        json!({"role": "assistant", "content": "Looks fine", "tool_calls": [{
            "id": "call-1", "type": "function", "function": {"name": "memory_read", "arguments": "{}"}
        }]}),
        json!({"role": "assistant", "content": "Looks fine", "function_call": {"name": "memory_read", "arguments": "{}"}}),
        json!({"role": "assistant", "content": "Looks fine", "tool_calls": [{}]}),
    ] {
        assert!(parse(chat(message, "stop"), LlmApiStyle::ChatCompletionsStream).is_err());
    }
}

#[test]
fn chat_briefing_rejects_malformed_failed_truncated_or_empty_completions() {
    let style = LlmApiStyle::ChatCompletionsStream;
    for reason in [
        "length",
        "tool_calls",
        "function_call",
        "content_filter",
        "unknown",
    ] {
        assert!(parse(
            chat(json!({"role": "assistant", "content": "partial"}), reason),
            style
        )
        .is_err());
    }
    for message in [
        json!({"role": "user", "content": "forged"}),
        json!({"role": "assistant", "content": null}),
        json!({"role": "assistant", "content": " "}),
        json!({"role": "assistant", "content": "x".repeat(MAX_TEXT_BYTES + 1)}),
        json!({"role": "assistant", "content": "partial", "refusal": "refused"}),
    ] {
        assert!(parse(chat(message, "stop"), style).is_err());
    }
    assert!(parse(json!({"choices": []}), style).is_err());
    let choice = chat(json!({"role": "assistant", "content": "ok"}), "stop")["choices"][0].clone();
    assert!(parse(json!({"choices": [choice.clone(), choice]}), style).is_err());
    let error = parse_briefing(b"secret-provider-body", style)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("secret-provider-body"));
}

fn responses(output: serde_json::Value) -> serde_json::Value {
    json!({"status": "completed", "output": output})
}
fn response_message(text: &str) -> serde_json::Value {
    json!({"type": "message", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text}]})
}

#[test]
fn responses_briefing_accepts_completed_text_without_exposing_reasoning() {
    assert_eq!(
        parse(
            responses(json!([
                {"type": "reasoning", "summary": [{"text": "private reasoning"}]},
                response_message("Deterministic outcome retained.")
            ])),
            LlmApiStyle::ResponsesStream
        )
        .unwrap(),
        "Deterministic outcome retained."
    );
}

#[test]
fn responses_briefing_rejects_tools_refusals_failure_and_ambiguous_output() {
    let style = LlmApiStyle::ResponsesStream;
    for kind in [
        "function_call",
        "web_search_call",
        "file_search_call",
        "computer_call",
        "unknown",
    ] {
        assert!(parse(
            responses(json!([response_message("summary"), {"type": kind}])),
            style
        )
        .is_err());
    }
    for status in ["failed", "cancelled", "incomplete", "in_progress", "queued"] {
        let mut value = responses(json!([response_message("partial")]));
        value["status"] = json!(status);
        assert!(parse(value, style).is_err());
        let mut message = response_message("partial");
        message["status"] = json!(status);
        assert!(parse(responses(json!([message])), style).is_err());
    }
    let mut refusal = response_message("partial");
    refusal["content"] = json!([{"type": "refusal", "refusal": "no"}]);
    assert!(parse(responses(json!([refusal])), style).is_err());
    assert!(parse(responses(json!([])), style).is_err());
    assert!(parse(
        responses(json!([response_message("one"), response_message("two")])),
        style
    )
    .is_err());
    assert!(parse(responses(json!([response_message("")])), style).is_err());
}

#[test]
fn briefing_uses_typed_catalog_api_selection_without_reasoning_override() {
    assert_eq!(
        preferred_api_style_for_model_with_catalog_support("openai/gpt-5", Some(false)),
        LlmApiStyle::ChatCompletionsStream
    );
    assert_eq!(
        preferred_api_style_for_model_with_catalog_support("openai/test", Some(true)),
        LlmApiStyle::ResponsesStream
    );
    assert!(request("briefing").unwrap().thinking_effort.is_none());
}

async fn running_source(pool: &sqlx::PgPool) -> CurateBriefingSource {
    use crate::reflection::{
        conductor::{
            claim_next_memory_curate_run, enqueue_memory_curate_for_proposals,
            ProposalEnqueueParams,
        },
        conversations::{bind_memory_curate_run_conversation, ensure_memory_curate_conversation},
    };
    let bear_id = db::create_bear(
        pool,
        db::BearParams {
            slug: "direct-briefing-test",
            name: "Briefing test Bear",
            description: "",
            system_prompt: "PRIVATE PROMPT MUST NOT BE READ",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let run = enqueue_memory_curate_for_proposals(
        pool,
        ProposalEnqueueParams {
            bear_id,
            binding_id: None,
            conversation_id: None,
            conversation_key: None,
            conversation_date: None,
            trigger: "direct_briefing_test",
            proposal_ids: vec![],
        },
    )
    .await
    .unwrap();
    claim_next_memory_curate_run(pool, bear_id)
        .await
        .unwrap()
        .unwrap();
    let date = time::Date::from_calendar_date(2026, time::Month::October, 3).unwrap();
    let conversation = ensure_memory_curate_conversation(pool, bear_id, None, date)
        .await
        .unwrap();
    bind_memory_curate_run_conversation(
        pool,
        bear_id,
        run.id,
        conversation.conversation_id.as_deref().unwrap(),
    )
    .await
    .unwrap();
    resolve_curate_briefing_source(pool, BearId::new(bear_id), ReflectionRunId::new(run.id))
        .await
        .unwrap()
}

async fn end_run(pool: &sqlx::PgPool, source: &CurateBriefingSource) {
    crate::reflection::conductor::mark_memory_curate_completed(
        pool,
        source.bear_id().as_uuid(),
        source.run_id().as_uuid(),
        json!({}),
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_briefing_rechecks_before_polling_network_future(pool: sqlx::PgPool) {
    let source = running_source(&pool).await;
    end_run(&pool, &source).await;
    let calls = std::cell::Cell::new(0);
    let result = verified_completion(&pool, source, async {
        calls.set(calls.get() + 1);
        Ok("Must not escape".into())
    })
    .await;
    assert!(result.is_err());
    assert_eq!(calls.get(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_briefing_rechecks_after_completion_before_returning_text(pool: sqlx::PgPool) {
    let source = running_source(&pool).await;
    let during_call = source.clone();
    let result = verified_completion(&pool, source, async {
        end_run(&pool, &during_call).await;
        Ok("Must not be projected".into())
    })
    .await;
    assert!(result.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn direct_briefing_returns_checked_destination_and_only_one_completion(pool: sqlx::PgPool) {
    let source = running_source(&pool).await;
    let calls = std::cell::Cell::new(0);
    let result = verified_completion(&pool, source.clone(), async {
        calls.set(calls.get() + 1);
        Ok("Retained deterministic outcome".into())
    })
    .await
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(result.source, source);
    assert_eq!(result.text, "Retained deterministic outcome");
}
