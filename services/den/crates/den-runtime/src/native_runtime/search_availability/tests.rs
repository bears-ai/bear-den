use super::*;
use den_core::{ids::UserId, ArmatureAvailability};
use den_service::{
    bears::{
        db,
        hats::{
            self,
            access::{HatAccessGrant, HttpsHost, ToolActionKey},
        },
    },
    conversation::persistence,
};

#[sqlx::test(migrations = "../../migrations")]
async fn search_roster_follows_current_owned_hat_tool_host_and_bear_block(pool: PgPool) {
    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "hatsearchroster",
            name: "Search roster",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('dispatch-hat@example.test', 'dispatchhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let first = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Research",
        "Search public sources",
    )
    .await
    .unwrap();
    let other = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Other",
        "Other role",
    )
    .await
    .unwrap();
    let own = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "hat-search-own",
        None,
        None,
    )
    .await
    .unwrap();
    let different = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "hat-search-other",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), own.id, first.id)
        .await
        .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), different.id, other.id)
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.den_search_provider = "brave".into();
    let origin = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let available = |conversation: &'static str, user| {
        for_turn(&pool, &config, origin, bear, conversation, Some(user))
    };
    let roster_has_search = |authorized| {
        crate::native_runtime::tools::merge_den_and_client_tools_with_search(
            &config, origin, true, true, true, None, None, authorized,
        )
        .unwrap()
        .iter()
        .any(|tool| tool.name == "web_search")
    };
    assert!(!roster_has_search(
        available("hat-search-own", admin).await.unwrap()
    ));
    let tool = HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name("web_search").unwrap());
    hats::access::grant(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        &tool,
        true,
    )
    .await
    .unwrap();
    assert!(!available("hat-search-own", admin).await.unwrap());
    let host = HatAccessGrant::HttpsHost(HttpsHost::parse("api.search.brave.com").unwrap());
    let grant_id = hats::access::grant(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        &host,
        true,
    )
    .await
    .unwrap();
    assert!(roster_has_search(
        available("hat-search-own", admin).await.unwrap()
    ));
    assert!(!roster_has_search(
        available("hat-search-other", admin).await.unwrap()
    ));
    assert!(available("hat-search-own", admin + 1).await.is_err());
    sqlx::query!("INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, policy) VALUES ($1, 'host', $2, 'blocked')", bear, "api.search.brave.com").execute(&pool).await.unwrap();
    assert!(!available("hat-search-own", admin).await.unwrap());
    sqlx::query!(
        "DELETE FROM bear_web_sources WHERE bear_id = $1 AND scope_value = $2",
        bear,
        "api.search.brave.com"
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::access::revoke(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        grant_id,
    )
    .await
    .unwrap();
    assert!(!roster_has_search(
        available("hat-search-own", admin).await.unwrap()
    ));
    assert!(!for_turn(
        &pool,
        &config,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        bear,
        "hat-search-own",
        Some(admin)
    )
    .await
    .unwrap());
    assert!(for_turn(
        &pool,
        &config,
        origin,
        bear,
        "missing-conversation",
        Some(admin)
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_hat_search_roster_rejects_missing_ordinary_source(pool: PgPool) {
    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "legacysearchroster",
            name: "Legacy search",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let config = Config::test_stub();
    assert!(for_turn(
        &pool,
        &config,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        bear,
        "unknown-legacy-conversation",
        None
    )
    .await
    .is_err());
}
