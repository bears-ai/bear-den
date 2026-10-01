use super::*;
use crate::bears::{
    db::{self, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
    hats,
};
use den_core::config::Config;
use den_memory::{append_memory_record, LogicalMemoryPath};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn populated_hat_requires_a_fresh_complete_admin_review_before_work(pool: PgPool) {
    let user = UserId::new(sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatworkreviewadmin', 'hatworkreviewadmin@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap());
    let member = UserId::new(sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatworkreviewmember', 'hatworkreviewmember@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap());
    let bear = BearId::new(
        db::create_bear(
            &pool,
            BearParams {
                slug: "hatworkreview",
                name: "Hat Work Review",
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
    db::grant_membership(&pool, user.get(), bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member.get(), bear.as_uuid(), Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, user, "Security", "Review code")
        .await
        .unwrap();
    let surface = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, 'hatworkreviewrepo', 'git_workspace', $2, NOW(), NOW())",
        surface,
        user.get(),
    )
    .execute(&pool)
    .await
    .unwrap();
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
        .join(format!("hat-review-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "note"),
        "note",
        "curate",
        None,
        "Earlier reviewed data for interactive users",
        &json!({}),
    )
    .await
    .unwrap();
    let identity_hash = identity_fingerprint(&hat.name, &hat.purpose, &hat.identity_prompt);
    assert!(
        hats::manage::enable_work_if_empty(&pool, &stores, bear, hat.id, &identity_hash)
            .await
            .is_err()
    );
    assert!(snapshot_for_admin(&pool, &stores, bear, hat.id, member)
        .await
        .is_err());
    let snapshot = snapshot_for_admin(&pool, &stores, bear, hat.id, user)
        .await
        .unwrap();
    assert_eq!(snapshot.total_records, 1);
    assert!(snapshot.complete);
    assert!(snapshot.sha256.is_some());
    let decision = WorkReviewDecision {
        expected_sha256: snapshot.sha256.unwrap(),
        expected_record_count: 1,
        expected_identity_sha256: snapshot.identity_sha256,
        rationale: "Reviewed every historical entry for autonomous Work".into(),
    };
    assert!(
        review_and_enable(&pool, &stores, bear, hat.id, member, decision.clone())
            .await
            .is_err()
    );
    assert!(review_and_enable(
        &pool,
        &stores,
        bear,
        hat.id,
        user,
        WorkReviewDecision {
            expected_sha256: "0".repeat(64),
            ..decision.clone()
        }
    )
    .await
    .is_err());
    assert!(
        !hats::manage::get_hat(&pool, bear, hat.id)
            .await
            .unwrap()
            .work_enabled
    );
    hats::manage::update_hat(
        &pool,
        bear,
        hat.id,
        "Renamed security",
        "Review code",
        "Review code",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(
        review_and_enable(&pool, &stores, bear, hat.id, user, decision.clone()).await,
        Err(DenError::ValidationError(_))
    ));
    hats::manage::update_hat(
        &pool,
        bear,
        hat.id,
        "Security",
        "Changed purpose before Work review",
        "Review code",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(
        review_and_enable(&pool, &stores, bear, hat.id, user, decision.clone()).await,
        Err(DenError::ValidationError(_))
    ));
    hats::manage::update_hat(
        &pool,
        bear,
        hat.id,
        "Security",
        "Review code",
        "Changed identity before Work review",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(
        review_and_enable(&pool, &stores, bear, hat.id, user, decision.clone()).await,
        Err(DenError::ValidationError(_))
    ));
    append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "second"),
        "second",
        "curate",
        None,
        "Another reviewed entry",
        &json!({}),
    )
    .await
    .unwrap();
    assert!(
        review_and_enable(&pool, &stores, bear, hat.id, user, decision)
            .await
            .is_err(),
        "concurrent memory changes invalidate the review snapshot"
    );
    hats::manage::set_auto_curate_enabled(&pool, bear, hat.id, true, true)
        .await
        .unwrap();
    let fresh = snapshot_for_admin(&pool, &stores, bear, hat.id, user)
        .await
        .unwrap();
    let reviewed_identity_hash = fresh.identity_sha256.clone();
    let receipt = review_and_enable(
        &pool,
        &stores,
        bear,
        hat.id,
        user,
        WorkReviewDecision {
            expected_sha256: fresh.sha256.unwrap(),
            expected_record_count: fresh.total_records,
            expected_identity_sha256: fresh.identity_sha256,
            rationale: "Reviewed both entries for sandbox Work and egress risk".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.record_count, 2);
    let enabled = hats::manage::get_hat(&pool, bear, hat.id).await.unwrap();
    assert!(enabled.work_enabled);
    assert!(enabled.auto_curate_enabled);
    let stored = sqlx::query!(
        "SELECT reviewed_by_user_id, record_count, identity_sha256 FROM bear_hat_work_reviews WHERE id = $1 AND bear_id = $2 AND hat_id = $3",
        receipt.id, bear.as_uuid(), hat.id.as_uuid(),
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(stored.reviewed_by_user_id, user.get());
    assert_eq!(stored.record_count, 2);
    assert_eq!(
        stored.identity_sha256.as_deref(),
        Some(reviewed_identity_hash.as_str())
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn large_hat_requires_all_pages_and_rejects_in_place_changes(pool: PgPool) {
    let user = UserId::new(sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ('hatbigreviewer', 'hatbigreviewer@example.test') RETURNING id"
    ).fetch_one(&pool).await.unwrap());
    let bear = BearId::new(
        db::create_bear(
            &pool,
            BearParams {
                slug: "hatbigreview",
                name: "Big Review",
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
    db::grant_membership(&pool, user.get(), bear.as_uuid(), Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let hat = hats::create_hat(&pool, bear, user, "Large hat", "Many notes")
        .await
        .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("hat-big-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    for index in 0..=500 {
        let kind = format!("entry{index}");
        append_memory_record(
            &store,
            &LogicalMemoryPath::hat(hat.id, &kind),
            &kind,
            "curate",
            None,
            "reviewed text",
            &json!({}),
        )
        .await
        .unwrap();
        if index == hat_review::REVIEW_PAGE_SIZE as i64 {
            let first = snapshot_for_admin(&pool, &stores, bear, hat.id, user)
                .await
                .unwrap();
            let second = snapshot_page_for_admin(&pool, &stores, bear, hat.id, user, 2)
                .await
                .unwrap();
            assert!(first.complete);
            assert_eq!(first.page_count, 2);
            assert_eq!(first.records.len(), hat_review::REVIEW_PAGE_SIZE);
            assert_eq!(second.records.len(), 1);
            assert_eq!(first.sha256, second.sha256);
            assert_ne!(first.records[0].memory_id, second.records[0].memory_id);
            assert!(
                snapshot_page_for_admin(&pool, &stores, bear, hat.id, user, 3)
                    .await
                    .is_err()
            );
        }
    }
    let snapshot = snapshot_for_admin(&pool, &stores, bear, hat.id, user)
        .await
        .unwrap();
    assert!(snapshot.complete);
    assert_eq!(snapshot.total_records, 501);
    assert_eq!(snapshot.records.len(), hat_review::REVIEW_PAGE_SIZE);
    assert_eq!(snapshot.page_count, 6);
    let final_page = snapshot_page_for_admin(&pool, &stores, bear, hat.id, user, 6)
        .await
        .unwrap();
    assert_eq!(final_page.records.len(), 1);
    assert_eq!(final_page.sha256, snapshot.sha256);
    assert!(
        snapshot_page_for_admin(&pool, &stores, bear, hat.id, user, 7)
            .await
            .is_err()
    );
    let surface = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, 'hatworkreviewrepo', 'git_workspace', $2, NOW(), NOW())",
        surface,
        user.get(),
    )
    .execute(&pool)
    .await
    .unwrap();
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
    let old_hash = snapshot.sha256.unwrap();
    sqlx::query("UPDATE memory_records SET content_text = ? WHERE bear_id = ? AND memory_id = ?")
        .bind("Changed without bumping sequence")
        .bind(bear.as_uuid().to_string())
        .bind(&snapshot.records[0].memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(matches!(
        review_and_enable(
            &pool,
            &stores,
            bear,
            hat.id,
            user,
            WorkReviewDecision {
                expected_sha256: old_hash,
                expected_record_count: snapshot.total_records,
                expected_identity_sha256: snapshot.identity_sha256.clone(),
                rationale: "I reviewed all the historical entries for Work".into(),
            }
        )
        .await,
        Err(DenError::ValidationError(_))
    ));
    assert!(
        !hats::manage::get_hat(&pool, bear, hat.id)
            .await
            .unwrap()
            .work_enabled
    );
    let fresh = snapshot_page_for_admin(&pool, &stores, bear, hat.id, user, 6)
        .await
        .unwrap();
    let receipt = review_and_enable(
        &pool,
        &stores,
        bear,
        hat.id,
        user,
        WorkReviewDecision {
            expected_sha256: fresh.sha256.unwrap(),
            expected_record_count: fresh.total_records,
            expected_identity_sha256: fresh.identity_sha256,
            rationale: "Reviewed all six pages and Work audience risks".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.record_count, 501);
    assert!(
        hats::manage::get_hat(&pool, bear, hat.id)
            .await
            .unwrap()
            .work_enabled
    );
}
