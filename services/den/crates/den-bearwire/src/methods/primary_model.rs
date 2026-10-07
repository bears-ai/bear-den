//! Shared model resolution for canonical BearWire turn sources.

use den_core::{ids::BearId, DenError};
use den_service::{
    bears::{
        hats::{memory_binding, turn_binding::NativeTurnSource},
        model_configurations::{self, ResolvedPrimaryModel},
    },
    model_selection,
};
use sqlx::PgPool;

pub(super) async fn resolve_for_source(
    pool: &PgPool,
    bear_id: BearId,
    source: NativeTurnSource,
    deployment_default: &str,
) -> Result<ResolvedPrimaryModel, DenError> {
    match source {
        NativeTurnSource::Conversation(id) => {
            model_selection::resolve_conversation_primary_model(
                pool,
                bear_id,
                id,
                deployment_default,
            )
            .await
        }
        NativeTurnSource::WorkRun(id) => {
            // A Work transcript's hat and selector are not its model authority.
            let memory_binding::ResolvedMemoryBinding::Bound(grant) =
                memory_binding::for_work_run(pool, bear_id, id).await?;
            model_configurations::resolve_primary(
                pool,
                bear_id,
                grant.hat_id(),
                None,
                deployment_default,
            )
            .await
        }
    }
}
