use super::*;
use crate::bears::{
    db::{self, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
    hats,
};
use den_core::config::Config;
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn only_a_bear_admin_can_promote_verified_hat_knowledge_to_core(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatcoreadmin', 'hatcoreadmin@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let member = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatcoremember', 'hatcoremember@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear = BearId::new(
        db::create_bear(
            &pool,
            BearParams {
                slug: "hatcorebear",
                name: "Core test",
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
    db::grant_membership(&pool, user, bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member, bear.as_uuid(), Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, UserId::new(user), "Security", "Review code")
        .await
        .unwrap();
    let other = hats::create_hat(&pool, bear, UserId::new(user), "Support", "Help users")
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("hat-core-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "finding"),
        "finding",
        "curate",
        None,
        "Hat-only details",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "finding"),
        "finding",
        "pair",
        None,
        "Private raw details",
        &json!({}),
    )
    .await
    .unwrap();
    let source_id = Uuid::parse_str(&source.memory_id).unwrap();
    assert!(
        candidates(&pool, &stores, bear, UserId::new(member), hat.id)
            .await
            .is_err()
    );
    assert!(
        candidate(&pool, &stores, bear, UserId::new(user), other.id, source_id)
            .await
            .is_err()
    );
    assert!(candidate(
        &pool,
        &stores,
        bear,
        UserId::new(user),
        hat.id,
        Uuid::parse_str(&raw.memory_id).unwrap()
    )
    .await
    .is_err());
    let choice = candidates(&pool, &stores, bear, UserId::new(user), hat.id)
        .await
        .unwrap();
    assert_eq!(choice.len(), 1);
    assert_eq!(choice[0].memory_id, source.memory_id);
    let decision = CoreReviewDecision {
        source_memory_id: source_id,
        hat_id: hat.id,
        kind: "finding".into(),
        reviewed_content: "Shareable, independently reviewed finding".into(),
        expected_head: None,
        review_notes: "Omitted the sensitive hat-specific context".into(),
        acknowledge_bear_and_work_audience: true,
    };
    assert!(
        promote(&pool, &stores, bear, UserId::new(member), decision.clone())
            .await
            .is_err()
    );
    assert!(promote(
        &pool,
        &stores,
        bear,
        UserId::new(user),
        CoreReviewDecision {
            acknowledge_bear_and_work_audience: false,
            ..decision.clone()
        }
    )
    .await
    .is_err());
    let result = promote(&pool, &stores, bear, UserId::new(user), decision)
        .await
        .unwrap();
    let core = den_memory::library::detail(
        &store,
        &den_memory::library::CuratedMemoryGrant::new(vec![other.id]),
        &result.memory_id.to_string(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        core.content_text,
        "Shareable, independently reviewed finding"
    );
    assert!(promote(
        &pool,
        &stores,
        bear,
        UserId::new(user),
        CoreReviewDecision {
            source_memory_id: source_id,
            hat_id: hat.id,
            kind: "finding".into(),
            reviewed_content: "Duplicate".into(),
            expected_head: Some(result.memory_id),
            review_notes: "Must not reuse the same source promotion".into(),
            acknowledge_bear_and_work_audience: true,
        }
    )
    .await
    .is_err());
}
