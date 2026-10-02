use super::*;
use crate::{
    tools::{
        constants::{DEN_RUN_WRITE_RESULT, DEN_WEB_FETCH},
        context::DenToolInvocationContext,
    },
    ArmatureAvailability,
};
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
use uuid::Uuid;

fn immediate<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fake directory unexpectedly yielded a pending future"),
    }
}

struct FakeDirectory {
    member: bool,
    registered: Option<BearProfile>,
}

impl BearDirectory for FakeDirectory {
    async fn user_may_use_bear(&self, _: i32, _: Uuid) -> Result<bool, DenError> {
        Ok(self.member)
    }

    async fn registered_profile(&self, _: Uuid, _: &str) -> Result<Option<BearProfile>, DenError> {
        Ok(self.registered)
    }

    async fn bear_self(&self, _: Uuid) -> Result<Option<BearRecord>, DenError> {
        unreachable!()
    }

    async fn member_count(&self, _: Uuid) -> Result<i64, DenError> {
        unreachable!()
    }

    async fn members(&self, _: Uuid) -> Result<Vec<BearMemberRecord>, DenError> {
        unreachable!()
    }

    async fn current_user(&self, _: i32) -> Result<CurrentUser, DenError> {
        unreachable!()
    }
}

fn context(profile: BearProfile) -> DenToolInvocationContext {
    serde_json::from_value(serde_json::json!({
        "bear_id": Uuid::nil(),
        "bear_slug": "test",
        "binding_id": "registered-native-binding",
        "profile": profile,
        "user_id": 7,
        "conversation_id": "conversation",
        "session_id": "session",
        "request_id": null
    }))
    .unwrap()
}

#[test]
fn native_ordinary_authorizer_uses_origin_without_profile_registration() {
    let pair = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let work = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let directory = FakeDirectory {
        member: true,
        registered: None,
    };
    assert_eq!(
        immediate(authorize_context_for_origin(
            &directory,
            &context(BearProfile::Pair),
            pair
        ))
        .unwrap(),
        BearProfile::Pair
    );
    assert_eq!(
        immediate(authorize_context_for_origin(
            &directory,
            &context(BearProfile::Work),
            work,
        ))
        .unwrap(),
        BearProfile::Work,
    );
    assert!(authorize_tool_for_origin(DEN_WEB_FETCH, pair).is_ok());
    assert!(matches!(
        authorize_tool_for_origin(DEN_RUN_WRITE_RESULT, pair),
        Err(DenError::Authorization(_))
    ));
    assert!(matches!(
        authorize_tool_for_origin(DEN_WEB_FETCH, work),
        Err(DenError::Authorization(_))
    ));
    for forged in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::InternalCuration,
        work,
    ] {
        assert!(matches!(
            immediate(authorize_context_for_origin(
                &directory,
                &context(BearProfile::Pair),
                forged
            )),
            Err(DenError::Authorization(_))
        ));
    }
    assert!(matches!(
        immediate(authorize_context_for_origin(
            &directory,
            &context(BearProfile::Curate),
            pair
        )),
        Err(DenError::Authorization(_))
    ));
    let departed = FakeDirectory {
        member: false,
        registered: None,
    };
    assert!(matches!(
        immediate(authorize_context_for_origin(
            &departed,
            &context(BearProfile::Pair),
            pair
        )),
        Err(DenError::Authorization(_))
    ));
    for (profile, origin) in [
        (BearProfile::Curate, TurnExecutionOrigin::InternalCuration),
        (BearProfile::Watch, TurnExecutionOrigin::InboundObservation),
    ] {
        assert!(matches!(
            immediate(authorize_context_for_origin(
                &directory,
                &context(profile),
                origin
            )),
            Err(DenError::Authorization(_)),
        ));
        let registered = FakeDirectory {
            member: true,
            registered: Some(profile),
        };
        assert_eq!(
            immediate(authorize_context_for_origin(
                &registered,
                &context(profile),
                origin
            ))
            .unwrap(),
            profile,
        );
    }
}

#[test]
fn native_capability_catalog_filters_by_origin_and_armature_availability() {
    use crate::tools::capability_catalog::SessionCapabilityDescriptor;

    let mut context = context(BearProfile::Pair);
    context
        .session_capabilities
        .push(SessionCapabilityDescriptor {
            instance_id: "session:mcp__filesystem__read".to_string(),
            name: "mcp__filesystem__read".to_string(),
            summary: "Read files through a connected provider".to_string(),
            kind: "tool".to_string(),
            provider: "mcp".to_string(),
            execution_locality: "connected MCP provider".to_string(),
            authority: "current client connection and turn policy".to_string(),
            surface: "workspace roots".to_string(),
            availability: "available".to_string(),
            tags: vec!["session-bound".to_string()],
        });
    let channel = TurnExecutionOrigin::ChannelConversation;
    let browser = TurnExecutionOrigin::BrowserTaskSession;
    let connected = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let disconnected = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent);
    let tool_ref = "capability-instance:session:mcp__filesystem__read";
    for origin in [channel, browser, disconnected] {
        let entries = capability_entries_for_origin(origin, &context);
        assert!(
            !entries.iter().any(|entry| entry.r#ref == tool_ref),
            "{origin:?}"
        );
        assert!(matches!(
            capability_describe_for_origin(serde_json::json!({"ref": tool_ref}), origin, &context),
            Err(DenError::NotFound(_))
        ));
    }
    let pair = capability_entries_for_origin(connected, &context);
    assert!(pair.iter().any(|entry| entry.r#ref == tool_ref));
    assert!(pair
        .iter()
        .any(|entry| entry.r#ref == format!("tool:{DEN_WEB_FETCH}")));
    let channel_entries = capability_entries_for_origin(channel, &context);
    assert!(!channel_entries
        .iter()
        .any(|entry| entry.r#ref == format!("tool:{DEN_WEB_FETCH}")));
    let roster = list_capabilities_for_origin(&context, channel);
    assert!(!roster["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["name"] == DEN_WEB_FETCH));
}
