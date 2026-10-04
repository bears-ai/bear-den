use super::{
    create_work_surface_scaffold, infer_work_surface_hint, orient_work_surface, ScaffoldRequest,
    WorkSurfaceOps, WorkSurfaceScaffoldOutcome, WorkSurfaceSessionAnchor,
};
use crate::tools::context::DenToolInvocationContext;
use crate::{
    ArmatureAvailability, DenError, EffectivePolicy, Governance, RuntimeContextLabel,
    TurnExecutionOrigin,
};
use serde_json::json;
use std::{
    future::Future,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

fn pair_context() -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: uuid::Uuid::nil(),
        bear_slug: "test".to_string(),
        binding_id: "agent".to_string(),
        profile: Some(RuntimeContextLabel::ArmatureConversation),
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

fn immediate<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fake storage unexpectedly yielded a pending future"),
    }
}

#[derive(Default)]
struct RecordingOps {
    writes: Mutex<Vec<(RuntimeContextLabel, Vec<ScaffoldRequest>)>>,
}

impl WorkSurfaceOps for RecordingOps {
    async fn write_scaffold(
        &self,
        _: uuid::Uuid,
        role: RuntimeContextLabel,
        _: &str,
        _: &str,
        requests: Vec<ScaffoldRequest>,
    ) -> Result<WorkSurfaceScaffoldOutcome, DenError> {
        self.writes.lock().unwrap().push((role, requests));
        Ok(WorkSurfaceScaffoldOutcome {
            storage: None,
            updates: Vec::new(),
        })
    }

    async fn orient(
        &self,
        _: &DenToolInvocationContext,
        _: RuntimeContextLabel,
    ) -> Result<serde_json::Value, DenError> {
        unreachable!()
    }
}

fn scaffold_arguments() -> serde_json::Value {
    json!({
        "work_surface_slug": "example",
        "work_surface_name": "Example",
        "overview": "Example work surface",
        "current_understanding": "Current understanding",
    })
}

#[test]
fn work_origin_cannot_scaffold_with_claimed_pair_profile_and_client_id() {
    let context = pair_context();
    for availability in [
        ArmatureAvailability::Connected,
        ArmatureAvailability::Absent,
    ] {
        let policy = EffectivePolicy::compile_for_origin(
            TurnExecutionOrigin::AuthorizedWorkRun(availability),
            Governance::Interactive,
        );
        let ops = RecordingOps::default();
        let result = immediate(create_work_surface_scaffold(
            &ops,
            &context,
            &policy,
            scaffold_arguments(),
        ));
        assert!(matches!(result, Err(DenError::NotFound(_))));
        assert!(ops.writes.lock().unwrap().is_empty());
    }
}

#[test]
fn noninteractive_editor_governance_cannot_scaffold_before_storage() {
    let context = pair_context();
    for governance in [
        Governance::Grace,
        Governance::AutonomousContinuation,
        Governance::Observational,
        Governance::Frozen,
    ] {
        let policy = EffectivePolicy::compile_for_origin(
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            governance,
        );
        let ops = RecordingOps::default();
        let result = immediate(create_work_surface_scaffold(
            &ops,
            &context,
            &policy,
            scaffold_arguments(),
        ));
        assert!(
            matches!(result, Err(DenError::NotFound(_))),
            "{governance:?}"
        );
        assert!(ops.writes.lock().unwrap().is_empty());
    }
}

#[test]
fn retired_work_surface_invocations_deny_every_profile_before_storage() {
    for role in [
        RuntimeContextLabel::ChannelConversation,
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::Curation,
        RuntimeContextLabel::Observation,
    ] {
        let mut context = pair_context();
        context.profile = Some(role);
        let policy = EffectivePolicy::compile_for_origin(
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            Governance::Interactive,
        );
        let ops = RecordingOps::default();
        for arguments in [scaffold_arguments(), json!(null)] {
            assert!(matches!(
                immediate(create_work_surface_scaffold(
                    &ops, &context, &policy, arguments
                )),
                Err(DenError::NotFound(_))
            ));
        }
        assert!(matches!(
            immediate(orient_work_surface(&ops, &context, role)),
            Err(DenError::NotFound(_))
        ));
        assert!(ops.writes.lock().unwrap().is_empty());
    }
}

#[test]
fn infer_work_surface_hint_surfaces_trusted_candidates() {
    let payload =
        infer_work_surface_hint(&pair_context(), RuntimeContextLabel::ArmatureConversation);
    assert_eq!(payload["workplace"]["profile"], json!("pair"));
    assert_eq!(payload["workplace"]["memory_surface"], json!("pair/"));
    assert_eq!(payload["work_surface"]["status"], json!("candidate"));
    assert_eq!(payload["work_surface"]["confidence"], json!("medium"));
    assert_eq!(
        payload["work_surface"]["needs_user_confirmation"],
        json!(false)
    );
    assert_eq!(
        payload["work_surface"]["agent_guidance"]["may_state_assumption"],
        json!(true)
    );
    let candidates = payload["work_surface"]["reference_candidates"]
        .as_array()
        .expect("reference candidates array");
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("runtime_target")));
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("conversation_selection")));
    assert!(candidates
        .iter()
        .any(|item| item["kind"] == json!("workspace_root")));
}

#[test]
fn infer_work_surface_hint_reports_unresolved_without_trusted_candidates() {
    let mut context = pair_context();
    context.runtime_target = None;
    context.conversation_selection = None;
    context.workspace_roots.clear();

    let payload = infer_work_surface_hint(&context, RuntimeContextLabel::ArmatureConversation);
    assert_eq!(payload["work_surface"]["status"], json!("unresolved"));
    assert_eq!(payload["work_surface"]["confidence"], json!("none"));
    assert_eq!(
        payload["work_surface"]["needs_user_confirmation"],
        json!(false)
    );
    assert_eq!(
        payload["work_surface"]["agent_guidance"]["may_state_assumption"],
        json!(false)
    );
    assert_eq!(payload["work_surface"]["reference_candidates"], json!([]));
}

#[test]
fn accepts_only_resolved_or_confirmed_typed_session_anchors() {
    let resolved = json!({
        "work_surface_anchor": {
            "surface_id": "00000000-0000-0000-0000-000000000001",
            "status": "resolved"
        }
    });
    assert_eq!(
        WorkSurfaceSessionAnchor::from_adapter_environment(Some(&resolved))
            .expect("resolved anchor")
            .surface_id,
        uuid::Uuid::from_u128(1)
    );

    for invalid in [
        json!({"work_surface_anchor": {"surface_id": "00000000-0000-0000-0000-000000000001", "status": "candidate"}}),
        json!({"work_surface_anchor": {"surface_id": "not-a-uuid", "status": "confirmed"}}),
        json!({"work_surface_anchor": {"status": "confirmed"}}),
    ] {
        assert!(WorkSurfaceSessionAnchor::from_adapter_environment(Some(&invalid)).is_none());
    }
}
