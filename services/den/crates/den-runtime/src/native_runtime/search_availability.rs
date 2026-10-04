//! Derive model-visible search availability from the same live hat grants
//! enforced at provider egress. This is a projection, never effect authority.

#[cfg(test)]
#[path = "search_availability/tests.rs"]
mod tests;

use den_core::{
    config::Config,
    ids::{BearId, UserId},
    tools::web::BRAVE_SEARCH_URL,
    DenError, TurnExecutionOrigin,
};
use den_http::web_policy::{self, WebApprovalDecision};
use den_service::{
    bears::hats::{access, memory_binding},
    conversation::persistence,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(super) async fn for_turn(
    pool: &PgPool,
    config: &Config,
    origin: TurnExecutionOrigin,
    bear_id: Uuid,
    external_conversation_id: &str,
    user_id: Option<i32>,
) -> Result<bool, DenError> {
    if !matches!(
        origin,
        TurnExecutionOrigin::ArmatureConversation(_) | TurnExecutionOrigin::BrowserTaskSession
    ) {
        return Ok(false);
    }
    let bear = BearId::new(bear_id);
    memory_binding::for_external_conversation(pool, bear, external_conversation_id).await?;
    if config.den_search_provider != "brave" {
        return Ok(false);
    }
    if web_policy::decide_web_fetch_approval(pool, bear_id, BRAVE_SEARCH_URL)
        .await
        .map_err(den_http::errors::CustomError::into_den)?
        .1
        == WebApprovalDecision::Blocked
    {
        return Ok(false);
    }
    let Some(human) = user_id.map(UserId::new) else {
        return Ok(false);
    };
    let Some(conversation) =
        persistence::get_conversation_for_external_id(pool, bear_id, external_conversation_id)
            .await?
    else {
        return Ok(false);
    };
    let provider_host = reqwest::Url::parse(BRAVE_SEARCH_URL)
        .map_err(|err| DenError::System(format!("invalid configured search provider URL: {err}")))?
        .host_str()
        .ok_or_else(|| DenError::System("search provider URL is missing its host".into()))?
        .to_string();
    access::has_web_search_grants_for_own_conversation(
        pool,
        bear,
        conversation.id,
        human,
        &provider_host,
    )
    .await
}
