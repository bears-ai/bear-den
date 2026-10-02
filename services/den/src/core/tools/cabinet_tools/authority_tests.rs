use super::*;
use den_core::{ArmatureAvailability, Governance, TurnExecutionOrigin};

#[test]
fn cabinet_write_policy_uses_descriptor_audience_not_an_audit_profile() {
    for tool in [
        DEN_CABINET_CREATE,
        DEN_CABINET_UPDATE,
        DEN_CABINET_SOURCE_LINK,
        DEN_CABINET_LIFECYCLE,
    ] {
        for origin in [
            TurnExecutionOrigin::ChannelConversation,
            TurnExecutionOrigin::BrowserTaskSession,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            TurnExecutionOrigin::InternalCuration,
            TurnExecutionOrigin::InboundObservation,
        ] {
            let descriptor = builtin_den_tool_descriptor_for_provider_name(tool).unwrap();
            let authority = CabinetToolAuthority {
                origin,
                governance: Governance::Interactive,
            };
            assert_eq!(
                require_write_authority(tool, authority).is_ok(),
                descriptor.allows_origin(origin),
                "{tool}: {origin:?}",
            );
            for governance in [Governance::Observational, Governance::Frozen] {
                assert!(
                    require_write_authority(tool, CabinetToolAuthority { origin, governance },)
                        .is_err()
                );
            }
        }
    }
}

#[tokio::test]
async fn work_origin_cannot_write_cabinet_with_a_claimed_pair_profile() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": uuid::Uuid::new_v4(),
        "bear_slug": "cabinet-authority",
        "binding_id": "untrusted-pair",
        "profile": "pair",
        "user_id": 1,
        "conversation_id": "untrusted-conversation",
        "session_id": "untrusted-session",
        "client_session_id": "untrusted-client-session",
        "channel": {}
    }))
    .unwrap();
    let result = invoke_cabinet_tool(
        &pool,
        DEN_CABINET_CREATE,
        json!({"title": "Untrusted", "content": "Untrusted"}),
        &context,
        CabinetToolAuthority {
            origin: TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            governance: Governance::Interactive,
        },
    )
    .await;
    assert!(matches!(result, Err(CustomError::Authorization(_))));
}
