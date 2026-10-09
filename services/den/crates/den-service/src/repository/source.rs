use crate::{
    bears::hats::{memory_binding, turn_binding::NativeTurnSource},
    conversation::viewer,
};
use den_core::{
    ids::{BearId, HatId, UserId},
    tools::{
        context::DenToolInvocationContext,
        repository::{RepositoryError, RepositorySurfaceId},
    },
    EffectivePolicy, Governance, TurnExecutionOrigin,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(super) struct Source {
    pub id: Uuid,
    pub hat: HatId,
    pub actor: UserId,
}

pub(super) async fn resolve(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
    governance: Governance,
    surface: RepositorySurfaceId,
) -> Result<Source, RepositoryError> {
    origin
        .require_ordinary_session()
        .map_err(|_| RepositoryError::NotAuthorized)?;
    if matches!(governance, Governance::Frozen | Governance::Observational)
        || context.profile
            != Some(EffectivePolicy::compile_for_origin(origin, governance).context_label)
        || context
            .client_session_id
            .as_deref()
            .is_some_and(|id| id != context.session_id)
    {
        return Err(RepositoryError::NotAuthorized);
    }
    let bear = BearId::new(context.bear_id);
    let actor = UserId::new(context.user_id);
    let (id, hat, native) = if matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_)) {
        let run = context.work_run_id.ok_or(RepositoryError::NotAuthorized)?;
        let row = sqlx::query!(
            "SELECT r.id, j.hat_id FROM bear_work_runs r
             JOIN bear_jobs j ON j.id = r.job_id AND j.bear_id = r.bear_id
             JOIN bear_job_runs job_run ON job_run.id = r.job_run_id AND job_run.job_id = j.id
             JOIN user_bear ub ON ub.bear_id = j.bear_id AND ub.user_id = j.created_by_user_id
             WHERE r.id = $1 AND r.bear_id = $2 AND r.bearwire_session_id = $3
               AND r.state IN ('claimed', 'provisioning', 'running', 'reporting') AND NOT r.cancel_requested
               AND r.lease_expires_at > now()
               AND j.created_by_user_id = $4 AND j.current_run_id = r.job_run_id AND j.lifecycle_intent IS NULL
               AND EXISTS (SELECT 1 FROM job_work_surface_assignments a WHERE a.job_id = j.id AND a.work_surface_id = $5)",
            run, context.bear_id, &context.session_id, actor.get(), surface.0,
        ).fetch_optional(pool).await.map_err(|_| RepositoryError::PolicyUnavailable)?.ok_or(RepositoryError::NotAuthorized)?;
        memory_binding::for_work_run(pool, bear, run)
            .await
            .map_err(|_| RepositoryError::NotAuthorized)?;
        (
            row.id,
            row.hat_id.ok_or(RepositoryError::NotAuthorized)?.into(),
            NativeTurnSource::WorkRun(run),
        )
    } else {
        if context.work_run_id.is_some() {
            return Err(RepositoryError::NotAuthorized);
        }
        viewer::require_ordinary_tool_source(pool, bear, actor, &context.conversation_id)
            .await
            .map_err(|_| RepositoryError::NotAuthorized)?;
        let row = sqlx::query!(
            "SELECT c.id, c.hat_id FROM conversations c JOIN user_bear ub ON ub.bear_id = c.bear_id AND ub.user_id = c.created_by_user_id
             WHERE c.bear_id = $1 AND c.external_conversation_id = $2 AND c.created_by_user_id = $3 AND c.status = 'active'
               AND NOT EXISTS(SELECT 1 FROM bear_work_runs r WHERE r.bearwire_session_id = $4 AND r.state IN ('claimed', 'provisioning', 'running', 'reporting'))",
            context.bear_id, &context.conversation_id, actor.get(), &context.session_id,
        ).fetch_optional(pool).await.map_err(|_| RepositoryError::PolicyUnavailable)?.ok_or(RepositoryError::NotAuthorized)?;
        if matches!(origin, TurnExecutionOrigin::ArmatureConversation(_)) {
            let valid = sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM client_sessions s WHERE s.bear_id = $1 AND s.user_id = $2
                 AND s.client_session_id = $3 AND s.closed_at IS NULL AND s.archived_at IS NULL
                 AND coalesce(s.resolved_conversation_id, s.conversation_id) = $4) AS \"valid!\"",
                context.bear_id, actor.get(), &context.session_id, &context.conversation_id,
            ).fetch_one(pool).await.map_err(|_| RepositoryError::PolicyUnavailable)?;
            if !valid {
                return Err(RepositoryError::NotAuthorized);
            }
        }
        (
            row.id,
            row.hat_id.ok_or(RepositoryError::NotAuthorized)?.into(),
            NativeTurnSource::Conversation(row.id),
        )
    };
    if native.binding_id(bear) != context.binding_id {
        return Err(RepositoryError::NotAuthorized);
    }
    Ok(Source { id, hat, actor })
}
