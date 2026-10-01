//! Per-run Den ceiling for restricted Work sandboxes. The provider applies
//! its own current root ceiling again; this value cannot grant a destination.

use den_core::{ids::BearId, DenError};

#[cfg(test)]
#[path = "network_policy/tests.rs"]
mod tests;
use den_docket::work_runs::{WorkRunDispatchContext, WorkRunRow};
use den_sandbox::protocol::{AllowedOutboundHosts, HealthResponse};
use den_service::{
    bears::hats::{
        access,
        memory_binding::{self, ResolvedMemoryBinding},
    },
    work_surfaces,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

#[derive(Serialize, Deserialize)]
struct RunEgressSnapshot {
    hosts: AllowedOutboundHosts,
}

pub(super) fn snapshot(hosts: &AllowedOutboundHosts) -> Value {
    serde_json::to_value(RunEgressSnapshot {
        hosts: hosts.clone(),
    })
    .expect("validated outbound hosts serialize")
}

/// Only an immutable audit snapshot, never another writable grant. A missing
/// snapshot on a hat-bound active run is an authorization failure: it may be a
/// sandbox provisioned before the per-run ceiling was deployed.
pub(super) async fn active_run_still_authorized(
    pool: &PgPool,
    run: &WorkRunRow,
) -> Result<bool, DenError> {
    let bear_id = BearId::new(run.bear_id);
    let binding = memory_binding::for_work_run(pool, bear_id, run.id).await?;
    if matches!(binding, ResolvedMemoryBinding::Legacy) {
        return Ok(true);
    }
    let value = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.get("hat_egress"))
        .ok_or_else(|| {
            DenError::Authorization(
                "hat-bound Work sandbox has no provisioned egress snapshot".into(),
            )
        })?;
    let at_provision: RunEgressSnapshot = serde_json::from_value(value.clone())
        .map_err(|err| DenError::Authorization(format!("invalid Work egress snapshot: {err}")))?;
    let context = den_docket::work_runs::get_work_run_dispatch_context(pool, run.id).await?;
    let root = context.work_surface_name.as_deref().ok_or_else(|| {
        DenError::Authorization("Work run no longer has an assigned surface".into())
    })?;
    let current = for_run(pool, bear_id, binding, &context, root)
        .await?
        .ok_or_else(|| DenError::Authorization("hat-bound Work lost its egress policy".into()))?;
    Ok(at_provision
        .hosts
        .as_slice()
        .iter()
        .all(|host| current.as_slice().contains(host)))
}

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
