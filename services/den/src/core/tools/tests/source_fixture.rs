use den_core::{tools::context::DenToolInvocationContext, DenError};
use den_service::{
    bears::hats::{self, turn_binding::NativeTurnSource},
    client_sessions::{self, UpsertClientSession},
    conversation::persistence,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Bind a positive tool fixture to an owned conversation, explicit hat, and client.
pub(crate) async fn admit_tool_source(
    pool: &PgPool,
    context: &mut DenToolInvocationContext,
) -> Result<Uuid, DenError> {
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        context.bear_id,
        Some(context.user_id),
        &context.conversation_id,
        Some(&context.session_id),
        None,
    )
    .await?;
    let hat = hats::create_hat(
        pool,
        context.bear_id.into(),
        context.user_id.into(),
        &format!("Tool fixture {}", context.user_id),
        "Exercise authorized tools",
    )
    .await?;
    hats::bindings::bind_conversation_hat(pool, context.bear_id.into(), conversation.id, hat.id)
        .await?;
    bind_tool_client(pool, context).await?;
    context.binding_id =
        NativeTurnSource::Conversation(conversation.id).binding_id(context.bear_id.into());
    Ok(conversation.id)
}

pub(crate) async fn bind_tool_client(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<(), DenError> {
    client_sessions::upsert_session(
        pool,
        UpsertClientSession {
            user_id: context.user_id,
            bear_id: context.bear_id,
            bear_slug: context.bear_slug.clone(),
            client_session_id: context.session_id.clone(),
            runtime_session_id: "native-fixture".into(),
            conversation_id: context.conversation_id.clone(),
            resolved_conversation_id: None,
            client: "bear-armature".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await?;
    Ok(())
}
