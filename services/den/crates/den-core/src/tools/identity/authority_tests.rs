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
fn native_core_authorizer_uses_origin_for_tools_and_registration_only_as_a_check() {
    let pair = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let work = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    let directory = FakeDirectory {
        member: true,
        registered: Some(BearProfile::Pair),
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
        registered: Some(BearProfile::Pair),
    };
    assert!(matches!(
        immediate(authorize_context_for_origin(
            &departed,
            &context(BearProfile::Pair),
            pair
        )),
        Err(DenError::Authorization(_))
    ));
}
