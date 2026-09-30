use super::*;
use crate::bears::{
    db::{self, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
    hats,
};
use den_core::config::Config;
use den_memory::{append_memory_record, LogicalMemoryPath};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn only_admins_reauthor_unattributed_legacy_into_an_existing_hat(pool: PgPool) {
    let admin = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('legacyhatadmin@example.test', 'legacyhatadmin') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let member = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('legacyhatmember@example.test', 'legacyhatmember') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear = BearId::new(
        db::create_bear(
            &pool,
            BearParams {
                slug: "legacyreviewbear",
                name: "Legacy review",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    );
    db::grant_membership(&pool, admin, bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member, bear.as_uuid(), Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, UserId::new(admin), "Security", "Review safely")
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("legacy-review-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "note",
        "pair",
        None,
        "Potentially private old note",
        &json!({}),
    )
    .await
    .unwrap();
    let page = inventory_page(&pool, &stores, bear, UserId::new(admin), hat.id, None)
        .await
        .unwrap();
    assert_eq!(page.inventory[0].reviewable, 1);
    assert_eq!(page.candidates.len(), 1);
    assert!(
        inventory_page(&pool, &stores, bear, UserId::new(member), hat.id, None)
            .await
            .is_err()
    );
    let decision = LegacyReviewDecision {
        source_memory_id: raw.memory_id.clone(),
        target_hat: hat.id,
        kind: "note".into(),
        reviewed_content: "Reauthored for security work".into(),
        expected_head: None,
        review_notes: "Omitted any identifiable or private context".into(),
        acknowledge_unverified_source_and_members: true,
        work_audience_reviewed: false,
    };
    assert!(
        reauthor(&pool, &stores, bear, UserId::new(member), decision.clone())
            .await
            .is_err()
    );
    assert!(reauthor(
        &pool,
        &stores,
        bear,
        UserId::new(admin),
        LegacyReviewDecision {
            acknowledge_unverified_source_and_members: false,
            ..decision.clone()
        }
    )
    .await
    .is_err());
    let result = reauthor(&pool, &stores, bear, UserId::new(admin), decision)
        .await
        .unwrap();
    let published = den_memory::library::detail(
        &store,
        &den_memory::library::CuratedMemoryGrant::new(vec![hat.id]),
        &result.memory_id.to_string(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(published.content_text, "Reauthored for security work");
    assert_eq!(published.metadata_json["legacy_source_owner"], "unverified");
}
