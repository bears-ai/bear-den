//! Per-run Den ceiling for restricted Work sandboxes. The provider applies
//! its own current root ceiling again; this value cannot grant a destination.

use den_core::{ids::BearId, DenError};

#[cfg(test)]
#[path = "network_policy/tests.rs"]
mod tests;
use den_docket::work_runs::WorkRunDispatchContext;
use den_sandbox::protocol::{AllowedOutboundHosts, HealthResponse};
use den_service::{
    bears::hats::{access, memory_binding::ResolvedMemoryBinding},
    work_surfaces,
};
use sqlx::PgPool;

pub(super) fn require_provider_run_ceiling(health: &HealthResponse) -> Result<(), DenError> {
    if !health.ok || !health.backend_available || !health.run_outbound_ceiling_supported {
        return Err(DenError::Authorization(
            "the sandbox provider must be healthy and enforce run-scoped outbound ceilings for hat-bound Work".into(),
        ));
    }
    Ok(())
}

pub(super) async fn for_run(
    pool: &PgPool,
    bear_id: BearId,
    binding: ResolvedMemoryBinding,
    context: &WorkRunDispatchContext,
    root_name: &str,
) -> Result<Option<AllowedOutboundHosts>, DenError> {
    let ResolvedMemoryBinding::Bound(grant) = binding else {
        return Ok(None);
    };
    let hat_id = grant
        .hat_id()
        .ok_or_else(|| DenError::Authorization("bound Work run has no hat".into()))?;
    let surface_id = context.work_surface_id.ok_or_else(|| {
        DenError::Authorization("bound Work run has no assigned git work surface".into())
    })?;
    let surface = work_surfaces::surface_by_id(pool, surface_id)
        .await?
        .ok_or_else(|| DenError::Authorization("Job work surface no longer exists".into()))?;
    if surface.name != root_name {
        return Err(DenError::Authorization(
            "Job surface does not match the provisioned root".into(),
        ));
    }
    let ceiling = AllowedOutboundHosts::new(surface.allowed_outbound_hosts)
        .map_err(|err| DenError::System(format!("invalid saved work-surface hosts: {err}")))?;
    let permitted =
        access::intersect_surface_outbound_hosts(pool, bear_id, hat_id, &ceiling).await?;
    Ok(Some(permitted))
}
