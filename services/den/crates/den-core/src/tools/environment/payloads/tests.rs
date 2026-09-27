use super::{
    bear_environment_payload, session_info_payload, session_info_payload_with_visibility,
    trusted_workspace_roots_from_adapter_runtime,
};
use crate::tools::arguments::DenToolChannelContext;
use crate::tools::context::DenToolInvocationContext;
use crate::tools::descriptor::builtin_den_tool_descriptors_for_profile;
use crate::tools::prompt_memory::PromptMemoryVisibility;
use crate::BearProfile;
use serde_json::json;

fn pair_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(BearProfile::Pair),
        user_id: 1,
        username: Some("tester".to_string()),
        membership_role: None,
        conversation_id: "conv-test".to_string(),
        session_id: "sess-test".to_string(),
        work_run_id: None,
        client_session_id: Some("client-test".to_string()),
        conversation_selection: Some("src/main.rs".to_string()),
        runtime_target: Some("repo:builder-bear".to_string()),
        workspace_roots: vec!["/workspace".to_string()],
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    }
}

#[test]
fn bound_session_info_does_not_claim_profile_memory_scopes() {
    let context = pair_context();
    let status = json!({ "scope": "bound", "file_count": 1 });
    let payload = session_info_payload_with_visibility(
        &context,
        BearProfile::Pair,
        None,
        2,
        &status,
        &json!({}),
        PromptMemoryVisibility::BoundSession,
    );
    assert_eq!(
        payload["memory"]["read_scopes"],
        json!(["session/", "hat/", "core/"])
    );
    assert_eq!(payload["memory"]["write_scopes"], json!(["session/"]));
    assert_eq!(
        payload["policy"]["memory_scope_default"],
        "session/ + hat/ + core/"
    );
    assert_eq!(
        payload["runtime_context"]["memory_surface"],
        "session/ + hat/ + core/"
    );
    let unknown = session_info_payload_with_visibility(
        &context,
        BearProfile::Pair,
        None,
        2,
        &status,
        &json!({}),
        PromptMemoryVisibility::SharedOnly,
    );
    assert_eq!(unknown["memory"]["read_scopes"], json!(["core/"]));
    assert_eq!(unknown["memory"]["write_scopes"], json!([]));
}

#[test]
fn pair_session_info_context_fields_distinguish_role_contract_from_runtime() {
    let context = pair_context();
    let payload = session_info_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "available": true }),
        &json!({ "status": "ok" }),
    );

    assert_eq!(
        payload["role_contract_context"]["contract_label"],
        json!("Builder Bear")
    );
    assert_eq!(payload["role_contract_context"]["profile"], json!("pair"));
    assert_eq!(
        payload["runtime_context"]["active_bear_slug"],
        json!("test")
    );
    assert_eq!(
        payload["runtime_context"]["active_bear_authority"],
        json!("trusted_session")
    );
    assert_eq!(payload["context_composition_note"], json!("Role-contract context defines role behavior and style. Runtime context defines active Bear attachment, scope, attribution, workspace, and permissions for this session."));
    assert_eq!(payload["agent_context_summary"], json!("You are the pair-role collaborator operating under the Builder Bear role-contract context, currently attached to the test Bear runtime context."));
}

#[test]
fn pair_session_info_includes_runtime_health_and_context_budget_defaults() {
    let context = pair_context();
    let payload = session_info_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "available": true }),
        &json!({ "status": "ok" }),
    );

    assert_eq!(payload["runtime"]["state"], json!("idle"));
    assert_eq!(payload["runtime"]["active_turn"]["present"], json!(false));
    assert_eq!(
        payload["runtime"]["active_turn"]["pending_obligations"],
        json!(0)
    );
    assert_eq!(
        payload["runtime"]["active_turn"]["pending_adapter_tools"],
        json!(0)
    );
    assert_eq!(
        payload["runtime"]["active_turn"]["pending_den_tools"],
        json!(0)
    );
    assert_eq!(payload["runtime"]["source"], json!("session_info_default"));
    assert_eq!(payload["context_budget"]["status"], json!("unavailable"));
    assert_eq!(
        payload["context_budget"]["source"],
        json!("den.session_info")
    );
}

