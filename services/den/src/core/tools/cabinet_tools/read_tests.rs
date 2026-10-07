use super::*;
use den_core::{
    tools::{
        constants::DEN_CABINET_READ, descriptor::builtin_den_tool_descriptor_for_provider_name,
    },
    ArmatureAvailability, Governance, TurnExecutionOrigin,
};

#[test]
fn page_and_attachment_arguments_use_distinct_typed_targets() {
    let page = CabinetItemRef::mint();
    let attachment = CabinetAttachmentRef::mint();
    let version = CabinetVersionRef::mint();
    assert!(matches!(
        parse_arguments::<Arguments>(json!({"cabinet_ref":page,"version_ref":version}))
            .unwrap()
            .target()
            .unwrap(),
        Target::Page { .. }
    ));
    assert!(matches!(
        parse_arguments::<Arguments>(
            json!({"cabinet_ref":page,"attachment_ref":attachment,"offset_chars":2,"limit_chars":3})
        )
        .unwrap()
        .target()
        .unwrap(),
        Target::Attachment { .. }
    ));
    for args in [
        json!({"cabinet_ref":page,"version_ref":version,"attachment_ref":attachment}),
        json!({"cabinet_ref":page,"offset_chars":0}),
        json!({"cabinet_ref":page,"attachment_ref":attachment,"limit_chars":0}),
        json!({"cabinet_ref":page,"attachment_ref":attachment,"limit_chars":24001}),
        json!({"cabinet_ref":page,"attachment_ref":attachment,"offset_chars":-1}),
        json!({"cabinet_ref":page,"attachment_ref":"../secret"}),
        json!({"cabinet_ref":page,"storage_key":"secret"}),
    ] {
        assert!(parse_arguments::<Arguments>(args)
            .and_then(Arguments::target)
            .is_err());
    }
}

#[test]
fn attachment_read_remains_descriptor_owned_and_den_hosted() {
    let descriptor = builtin_den_tool_descriptor_for_provider_name("cabinet_read").unwrap();
    assert_eq!(descriptor.name, DEN_CABINET_READ);
    assert_eq!(descriptor.execution_target, "den");
    assert_eq!(descriptor.permissions, &["cabinet.read"]);
    assert!(descriptor.input_schema["properties"]["attachment_ref"].is_object());
    assert_eq!(
        descriptor.input_schema["properties"]["limit_chars"]["maximum"],
        24000
    );
    assert!(descriptor
        .description
        .contains("Private files and files belonging to another Bear are omitted"));
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
    ] {
        assert!(descriptor.allows_origin(origin));
    }
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(!descriptor.allows_origin(origin));
    }
}

#[tokio::test]
async fn internal_origin_cannot_borrow_the_attachment_read_surface() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id":uuid::Uuid::new_v4(),"bear_slug":"read-authority","binding_id":"claimed-pair",
        "profile":"pair","user_id":1,"conversation_id":"claimed","session_id":"claimed","client_session_id":"claimed","channel":{}
    })).unwrap();
    let result = super::super::invoke_cabinet_tool(
        &pool,
        DEN_CABINET_READ,
        json!({"cabinet_ref":CabinetItemRef::mint(),"attachment_ref":CabinetAttachmentRef::mint()}),
        &context,
        super::super::CabinetToolAuthority {
            origin: TurnExecutionOrigin::InternalCuration,
            governance: Governance::Interactive,
        },
        None,
    )
    .await;
    assert!(matches!(result, Err(CustomError::Authorization(_))));
}
