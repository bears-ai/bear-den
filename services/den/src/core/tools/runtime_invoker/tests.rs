use super::*;
use den_core::{
    tools::{arguments::DenToolChannelContext, context::DenToolInvocationContext},
    ArmatureAvailability, BearProfile, Governance, TurnExecutionOrigin,
};
use uuid::Uuid;

fn context(profile: BearProfile) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id: Uuid::nil(),
        bear_slug: "test".into(),
        binding_id: "test".into(),
        profile: Some(profile),
        user_id: 1,
        username: None,
        membership_role: None,
        conversation_id: "conv".into(),
        session_id: "session".into(),
        work_run_id: None,
        client_session_id: None,
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: None,
        channel: DenToolChannelContext::default(),
    }
}

#[test]
fn direct_invoker_cannot_widen_a_pair_origin_with_a_curate_or_work_profile() {
    let pair = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        Governance::Interactive,
    );
    assert!(require_origin_profile(&context(BearProfile::Pair), &pair).is_ok());
    for forged in [
        BearProfile::Curate,
        BearProfile::Work,
        BearProfile::Watch,
        BearProfile::Chat,
    ] {
        assert!(
            matches!(
                require_origin_profile(&context(forged), &pair),
                Err(DenError::Authorization(_))
            ),
            "{forged:?}"
        );
    }
    let work = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        Governance::Interactive,
    );
    assert!(require_origin_profile(&context(BearProfile::Work), &work).is_ok());
    assert!(require_origin_profile(&context(BearProfile::Pair), &work).is_err());
}