#[test]
fn pair_session_info_uses_context_runtime_health_when_available() {
    let mut context = pair_context();
    context.runtime = Some(json!({
        "state": "requires_action",
        "active_turn": {
            "present": true,
            "phase": "WaitingForObligations",
            "pending_obligations": 1,
            "pending_adapter_tools": 1,
            "pending_den_tools": 0,
            "pending_permissions": 0
        },
        "source": "client_active_turn_registry"
    }));
    context.context_budget = Some(json!({
        "status": "estimated",
        "used_tokens": 1000,
        "remaining_tokens": 9000,
        "source": "test"
    }));
    let payload = session_info_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "available": true }),
        &json!({ "status": "ok" }),
    );

    assert_eq!(payload["runtime"]["state"], json!("requires_action"));
    assert_eq!(payload["runtime"]["active_turn"]["present"], json!(true));
    assert_eq!(
        payload["runtime"]["active_turn"]["pending_adapter_tools"],
        json!(1)
    );
    assert_eq!(
        payload["runtime"]["source"],
        json!("client_active_turn_registry")
    );
    assert_eq!(payload["context_budget"]["status"], json!("estimated"));
    assert_eq!(payload["context_budget"]["source"], json!("test"));
}

#[test]
fn session_info_preserves_structured_runtime_budget_and_task_focus_state() {
    let mut context = pair_context();
    context.runtime = Some(json!({
        "schema": "den.runtime_state.v1",
        "state": "active",
        "budgets": {
            "turn": {
                "max_wall_clock_ms": 360000,
                "emergency_hard_steps": 80,
                "remaining_steps_before_hard_fuse": 77
            },
            "tool_calls": {
                "limits": { "total": 112 },
                "usage": { "total": 3 }
            }
        },
        "loop_guards": {
            "same_tool_signature_repeats": 1,
            "max_same_tool_signature_repeats": 2
        },
        "task_focus": {
            "active": true,
            "next_incomplete_task_title": "Run focused checks"
        },
        "last_budget_advisory": {
            "present": true,
            "summary": "Budget advisory: prefer final answer soon."
        }
    }));
    let payload = session_info_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "available": true }),
        &json!({ "status": "ok" }),
    );

    assert_eq!(
        payload["runtime"]["budgets"]["turn"]["emergency_hard_steps"],
        json!(80)
    );
    assert_eq!(
        payload["runtime"]["budgets"]["tool_calls"]["limits"]["total"],
        json!(112)
    );
    assert_eq!(
        payload["runtime"]["loop_guards"]["same_tool_signature_repeats"],
        json!(1)
    );
    assert_eq!(payload["runtime"]["task_focus"]["active"], json!(true));
    assert_eq!(
        payload["runtime"]["last_budget_advisory"]["present"],
        json!(true)
    );
}

#[test]
fn chat_profile_exposes_memory_read_and_write_tools() {
    let names: Vec<_> = builtin_den_tool_descriptors_for_profile(BearProfile::Chat)
        .into_iter()
        .filter(|descriptor| descriptor.domain == "memory")
        .map(|descriptor| descriptor.provider_name)
        .collect();
    assert!(names.contains(&"memory_search".to_string()));
    assert!(names.contains(&"memory_read".to_string()));
    assert!(names.contains(&"memory_write_entry".to_string()));
    assert!(!names.contains(&"memory_list_proposals".to_string()));
}

#[test]
fn chat_session_info_available_tools_match_memory_roster() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "meta".to_string(),
        binding_id: "agent-123".to_string(),
        profile: Some(BearProfile::Chat),
        user_id: 7,
        username: Some("gerwitz".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-123".to_string(),
        session_id: "sess-123".to_string(),
        work_run_id: None,
        client_session_id: None,
        conversation_selection: Some("conv-123".to_string()),
        runtime_target: Some("conv-123".to_string()),
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: Default::default(),
    };
    let payload = session_info_payload(
        &context,
        BearProfile::Chat,
        None,
        2,
        &json!({ "available": true }),
        &json!({ "status": "ok" }),
    );
    let tools = payload["memory"]["available_tools"]
        .as_array()
        .expect("available_tools array");
    let names: Vec<_> = tools.iter().filter_map(|value| value.as_str()).collect();
    assert!(names.contains(&"memory_search"));
    assert!(names.contains(&"memory_write_entry"));
    assert!(!names.contains(&"memory_list_proposals"));
}

