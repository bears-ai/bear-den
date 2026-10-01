//! Concrete native-runtime tool capability composed with the canonical Den state.

use async_trait::async_trait;
use serde_json::Value;

use den_core::tools::{
    constants::{DEN_TASK_FOCUS, DEN_TASK_FOCUS_PROVIDER},
    context::DenToolInvocationContext,
    descriptor::builtin_den_tool_descriptor_for_provider_name,
};
use den_core::{ids::BearId, DenError, EffectivePolicy, TurnExecutionOrigin};
use den_runtime::native_runtime::{RuntimeToolInvocation, RuntimeToolInvoker};
use den_service::{bears::hats::memory_binding, DenState};
use sqlx::PgPool;

use crate::core::tools::{context::DenToolContext, session::invoke_den_tool};
use crate::errors::CustomError;

pub struct DenRuntimeToolInvoker {
    state: DenState,
}

impl DenRuntimeToolInvoker {
    pub fn new(state: DenState) -> Self {
        Self { state }
    }
}

fn require_origin_policy_and_descriptor(
    context: &DenToolInvocationContext,
    effective_policy: &EffectivePolicy,
    origin: TurnExecutionOrigin,
    tool_name: &str,
) -> Result<(), DenError> {
    if *effective_policy != EffectivePolicy::compile_for_origin(origin, effective_policy.governance)
    {
        return Err(DenError::Authorization(
            "Den tool policy does not match its verified execution origin".into(),
        ));
    }
    if context.profile != Some(effective_policy.trust_profile) {
        return Err(DenError::Authorization(
            "Den tool context profile does not match the verified execution origin".into(),
        ));
    }
    if matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_)) != context.work_run_id.is_some()
    {
        return Err(DenError::Authorization(
            "Den tool Work-run binding does not match the verified execution origin".into(),
        ));
    }
    let descriptor = builtin_den_tool_descriptor_for_provider_name(tool_name)
        .ok_or_else(|| DenError::NotFound(format!("unknown Den tool: {tool_name}")))?;
    if !descriptor.allows_origin(origin) {
        return Err(DenError::Authorization(format!(
            "Den tool `{}` is unavailable to this verified execution origin",
            descriptor.name,
        )));
    }
    Ok(())
}

async fn require_live_work_tool_source(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    origin: TurnExecutionOrigin,
) -> Result<(), DenError> {
    if !matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_)) {
        return Ok(());
    }
    let work_run_id = context.work_run_id.ok_or_else(|| {
        DenError::Authorization("Work tool invocation is missing its Work-run binding".into())
    })?;
    let run = den_docket::work_runs::get_live_work_run_by_session(pool, &context.session_id)
        .await?
        .filter(|run| {
            run.id == work_run_id && run.bear_id == context.bear_id && !run.cancel_requested
        })
        .ok_or_else(|| {
            DenError::Authorization(
                "Work tool invocation is not bound to its live, uncancelled Job run".into(),
            )
        })?;
    memory_binding::for_work_run(pool, BearId::new(context.bear_id), run.id).await?;
    Ok(())
}

#[cfg(test)]
#[path = "runtime_invoker/tests.rs"]
mod tests;

#[async_trait]
impl RuntimeToolInvoker for DenRuntimeToolInvoker {
    async fn invoke(&self, invocation: RuntimeToolInvocation) -> Result<Value, DenError> {
        let RuntimeToolInvocation {
            tool_name,
            arguments,
            context,
            origin,
            effective_policy,
            origin_run_id,
            tool_call_id,
        } = invocation;
        // The runtime's verified origin is the authority. A forged/supplied
        // compatibility profile or binding cannot turn a Pair/Channel run
        // into an internal Curate/Work Den tool invocation.
        require_origin_policy_and_descriptor(&context, &effective_policy, origin, &tool_name)?;
        require_live_work_tool_source(&self.state.sqlx_pool, &context, origin).await?;
        if matches!(tool_name.as_str(), DEN_TASK_FOCUS | DEN_TASK_FOCUS_PROVIDER) {
            effective_policy
                .capabilities
                .require(den_core::BearCapability::ExecuteFocusedTask)?;
            let tool_context = DenToolContext::new(
                &self.state.sqlx_pool,
                self.state.config.as_ref(),
                &self.state.memory_stores,
            );
            den_core::tools::dispatch::authorize_den_tool(&tool_context, DEN_TASK_FOCUS, &context)
                .await?;
            let origin_run_id = origin_run_id.ok_or_else(|| {
                DenError::ValidationError(
                    "focus_current_task requires an active Pair run origin".to_string(),
                )
            })?;
            let bear = den_service::bears::db::get_bear(&self.state.sqlx_pool, context.bear_id)
                .await?
                .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;
            let session_id = context
                .client_session_id
                .as_deref()
                .unwrap_or(&context.session_id);
            let execution = den_bearwire::acquire_selected_task_for_run(
                &self.state,
                context.user_id,
                bear,
                session_id,
                &origin_run_id,
                &tool_call_id,
                &effective_policy.capabilities,
            )
            .await
            .map_err(CustomError::into_den)?;
            return serde_json::to_value(execution).map_err(|error| {
                DenError::System(format!("serialize Pair focus result: {error}"))
            });
        }

        invoke_den_tool(
            &self.state.sqlx_pool,
            self.state.config.as_ref(),
            &self.state.memory_stores,
            &tool_name,
            arguments,
            context,
        )
        .await
        .map_err(CustomError::into_den)
    }
}
