//! Per-run Den ceiling for restricted Work sandboxes. The provider applies
//! its own current root ceiling again; this value cannot grant a destination.

use den_core::{ids::BearId, DenError};

#[cfg(test)]
#[path = "network_policy/tests.rs"]
mod tests;
use den_docket::work_runs::{WorkRunDispatchContext, WorkRunRow, WorkRunState};
use den_sandbox::protocol::{AllowedOutboundHosts, HealthResponse};
use den_service::{
    bears::hats::{
        access::{self, HttpsHost},
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
    #[serde(default)]
    dynamic_authorization: bool,
}

pub(super) fn snapshot(hosts: &AllowedOutboundHosts) -> Value {
    serde_json::to_value(RunEgressSnapshot {
        hosts: hosts.clone(),
        dynamic_authorization: true,
    })
    .expect("validated outbound hosts serialize")
}

fn provisioned_snapshot(run: &WorkRunRow) -> Result<RunEgressSnapshot, DenError> {
    let value = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.get("hat_egress"))
        .ok_or_else(|| {
            DenError::Authorization(
                "hat-bound Work sandbox has no provisioned egress snapshot".into(),
            )
        })?;
    serde_json::from_value(value.clone())
        .map_err(|err| DenError::Authorization(format!("invalid Work egress snapshot: {err}")))
}

/// An individual new relay connection may use only a host provisioned for this
/// sandbox and still present in the current hat ∩ assigned-surface policy.
pub(super) async fn host_allowed_for_live_run(
    pool: &PgPool,
    run: &WorkRunRow,
    host: &HttpsHost,
) -> Result<bool, DenError> {
    if !matches!(
        WorkRunState::parse(&run.state),
        Some(WorkRunState::Running | WorkRunState::Reporting)
    ) || run.cancel_requested
        || run.sandbox_id.is_none()
        || run.execution_target != den_docket::work_runs::WorkExecutionTarget::Sandbox.as_str()
    {
        return Ok(false);
    }
    let bear_id = BearId::new(run.bear_id);
    let binding = memory_binding::for_work_run(pool, bear_id, run.id).await?;
    if !matches!(binding, ResolvedMemoryBinding::Bound(_)) {
        return Ok(false);
    }
    let at_provision = provisioned_snapshot(run)?;
    if !at_provision.dynamic_authorization {
        return Ok(false);
    }
    if !at_provision
        .hosts
        .as_slice()
        .iter()
        .any(|name| name == host.as_str())
    {
        return Ok(false);
    }
    let context = den_docket::work_runs::get_work_run_dispatch_context(pool, run.id).await?;
    let root = context.work_surface_name.as_deref().ok_or_else(|| {
        DenError::Authorization("Work run no longer has an assigned surface".into())
    })?;
    let current = for_run(pool, bear_id, binding, &context, root)
        .await?
        .ok_or_else(|| DenError::Authorization("hat-bound Work lost its egress policy".into()))?;
    Ok(current.as_slice().iter().any(|name| name == host.as_str()))
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
    let at_provision = provisioned_snapshot(run)?;
    if !at_provision.dynamic_authorization {
        return Err(DenError::Authorization(
            "hat-bound Work sandbox predates Den-checked outbound relays".into(),
        ));
    }
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
    if !health.ok
        || !health.backend_available
        || !health.run_outbound_ceiling_supported
        || !health.dynamic_egress_supported
    {
        return Err(DenError::Authorization(
            "the sandbox provider must be healthy and enforce run-scoped ceilings and per-connection egress checks for hat-bound Work".into(),
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
