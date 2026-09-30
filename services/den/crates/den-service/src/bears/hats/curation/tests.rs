use super::*;
use crate::{
    bears::{
        db::{self, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
        hats,
    },
    conversation::persistence,
};
use den_core::config::Config;
use den_memory::{
    append_memory_record,
    library::{self, CuratedMemoryGrant},
    LogicalMemoryPath, MemorySource,
};
use serde_json::json;

async fn new_bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        db::create_bear(
            pool,
            BearParams {
                slug,
                name: "Hat review test",
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

async fn new_user(pool: &PgPool, name: &str) -> UserId {
    UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id",
            name,
            format!("{name}@example.test"),
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_bear_admin_may_promote_verified_source_after_review(pool: PgPool) {
    let bear = new_bear(&pool, "hatcurationtest").await;
    let other_bear = new_bear(&pool, "hatcurationother").await;
    let admin = new_user(&pool, "hatcurationadmin").await;
    let member = new_user(&pool, "hatcurationmember").await;
    db::grant_membership(&pool, admin.get(), bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member.get(), bear.as_uuid(), Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, admin, "Security review", "Review code")
        .await
        .unwrap();
    let other_hat = hats::create_hat(&pool, other_bear, admin, "Different Bear", "Private")
        .await
        .unwrap();
    let conv = persistence::ensure_conversation_for_external_id(
        &pool,
        bear.as_uuid(),
        Some(admin.get()),
        "conv-curation-test",
        None,
        None,
    )
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("hat-curation-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(conv.id), "note"),
        "note",
        "pair",
        None,
        "Secret private raw session content",
        &json!({}),
    )
    .await
    .unwrap();
    let source_id = Uuid::parse_str(&raw.memory_id).unwrap();
    let request = ReviewedHatEntry {
        source_memory_id: source_id,
        hat_id: hat.id,
        kind: "note".into(),
        reviewed_content: "Use approved security checks before publishing.".into(),
        expected_head: None,
        review_notes: "Removed private details and checked accuracy".into(),
        work_audience_reviewed: false,
    };
    assert!(candidates(&pool, &stores, bear, member, hat.id, 20)
        .await
        .is_err());
    assert!(candidate(&pool, &stores, bear, member, hat.id, source_id)
        .await
        .is_err());
    assert!(promote(&pool, &stores, bear, member, request.clone())
        .await
        .is_err());
    assert!(promote(
        &pool,
        &stores,
        bear,
        admin,
        ReviewedHatEntry {
            hat_id: other_hat.id,
            ..request.clone()
        }
    )
    .await
    .is_err());
    assert_eq!(
        candidates(&pool, &stores, bear, admin, hat.id, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    let outcome = promote(&pool, &stores, bear, admin, request.clone())
        .await
        .unwrap();
    assert_eq!(
        library::detail(
            &store,
            &CuratedMemoryGrant::new(vec![hat.id]),
            &outcome.memory_id.to_string()
        )
        .await
        .unwrap()
        .unwrap()
        .content_text,
        request.reviewed_content
    );
    assert!(library::search(
        &store,
        &CuratedMemoryGrant::new(vec![hat.id]),
        "Secret private",
        10
    )
    .await
    .unwrap()
    .is_empty());
    assert!(promote(&pool, &stores, bear, admin, request).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn work_enabled_hat_requires_explicit_audience_review_and_canonical_source(pool: PgPool) {
    let bear = new_bear(&pool, "hatcurationwork").await;
    let admin = new_user(&pool, "hatcurationworker").await;
    db::grant_membership(&pool, admin.get(), bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, admin, "Work review", "Check code")
        .await
        .unwrap();
    let surface = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at) VALUES ($1, 'hatcurationrepo', 'git_workspace', $2, NOW(), NOW())",
        surface, admin.get(),
    ).execute(&pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        bear.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::manage::replace_surfaces(&pool, bear, hat.id, &[surface])
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("hat-curation-work-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let identity_hash = crate::bears::hats::identity::identity_fingerprint(
        &hat.name,
        &hat.purpose,
        &hat.identity_prompt,
    );
    hats::manage::enable_work_if_empty(&pool, &stores, bear, hat.id, &identity_hash)
        .await
        .unwrap();
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    let orphan = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "orphan"),
        "orphan",
        "pair",
        None,
        "not a verified source",
        &json!({}),
    )
    .await
    .unwrap();
    let valid = persistence::ensure_conversation_for_external_id(
        &pool,
        bear.as_uuid(),
        Some(admin.get()),
        "conv-curation-work",
        None,
        None,
    )
    .await
    .unwrap();
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(valid.id), "note"),
        "note",
        "pair",
        None,
        "Private raw",
        &json!({}),
    )
    .await
    .unwrap();
    let make_request = |source_memory_id| ReviewedHatEntry {
        source_memory_id,
        hat_id: hat.id,
        kind: "note".into(),
        reviewed_content: "Security review complete".into(),
        expected_head: None,
        review_notes: "Checked for Work audience".into(),
        work_audience_reviewed: false,
    };
    assert!(promote(
        &pool,
        &stores,
        bear,
        admin,
        ReviewedHatEntry {
            work_audience_reviewed: true,
            ..make_request(Uuid::parse_str(&orphan.memory_id).unwrap())
        }
    )
    .await
    .is_err());
    let source_id = Uuid::parse_str(&source.memory_id).unwrap();
    assert!(
        promote(&pool, &stores, bear, admin, make_request(source_id))
            .await
            .is_err()
    );
    let result = promote(
        &pool,
        &stores,
        bear,
        admin,
        ReviewedHatEntry {
            work_audience_reviewed: true,
            ..make_request(source_id)
        },
    )
    .await
    .unwrap();
    let detail = library::detail(
        &store,
        &CuratedMemoryGrant::new(vec![hat.id]),
        &result.memory_id.to_string(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(detail.metadata_json["work_audience_reviewed"], true);
    assert!(
        hats::manage::enable_work_if_empty(&pool, &stores, bear, hat.id, &identity_hash)
            .await
            .is_ok(),
        "already enabled remains idempotent"
    );
}
