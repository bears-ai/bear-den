use super::*;
use den_core::{
    tools::{
        arguments::DenToolChannelContext,
        constants::{DEN_RUN_WRITE_RESULT, DEN_WEB_FETCH},
        context::DenToolInvocationContext,
    },
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
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let pair = EffectivePolicy::compile_for_origin(origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Pair),
        &pair,
        origin,
        DEN_WEB_FETCH
    )
    .is_ok());
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Pair),
        &pair,
        origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    for forged in [
        BearProfile::Curate,
        BearProfile::Work,
        BearProfile::Watch,
        BearProfile::Chat,
    ] {
        assert!(
            matches!(
                require_origin_policy_and_descriptor(
                    &context(forged),
                    &pair,
                    origin,
                    DEN_WEB_FETCH
                ),
                Err(DenError::Authorization(_))
            ),
            "{forged:?}"
        );
    }
    let work_origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let work = EffectivePolicy::compile_for_origin(work_origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Work),
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_ok());
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Pair),
        &work,
        work_origin,
        DEN_RUN_WRITE_RESULT
    )
    .is_err());
    assert!(
        require_origin_policy_and_descriptor(
            &context(BearProfile::Pair),
            &work,
            origin,
            DEN_WEB_FETCH
        )
        .is_err(),
        "a Work policy cannot claim an interactive origin"
    );
    let chat_origin = TurnExecutionOrigin::ChannelConversation;
    let chat = EffectivePolicy::compile_for_origin(chat_origin, Governance::Interactive);
    assert!(require_origin_policy_and_descriptor(
        &context(BearProfile::Chat),
        &chat,
        chat_origin,
        DEN_WEB_FETCH
    )
    .is_err());
}
