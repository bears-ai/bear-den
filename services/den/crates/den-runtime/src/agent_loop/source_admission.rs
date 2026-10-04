//! Effect-time admission of ordinary inference and continuation sources.

use den_core::{
    execution_context::RuntimeContextLabel,
    ids::{BearId, UserId},
    DenError, TurnExecutionOrigin,
};
use den_docket::{work_runs, DocketService, PgDocketService};
use den_service::{
    bears::hats::{memory_binding, turn_binding::NativeTurnSource},
    conversation::{persistence, viewer::ConversationViewer},
};
use sqlx::PgPool;
use uuid::Uuid;

use super::AgentLoopSession;

#[derive(Clone, Copy)]
pub struct OrdinarySessionSource<'a> {
    pub bear_id: Uuid,
    pub user_id: Option<i32>,
    pub origin: TurnExecutionOrigin,
    pub profile: RuntimeContextLabel,
    pub conversation_id: &'a str,
    pub client_session_id: &'a str,
    pub work_run_id: Option<Uuid>,
}

impl<'a> From<&'a AgentLoopSession> for OrdinarySessionSource<'a> {
    fn from(session: &'a AgentLoopSession) -> Self {
        Self {
            bear_id: session.bear_id,
            user_id: session.user_id,
            origin: session.origin,
            profile: session.profile,
            conversation_id: &session.conversation_id,
            client_session_id: &session.client_session_id,
            work_run_id: session.work_run_id,
        }
    }
}

pub(crate) fn require_same_work_run(bound: Option<Uuid>, live: Uuid) -> Result<(), DenError> {
    if bound != Some(live) {
        return Err(DenError::Authorization(
            "Work continuation's live Job run differs from its verified session binding".into(),
        ));
    }
    Ok(())
}

/// Resolve only from current canonical state, never from profile registrations or
/// caller-supplied hat/origin text. A cached prompt is not an authority grant.
pub async fn require_ordinary_session_source(
    pool: &PgPool,
    session: OrdinarySessionSource<'_>,
) -> Result<NativeTurnSource, DenError> {
    session.origin.require_ordinary_session()?;
    if den_core::EffectivePolicy::compile_for_origin(
        session.origin,
        den_core::Governance::Interactive,
    )
    .context_label
        != session.profile
    {
        return Err(DenError::Authorization(
            "ordinary session origin disagrees with its profile".into(),
        ));
    }
    let bear_id = BearId::new(session.bear_id);
    match session.origin {
        TurnExecutionOrigin::AuthorizedWorkRun(_) => {
            let original = session.work_run_id.ok_or_else(|| {
                DenError::Authorization("Work continuation has no originating Job run".into())
            })?;
            let live = work_runs::get_live_work_run_by_session(pool, session.client_session_id)
                .await?
                .filter(|run| run.bear_id == session.bear_id && !run.cancel_requested)
                .ok_or_else(|| {
                    DenError::Authorization("Work continuation lost its live Job run".into())
                })?;
            require_same_work_run(Some(original), live.id)?;
            let job = PgDocketService::from_pool(pool)
                .get_job(session.bear_id, live.job_id)
                .await?
                .ok_or_else(|| {
                    DenError::Authorization("Work source lost its canonical Job".into())
                })?;
            if session
                .user_id
                .is_some_and(|user_id| user_id != job.job.created_by_user_id)
            {
                return Err(DenError::Authorization(
                    "Work source actor does not match its canonical Job creator".into(),
                ));
            }
            if !den_service::bears::db::user_may_use_bear(
                pool,
                job.job.created_by_user_id,
                session.bear_id,
            )
            .await?
            {
                return Err(DenError::Authorization(
                    "Work source's canonical Job creator lost Bear access".into(),
                ));
            }
            memory_binding::for_work_run(pool, bear_id, live.id).await?;
            Ok(NativeTurnSource::WorkRun(live.id))
        }
        TurnExecutionOrigin::ChannelConversation
        | TurnExecutionOrigin::BrowserTaskSession
        | TurnExecutionOrigin::ArmatureConversation(_) => {
            let user_id = session.user_id.ok_or_else(|| {
                DenError::Authorization("conversation continuation has no human owner".into())
            })?;
            if matches!(session.origin, TurnExecutionOrigin::ArmatureConversation(_)) {
                let active = den_service::client_sessions::find_for_user_bear_session_id(
                    pool,
                    user_id,
                    session.bear_id,
                    session.client_session_id,
                )
                .await?
                .ok_or_else(|| {
                    DenError::Authorization("continuation client session is missing".into())
                })?;
                if active.closed_at.is_some() || active.archived_at.is_some() {
                    return Err(DenError::Authorization(
                        "continuation editor session is closed".into(),
                    ));
                }
                if active
                    .resolved_conversation_id
                    .as_deref()
                    .unwrap_or(&active.conversation_id)
                    != session.conversation_id
                {
                    return Err(DenError::Authorization(
                        "continuation client session changed conversation".into(),
                    ));
                }
            }
            if session.work_run_id.is_some()
                || work_runs::get_live_work_run_by_session(pool, session.client_session_id)
                    .await?
                    .is_some()
            {
                return Err(DenError::Authorization(
                    "conversation continuation is bound to Work".into(),
                ));
            }
            let viewer = ConversationViewer::resolve(pool, bear_id, UserId::new(user_id))
                .await?
                .ok_or_else(|| {
                    DenError::Authorization("continuation actor lost Bear access".into())
                })?;
            let conversation = persistence::get_conversation_for_external_id(
                pool,
                session.bear_id,
                session.conversation_id,
            )
            .await?
            .ok_or_else(|| {
                DenError::Authorization("continuation conversation is missing".into())
            })?;
            if !viewer.may_read_own_source(pool, conversation.id).await? {
                return Err(DenError::Authorization(
                    "continuation actor does not own its active conversation".into(),
                ));
            }
            memory_binding::for_conversation(pool, bear_id, conversation.id).await?;
            Ok(NativeTurnSource::Conversation(conversation.id))
        }
        TurnExecutionOrigin::InternalCuration | TurnExecutionOrigin::InboundObservation => {
            Err(DenError::Authorization(
                "system execution cannot continue through a conversational session".into(),
            ))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
