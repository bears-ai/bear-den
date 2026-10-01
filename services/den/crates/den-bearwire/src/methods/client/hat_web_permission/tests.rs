use super::*;
use den_http::web_policy;
use den_service::bears::{db, hats};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn admin_hat_host_permission_persists_only_the_owned_hat_and_exact_host(pool: PgPool) {
    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "bearwirehatweb",
            name: "Hat web permission",
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
    let member = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatgrantmember', 'hatgrantmember@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear_id, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member, bear_id, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Research",
        "Read docs",
    )
    .await
    .unwrap();
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Other",
        "Other role",
    )
    .await
    .unwrap();
    let own = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin),
        "hat-acp-owned",
        None,
        None,
    )
    .await
    .unwrap();
    let other = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(member),
        "hat-acp-other",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), own.id, hat.id)
        .await
        .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), other.id, other_hat.id)
        .await
        .unwrap();
    let url = "https://example.com/docs";
    let payload = json!({"tool_name": "web_fetch", "arguments": {"url": url}});
    for (actor, conversation, input) in [
        (member, "hat-acp-other", payload.clone()),
        (admin, "hat-acp-other", payload.clone()),
        (
            admin,
            "hat-acp-owned",
            json!({"tool_name":"fs_edit_file","arguments":{"url":url}}),
        ),
        (
            admin,
            "hat-acp-owned",
            json!({"tool_name":"web_fetch","arguments":{"url":"http://example.com"}}),
        ),
    ] {
        assert!(
            persist(&pool, bear_id, actor, conversation, "client-admin", &input)
                .await
                .is_err()
        );
    }
    persist(
        &pool,
        bear_id,
        admin,
        "hat-acp-owned",
        "client-admin",
        &payload,
    )
    .await
    .unwrap();
    assert!(hats::access::has_web_fetch_grants_for_own_conversation(
        &pool,
        BearId::new(bear_id),
        own.id,
        UserId::new(admin),
        url,
    )
    .await
    .unwrap());
    assert!(!hats::access::has_web_fetch_grants_for_own_conversation(
        &pool,
        BearId::new(bear_id),
        other.id,
        UserId::new(member),
        url,
    )
    .await
    .unwrap());
    assert_eq!(
        web_policy::decide_web_fetch_approval(&pool, bear_id, url)
            .await
            .unwrap()
            .1,
        web_policy::WebApprovalDecision::RequiresApproval
    );
}
