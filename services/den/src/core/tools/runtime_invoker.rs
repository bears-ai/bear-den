//! Concrete native-runtime tool capability composed with the canonical Den state.

use async_trait::async_trait;
use serde_json::Value;

use den_core::tools::{
    constants::{DEN_TASK_FOCUS, DEN_TASK_FOCUS_PROVIDER},
    context::DenToolInvocationContext,
};
use den_core::{DenError, EffectivePolicy};
use den_runtime::native_runtime::{RuntimeToolInvocation, RuntimeToolInvoker};
use den_service::DenState;

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

fn require_origin_profile(
    context: &DenToolInvocationContext,
    effective_policy: &EffectivePolicy,
) -> Result<(), DenError> {
    if context.profile != Some(effective_policy.trust_profile) {
        return Err(DenError::Authorization(
            "Den tool context profile does not match the verified execution origin".into(),
        ));
    }
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
            effective_policy,
            origin_run_id,
            tool_call_id,
        } = invocation;
        // The runtime's verified origin is the authority. A forged/supplied
        // compatibility profile or binding cannot turn a Pair/Channel run
        // into an internal Curate/Work Den tool invocation.
        require_origin_profile(&context, &effective_policy)?;
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