#[test]
fn bear_environment_payload_exposes_baseline_sections() {
    let context = DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "meta".to_string(),
        binding_id: "agent-123".to_string(),
        profile: Some(BearProfile::Pair),
        user_id: 7,
        username: Some("gerwitz".to_string()),
        membership_role: Some("admin".to_string()),
        conversation_id: "conv-123".to_string(),
        session_id: "sess-123".to_string(),
        work_run_id: None,
        client_session_id: Some("client-123".to_string()),
        conversation_selection: Some("conv-123".to_string()),
        runtime_target: Some("conv-123".to_string()),
        workspace_roots: vec!["/workspace".to_string()],
        session_capabilities: Vec::new(),
        session_policy: Some(json!({ "mode_label": "Write" })),
        activity: None,
        runtime: Some(json!({
            "state": "running",
            "active_turn": { "present": true, "pending_obligations": 0 }
        })),
        context_budget: Some(json!({ "status": "unavailable" })),
        projected_memory: None,
        recalled_memory: None,
        request_id: Some("req-123".to_string()),
        channel: DenToolChannelContext {
            family: Some("bearwire".to_string()),
            client: Some("api-direct".to_string()),
            protocol: Some("bearwire".to_string()),
        },
    };
    let payload = bear_environment_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "configured": false, "available": false }),
        &json!({ "status": "ok" }),
        &json!({
            "status": "ok",
            "runtime": { "ok": true, "channel_kind": "client_session" },
            "adapter_environment": {
                "browser": { "active_source": "host_bridge", "status": "ok" },
                "services": { "den": { "status": "ok" } },
                "diagnostics": { "warnings": ["adapter warning"], "errors": [] }
            }
        }),
    );

    assert_eq!(payload["bear"]["slug"], "meta");
    assert_eq!(payload["runtime"]["state"], "running");
    assert_eq!(payload["session"]["id"], "sess-123");
    assert_eq!(payload["workspace"]["cwd"], "/workspace");
    assert_eq!(payload["browser"]["active_source"], "host_bridge");
    assert_eq!(payload["environment_variants"]["client"]["status"], "ok");
    assert_eq!(payload["environment_variants"]["adapter"]["status"], "ok");
    assert_eq!(payload["diagnostics"]["warnings"][0], "adapter warning");
    assert!(payload["tools"]["available_den_tools"].is_array());
}

#[test]
fn bear_environment_prefers_trusted_workspace_snapshot_when_present() {
    let context = pair_context();
    let payload = bear_environment_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "configured": false, "available": false }),
        &json!({ "status": "ok" }),
        &json!({
            "status": "ok",
            "trusted_workspace": {
                "cwd": "/workspace/project",
                "roots": ["/workspace/project", "/workspace/shared"],
                "source": "trusted_session"
            }
        }),
    );

    assert_eq!(payload["workspace"]["cwd"], "/workspace/project");
    assert_eq!(payload["workspace"]["roots"][0], "/workspace/project");
    assert_eq!(payload["workspace"]["roots"][1], "/workspace/shared");
    assert_eq!(payload["workspace"]["source"], "trusted_session");
}

#[test]
fn bear_environment_rejects_unknown_trusted_workspace_fields() {
    let context = pair_context();
    let payload = bear_environment_payload(
        &context,
        BearProfile::Pair,
        None,
        2,
        &json!({ "configured": false, "available": false }),
        &json!({ "status": "ok" }),
        &json!({
            "status": "ok",
            "trusted_workspace": {
                "cwd": "/untrusted/typo",
                "roots": ["/untrusted/typo"],
                "source": "trusted_session",
                "roooots": ["/typo"]
            }
        }),
    );

    assert_eq!(payload["workspace"]["cwd"], "/workspace");
    assert_eq!(payload["workspace"]["roots"], json!(["/workspace"]));
    assert_eq!(payload["workspace"]["source"], "trusted_session");
}

