//! Narrow, Den-owned persistent permission for an ACP web-fetch obligation.
//! The caller only invokes persistence after winning the result/continuation
//! claim; duplicate or late client responses cannot update a hat policy.

use bearwire_protocol::methods::PermissionDecisionInput;
use den_core::{
    ids::{BearId, UserId},
    tools::{constants::DEN_WEB_FETCH, descriptor::builtin_den_tool_descriptor_for_provider_name},
};
use den_http::errors::CustomError;
use den_service::{bears::hats::access, conversation::persistence};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(test)]
#[path = "hat_web_permission/tests.rs"]
mod tests;

pub(super) fn validate_decision(
    decision: PermissionDecisionInput,
    obligation_payload: &Value,
) -> Result<(), CustomError> {
    let tool_name = obligation_payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_web_fetch = builtin_den_tool_descriptor_for_provider_name(tool_name)
        .is_some_and(|descriptor| descriptor.name == DEN_WEB_FETCH);
    if decision == PermissionDecisionInput::AllowHatHost && !is_web_fetch {
        return Err(CustomError::ValidationError(
            "hat-host approval requires a Den-owned web_fetch obligation".into(),
        ));
    }
    if is_web_fetch
        && matches!(
            decision,
            PermissionDecisionInput::AllowSiteAccount | PermissionDecisionInput::AllowHost
        )
    {
        return Err(CustomError::ValidationError(
            "persistent web_fetch approval must be managed as a Den-owned hat policy; choose Just this time".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_destination(obligation_payload: &Value) -> Result<(), CustomError> {
    let raw_url = obligation_payload
        .pointer("/arguments/url")
        .and_then(Value::as_str)
        .ok_or_else(|| CustomError::ValidationError("web_fetch approval has no URL".into()))?;
    access::HttpsHost::from_https_url(raw_url)?;
    Ok(())
}

pub(super) async fn persist(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    conversation_id: &str,
    session_id: &str,
    obligation_payload: &Value,
) -> Result<(), CustomError> {
    validate_decision(PermissionDecisionInput::AllowHatHost, obligation_payload)?;
    validate_destination(obligation_payload)?;
    if den_docket::work_runs::get_work_run_by_session(pool, session_id)
        .await?
        .is_some()
    {
        return Err(CustomError::ValidationError(
            "a Work run cannot persist interactive hat web-fetch permissions".into(),
        ));
    }
    let raw_url = obligation_payload
        .pointer("/arguments/url")
        .and_then(Value::as_str)
        .ok_or_else(|| CustomError::ValidationError("web_fetch approval has no URL".into()))?;
    let conversation =
        persistence::get_conversation_for_external_id(pool, bear_id, conversation_id)
            .await?
            .ok_or_else(|| {
                CustomError::ValidationError("hat approval has no canonical conversation".into())
            })?;
    access::grant_web_fetch_host_for_own_conversation(
        pool,
        BearId::new(bear_id),
        conversation.id,
        UserId::new(user_id),
        raw_url,
        true,
    )
    .await?;
    Ok(())
}
