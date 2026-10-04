//! Resolve ordinary run authority before creating conversation/session/run state.

use den_core::{
    ids::{BearId, UserId},
    ArmatureAvailability, TurnExecutionOrigin,
};
use den_http::errors::CustomError;
use den_service::{
    bears::hats::{self, memory_binding, turn_binding::NativeTurnSource},
    conversation::{
        persistence::{self, ConversationRecord},
        viewer::{require_ordinary_tool_source, ConversationViewer},
    },
};
use sqlx::PgPool;

use super::super::conversation::authorize_or_create_conversation;

pub(super) struct AdmittedRunSource {
    pub conversation: ConversationRecord,
    pub origin: TurnExecutionOrigin,
    pub turn_source: NativeTurnSource,
}

pub(super) async fn admit(
    pool: &PgPool,
    viewer: &ConversationViewer,
    bear: BearId,
    user: UserId,
    session_id: &str,
    external_id: &str,
) -> Result<AdmittedRunSource, CustomError> {
    let work = den_docket::work_runs::get_live_work_run_by_session(pool, session_id).await?;
    let existing_source =
        persistence::get_conversation_for_external_id(pool, bear.as_uuid(), external_id).await?;
    let initial_hat = if let Some(run) = &work {
        if run.bear_id != bear.as_uuid() || run.cancel_requested {
            return Err(CustomError::Authorization(
                "Work run is not active for this Bear".into(),
            ));
        }
        memory_binding::for_work_run(pool, bear, run.id).await?;
        None
    } else if existing_source.is_some() {
        require_ordinary_tool_source(pool, bear, user, external_id).await?;
        None
    } else {
        // Only an explicitly configured real IDE default can admit a new source.
        // Existing unbound history is never promoted by this path.
        let hat = hats::ide_default_hat(pool, bear)
            .await?
            .ok_or_else(memory_binding::missing_binding)?;
        hats::manage::get_hat(pool, bear, hat).await?;
        Some(hat)
    };
    // A pending selection is not a transcript target. Allocate the durable ID
    // only after admission, before binding a single canonical memory source.
    let durable_id = if existing_source.is_none() && external_id.starts_with("new-") {
        format!("den-conv-{}", uuid::Uuid::new_v4().simple())
    } else {
        external_id.to_string()
    };
    let conversation =
        authorize_or_create_conversation(viewer, pool, bear.as_uuid(), user.get(), &durable_id)
            .await?;
    if let Some(hat) = initial_hat {
        hats::bindings::bind_conversation_hat(pool, bear, conversation.id, hat).await?;
    }
    let (origin, turn_source) = if let Some(run) = work {
        memory_binding::for_work_run(pool, bear, run.id).await?;
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            NativeTurnSource::WorkRun(run.id),
        )
    } else {
        require_ordinary_tool_source(pool, bear, user, &durable_id).await?;
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            NativeTurnSource::Conversation(conversation.id),
        )
    };
    Ok(AdmittedRunSource {
        conversation,
        origin,
        turn_source,
    })
}