#[test]
fn trusted_workspace_roots_reject_unknown_adapter_fields() {
    let roots = trusted_workspace_roots_from_adapter_runtime(&json!({
        "trusted_workspace": {
            "cwd": "/workspace/project",
            "roots": ["/workspace/project"],
            "source": "trusted_session",
            "roooots": ["/typo"]
        }
    }));

    assert!(roots.is_empty());
}

#[test]
fn session_info_context_surfaces_degrade_to_explicit_unknowns() {
    let context = pair_context();
    let payload =
        session_info_payload(&context, BearProfile::Pair, None, 2, &json!({}), &json!({}));

    assert_eq!(
        payload["context_surfaces"]["schema"],
        "den.memory_context_layers.v1"
    );
    let layers = payload["context_surfaces"]["layers"]
        .as_array()
        .expect("context layers array");

    let layer = |name: &str| {
        layers
            .iter()
            .find(|layer| layer["name"] == name)
            .unwrap_or_else(|| panic!("missing {name} layer"))
    };

    let expected_layers = [
        "conversation_context",
        "context_budget",
        "projected_memory",
        "recalled_memory",
        "durable_memory",
        "task_work_state",
        "entity_context",
        "tool_runtime_surface",
    ];
    assert_eq!(layers.len(), expected_layers.len());
    for name in expected_layers {
        assert!(
            layer(name)["next_surface"].is_string(),
            "{name} has next_surface"
        );
    }

    assert_eq!(layer("context_budget")["status"], "unavailable");
    assert_eq!(layer("projected_memory")["status"], "unknown");
    assert_eq!(layer("projected_memory")["count"], "unknown");
    assert_eq!(layer("recalled_memory")["status"], "unknown");
    assert_eq!(layer("recalled_memory")["count"], "unknown");
    assert_eq!(layer("durable_memory")["status"], "unknown");
    assert_eq!(layer("task_work_state")["status"], "unknown");
    assert_eq!(layer("entity_context")["status"], "unknown");
    assert_eq!(
        payload["model_experience"]["schema"],
        "den.model_experience.memory_surfaces.v1"
    );
    assert_eq!(
        payload["model_experience"]["guide"],
        "docs/guides/bear-memory.md#model-experience"
    );
    assert!(payload["model_experience"]["rule"]
        .as_str()
        .expect("model experience rule")
        .contains("unknown or unavailable"));
    assert!(payload["model_experience"]["next_surfaces"]
        .as_array()
        .expect("model experience next surfaces")
        .iter()
        .any(|surface| surface == "session_info.context_surfaces.layers"));
}

#[test]
fn session_info_context_surfaces_include_projection_and_recall_diagnostics_when_present() {
    let mut context = pair_context();
    context.projected_memory = Some(json!({
        "status": "available",
        "count": 2,
        "selected_paths": ["core/bear-overview.md", "pair/decisions/mem-1.md"],
        "matched_block_ids": ["block-a"],
        "next_surface": "projected prompt"
    }));
    context.recalled_memory = Some(json!({
        "status": "available",
        "count": 1,
        "query": "fix memory surface",
        "top_paths": ["core/shared-conventions.md"],
        "next_surface": "memory_search"
    }));

    let payload =
        session_info_payload(&context, BearProfile::Pair, None, 2, &json!({}), &json!({}));
    let layers = payload["context_surfaces"]["layers"]
        .as_array()
        .expect("context layers array");
    let projected = layers
        .iter()
        .find(|layer| layer["name"] == "projected_memory")
        .expect("projected memory layer");
    let recalled = layers
        .iter()
        .find(|layer| layer["name"] == "recalled_memory")
        .expect("recalled memory layer");

    assert_eq!(projected["status"], "available");
    assert_eq!(projected["count"], 2);
    assert_eq!(projected["selected_paths"][0], "core/bear-overview.md");
    assert_eq!(projected["matched_block_ids"][0], "block-a");

    assert_eq!(recalled["status"], "available");
    assert_eq!(recalled["count"], 1);
    assert_eq!(recalled["query"], "fix memory surface");
    assert_eq!(recalled["top_paths"][0], "core/shared-conventions.md");
}
