use super::*;
use crate::{
    bears::{db, hats},
    conversation::persistence,
};

async fn bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug,
                name: slug,
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    )
}

#[test]
fn tool_aliases_and_network_hosts_normalize_at_the_boundary() {
    assert_eq!(
        ToolActionKey::from_provider_name("fs_read_text_file").unwrap(),
        ToolActionKey::from_provider_name("armature.fs.read_text_file").unwrap()
    );
    assert_eq!(
        ToolActionKey::from_provider_name("web_fetch").unwrap(),
        ToolActionKey::from_provider_name("den.web.fetch").unwrap()
    );
    for invalid in ["unknown_tool", "mcp__untrusted__send", ""] {
        assert!(
            ToolActionKey::from_provider_name(invalid).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(
        HttpsHost::parse("EXAMPLE.COM.").unwrap(),
        HttpsHost::parse("example.com").unwrap()
    );
    for invalid in [
        "localhost",
        "repo.localhost",
        "127.0.0.1",
        "[::1]",
        "*.example.com",
        "example.com:8443",
        "https://example.com",
        "example.com/path",
        "singlelabel",
    ] {
        assert!(HttpsHost::parse(invalid).is_err(), "{invalid}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn hat_grants_are_admin_owned_idempotent_revocable_and_inert_until_enforced(pool: PgPool) {
    let first = bear(&pool, "hatgrantfirst").await;
    let second = bear(&pool, "hatgrantsecond").await;
    let admin = UserId::new(sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatgrantadmin', 'hatgrantadmin@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap());
    let member = UserId::new(sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatgrantmember', 'hatgrantmember@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap());
    db::grant_membership(
        &pool,
        admin.get(),
        first.as_uuid(),
        Some(db::BEAR_ROLE_ADMIN),
    )
    .await
    .unwrap();
    db::grant_membership(
        &pool,
        admin.get(),
        second.as_uuid(),
        Some(db::BEAR_ROLE_ADMIN),
    )
    .await
    .unwrap();
    db::grant_membership(
        &pool,
        member.get(),
        first.as_uuid(),
        Some(db::BEAR_ROLE_MEMBER),
    )
    .await
    .unwrap();
    let hat = hats::create_hat(&pool, first, admin, "Security", "Reviews code")
        .await
        .unwrap();
    let other_hat = hats::create_hat(&pool, first, admin, "Support", "Assists users")
        .await
        .unwrap();
    let foreign = hats::create_hat(&pool, second, admin, "Security", "Foreign Bear")
        .await
        .unwrap();
    let tool =
        HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name("fs_read_text_file").unwrap());
    let network = HatAccessGrant::HttpsHost(HttpsHost::parse("EXAMPLE.COM").unwrap());
    let own = persistence::ensure_conversation_for_external_id(
        &pool,
        first.as_uuid(),
        Some(member.get()),
        "conv-hat-grant-owner",
        None,
        None,
    )
    .await
    .unwrap();
    let admin_other_hat = persistence::ensure_conversation_for_external_id(
        &pool,
        first.as_uuid(),
        Some(admin.get()),
        "conv-hat-grant-other",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, first, own.id, hat.id)
        .await
        .unwrap();
    hats::bindings::bind_conversation_hat(&pool, first, admin_other_hat.id, other_hat.id)
        .await
        .unwrap();
    assert!(
        !has_grant_for_own_conversation(&pool, first, own.id, member, &tool)
            .await
            .unwrap()
    );
    assert!(!has_current(&pool, first, hat.id, &tool).await.unwrap());
    assert!(grant(&pool, first, hat.id, member, &tool, true)
        .await
        .is_err());
    assert!(grant(&pool, first, foreign.id, admin, &tool, true)
        .await
        .is_err());
    assert!(grant(&pool, first, hat.id, admin, &tool, false)
        .await
        .is_err());
    let id = grant(&pool, first, hat.id, admin, &tool, true)
        .await
        .unwrap();
    assert_eq!(
        grant(&pool, first, hat.id, admin, &tool, true)
            .await
            .unwrap(),
        id
    );
    let web_tool = HatAccessGrant::ToolForHat(
        ToolActionKey::from_provider_name(den_core::tools::constants::DEN_WEB_FETCH).unwrap(),
    );
    let web_url = "https://example.com/docs";
    assert!(
        !has_web_fetch_grants_for_own_conversation(&pool, first, own.id, member, web_url,)
            .await
            .unwrap()
    );
    grant(&pool, first, hat.id, admin, &web_tool, true)
        .await
        .unwrap();
    assert!(
        !has_web_fetch_grants_for_own_conversation(&pool, first, own.id, member, web_url,)
            .await
            .unwrap(),
        "tool grant alone cannot authorize a destination"
    );
    let host_grant_id = grant(&pool, first, hat.id, admin, &network, true)
        .await
        .unwrap();
    let surface_ceiling =
        AllowedOutboundHosts::new(vec!["example.com".into(), "other.example.com".into()]).unwrap();
    assert_eq!(
        intersect_surface_outbound_hosts(&pool, first, hat.id, &surface_ceiling)
            .await
            .unwrap()
            .as_slice(),
        &["example.com".to_string()],
    );
    assert!(
        intersect_surface_outbound_hosts(&pool, first, other_hat.id, &surface_ceiling)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        intersect_surface_outbound_hosts(&pool, second, hat.id, &surface_ceiling)
            .await
            .is_err()
    );
    assert!(
        has_web_fetch_grants_for_own_conversation(&pool, first, own.id, member, web_url,)
            .await
            .unwrap()
    );
    for target in [
        "https://other.example.com/docs",
        "http://example.com/docs",
        "https://example.com:8443/docs",
    ] {
        assert!(
            !has_web_fetch_grants_for_own_conversation(&pool, first, own.id, member, target,)
                .await
                .unwrap(),
            "{target}"
        );
    }
    assert!(!has_web_fetch_grants_for_own_conversation(
        &pool,
        first,
        admin_other_hat.id,
        admin,
        web_url,
    )
    .await
    .unwrap());
    assert!(has_current(&pool, first, hat.id, &tool).await.unwrap());
    assert!(has_current(&pool, first, hat.id, &network).await.unwrap());
    assert!(
        has_grant_for_own_conversation(&pool, first, own.id, member, &tool)
            .await
            .unwrap()
    );
    assert!(
        has_grant_for_own_conversation(&pool, first, own.id, member, &network)
            .await
            .unwrap()
    );
    assert!(
        !has_grant_for_own_conversation(&pool, first, admin_other_hat.id, admin, &network)
            .await
            .unwrap()
    );
    assert!(
        has_grant_for_own_conversation(&pool, first, own.id, admin, &tool)
            .await
            .is_err()
    );
    assert!(!has_current(&pool, first, other_hat.id, &tool)
        .await
        .unwrap());
    assert!(!has_current(&pool, second, foreign.id, &tool).await.unwrap());
    assert!(revoke(&pool, first, other_hat.id, admin, id).await.is_err());
    assert!(revoke(&pool, first, hat.id, member, id).await.is_err());
    revoke(&pool, first, hat.id, admin, id).await.unwrap();
    assert!(!has_current(&pool, first, hat.id, &tool).await.unwrap());
    assert!(
        !has_grant_for_own_conversation(&pool, first, own.id, member, &tool)
            .await
            .unwrap()
    );
    assert!(has_current(&pool, first, hat.id, &network).await.unwrap());
    revoke(&pool, first, hat.id, admin, host_grant_id)
        .await
        .unwrap();
    assert!(
        intersect_surface_outbound_hosts(&pool, first, hat.id, &surface_ceiling)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !has_web_fetch_grants_for_own_conversation(&pool, first, own.id, member, web_url,)
            .await
            .unwrap(),
        "host revocation takes effect without deleting the tool grant"
    );
    let replaced = grant(&pool, first, hat.id, admin, &tool, true)
        .await
        .unwrap();
    assert_ne!(id, replaced);
    db::revoke_membership(&pool, admin.get(), first.as_uuid())
        .await
        .unwrap();
    assert!(grant(&pool, first, hat.id, admin, &tool, true)
        .await
        .is_err());
    assert!(revoke(&pool, first, hat.id, admin, replaced).await.is_err());
    // Storage alone is not actor authority: revocation must fail interactive reads.
    assert!(has_current(&pool, first, hat.id, &tool).await.unwrap());
    db::revoke_membership(&pool, member.get(), first.as_uuid())
        .await
        .unwrap();
    assert!(
        has_grant_for_own_conversation(&pool, first, own.id, member, &tool)
            .await
            .is_err()
    );
}
