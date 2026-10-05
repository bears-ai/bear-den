use super::*;
use den_core::ids::{BearId, UserId};
use den_service::{connections, skills, work_surfaces};

#[tokio::test]
async fn reviewed_skills_are_pinned_scoped_and_removed_from_next_prompt_when_disabled() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let member = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let content = "Use the reviewed canary procedure. {{ not_evaluated_as_template }}";
    let skill = skills::create_draft(
        &pool,
        UserId::new(admin),
        &format!("procedure-{}", Uuid::new_v4()),
        "1",
        "Canary procedure",
        content,
    )
    .await
    .unwrap();
    assert!(
        skills::list(&pool, BearId::new(bear_id), UserId::new(member))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(skills::effective(
        &pool,
        BearId::new(bear_id),
        RuntimeContextLabel::ChannelConversation
    )
    .await
    .unwrap()
    .is_empty());
    let checksum = skills::hash(content);
    assert!(
        skills::approve(&pool, UserId::new(admin), skill, "stale", true)
            .await
            .is_err()
    );
    assert!(
        skills::approve(&pool, UserId::new(admin), skill, &checksum, false)
            .await
            .is_err()
    );
    skills::approve(&pool, UserId::new(admin), skill, &checksum, true)
        .await
        .unwrap();
    assert!(skills::attach(
        &pool,
        BearId::new(bear_id),
        UserId::new(member),
        skill,
        &checksum,
        &[RuntimeContextLabel::ChannelConversation],
        false
    )
    .await
    .is_err());
    assert!(skills::attach(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        skill,
        &checksum,
        &[RuntimeContextLabel::JobRun],
        false
    )
    .await
    .is_err());
    skills::attach(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        skill,
        &checksum,
        &[RuntimeContextLabel::ChannelConversation],
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        skills::list(&pool, BearId::new(bear_id), UserId::new(member))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(skills::effective(
        &pool,
        BearId::new(bear_id),
        RuntimeContextLabel::ArmatureConversation
    )
    .await
    .unwrap()
    .is_empty());
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Home",
        "House care",
    )
    .await
    .unwrap();
    let bear = bears_db::bear_for_user_by_slug(&pool, admin, &slug)
        .await
        .unwrap()
        .unwrap();
    let before = hats::identity::bound_prompt_text(
        &pool,
        &bear,
        RuntimeContextLabel::ChannelConversation,
        hat.id,
    )
    .await
    .unwrap();
    assert!(before.contains(content));
    skills::disable(&pool, UserId::new(admin), skill)
        .await
        .unwrap();
    let after = hats::identity::bound_prompt_text(
        &pool,
        &bear,
        RuntimeContextLabel::ChannelConversation,
        hat.id,
    )
    .await
    .unwrap();
    assert!(!after.contains(content));
}

#[tokio::test]
async fn reusable_connections_deny_foreign_attachment_and_revocation_without_secret_fallback() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let owner = create_bear_admin_user(&pool, bear_id).await;
    let stranger = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let key = "test-only-reusable-connection-key";
    let name = format!("repo-{}", Uuid::new_v4().simple());
    let surface = work_surfaces::create_surface(
        &pool,
        owner,
        work_surfaces::NewWorkSurface {
            name,
            description: None,
            upstream_url: "https://github.com/example/repository.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        key,
    )
    .await
    .unwrap();
    let secret = "private-token-never-render";
    let connection = connections::create(
        &pool,
        UserId::new(owner),
        &format!("account-{}", Uuid::new_v4()),
        connections::Material::HttpsToken(secret.into()),
        key,
    )
    .await
    .unwrap();
    assert!(connections::list(&pool, UserId::new(stranger))
        .await
        .unwrap()
        .is_empty());
    assert!(
        connections::attach(&pool, UserId::new(stranger), connection, surface.id)
            .await
            .is_err()
    );
    connections::attach(&pool, UserId::new(owner), connection, surface.id)
        .await
        .unwrap();
    assert!(connections::require_live_for_surface(&pool, surface.id)
        .await
        .is_ok());
    let metadata = connections::list(&pool, UserId::new(owner)).await.unwrap();
    let record = metadata.iter().find(|row| row.id == connection).unwrap();
    assert_eq!(record.repository_count, 1);
    assert!(!serde_json::to_string(record).unwrap().contains(secret));
    assert!(
        work_surfaces::set_credential(&pool, surface.id, "https_token", "stale-fallback", key)
            .await
            .is_err()
    );
    assert!(
        connections::revoke(&pool, UserId::new(stranger), connection, record.revision)
            .await
            .is_err()
    );
    connections::revoke(&pool, UserId::new(owner), connection, record.revision)
        .await
        .unwrap();
    assert!(connections::require_live_for_surface(&pool, surface.id)
        .await
        .is_err());
    connections::detach(&pool, UserId::new(owner), surface.id)
        .await
        .unwrap();
    assert!(work_surfaces::surface_by_id(&pool, surface.id)
        .await
        .unwrap()
        .unwrap()
        .credential_kind
        .is_none());
}

#[tokio::test]
async fn cabinet_tree_narrows_access_reviews_versions_and_preserves_lifecycle_rules() {
    use den_cabinet::{
        ActorScope, CabinetError, CabinetPolicy, CreateItemRequest, ItemKind, Lifecycle,
        ReadRequest, ReviewDecision, ReviewRequest, ReviewState, SearchFilters, SearchRequest,
        UpdateItemRequest,
    };
    use den_service::cabinet;
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let alice = create_bear_admin_user(&pool, bear_id).await;
    let bob = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let owner = ActorScope::user(UserId::new(alice));
    let reader = ActorScope::user(UserId::new(bob));
    let bear = ActorScope::bear(
        BearId::new(bear_id),
        RuntimeContextLabel::ChannelConversation,
    );
    let root_title = format!("private-root-{}", Uuid::new_v4());
    let request = |scope: ActorScope, title: String, body: &str| CreateItemRequest {
        scope,
        kind: ItemKind::Document,
        title,
        content: body.into(),
        collection_ref: None,
        mission_ref: None,
        source_links: vec![],
    };
    let root = cabinet::create_item(
        &pool,
        request(owner.clone(), root_title.clone(), "Mission goal"),
    )
    .await
    .unwrap();
    cabinet::pages::configure(
        &pool,
        &owner,
        &root.item.cabinet_ref,
        CabinetPolicy {
            bears_may_write: true,
            review_required: true,
            allowed_kinds: None,
        },
        &[alice],
        &[bear_id],
        &[],
    )
    .await
    .unwrap();
    let child_title = format!("child-{}", Uuid::new_v4());
    let child = cabinet::create_child(
        &pool,
        request(owner.clone(), child_title.clone(), "Published base"),
        &root.item.cabinet_ref,
    )
    .await
    .unwrap();
    assert!(matches!(
        cabinet::read(
            &pool,
            ReadRequest {
                scope: reader.clone(),
                cabinet_ref: child.item.cabinet_ref.clone(),
                version_ref: None
            }
        )
        .await,
        Err(CabinetError::NotFound)
    ));
    assert!(cabinet::search(
        &pool,
        SearchRequest {
            scope: reader.clone(),
            query: child_title.clone(),
            filters: SearchFilters::default()
        }
    )
    .await
    .unwrap()
    .is_empty());
    cabinet::pages::configure(
        &pool,
        &owner,
        &child.item.cabinet_ref,
        CabinetPolicy::default(),
        &[],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert!(cabinet::read(
        &pool,
        ReadRequest {
            scope: reader.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: None
        }
    )
    .await
    .is_err());
    assert!(cabinet::pages::organize(
        &pool,
        &owner,
        &root.item.cabinet_ref,
        Some(&child.item.cabinet_ref),
        0,
        true
    )
    .await
    .is_err());
    cabinet::pages::configure(
        &pool,
        &owner,
        &root.item.cabinet_ref,
        CabinetPolicy {
            bears_may_write: true,
            review_required: true,
            allowed_kinds: None,
        },
        &[alice, bob],
        &[bear_id],
        &[],
    )
    .await
    .unwrap();
    let pending = cabinet::update_item(
        &pool,
        UpdateItemRequest {
            scope: bear.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            content: "Awaiting human review".into(),
            base_version: child.version.version_ref().clone(),
            title: Some("Pending title".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(pending.version.review(), ReviewState::Pending);
    let published = cabinet::read(
        &pool,
        ReadRequest {
            scope: reader.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(published.version.content(), "Published base");
    assert_eq!(published.item.title, child_title);
    assert!(cabinet::read(
        &pool,
        ReadRequest {
            scope: reader.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: Some(pending.version.version_ref().clone())
        }
    )
    .await
    .is_err());
    assert!(cabinet::pages::pending_reviews(&pool, &reader)
        .await
        .unwrap()
        .is_empty());
    assert!(cabinet::pages::pending_reviews(&pool, &owner)
        .await
        .unwrap()
        .iter()
        .any(|row| row.version_ref == *pending.version.version_ref()));
    cabinet::pages::review(
        &pool,
        ReviewRequest {
            scope: owner.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: pending.version.version_ref().clone(),
            decision: ReviewDecision::Approved,
            rationale: "Reviewed shared content".into(),
        },
    )
    .await
    .unwrap();
    let accepted = cabinet::read(
        &pool,
        ReadRequest {
            scope: reader.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(accepted.version.content(), "Awaiting human review");
    assert_eq!(accepted.item.title, "Pending title");
    let next = |body: &str| UpdateItemRequest {
        scope: bear.clone(),
        cabinet_ref: child.item.cabinet_ref.clone(),
        content: body.into(),
        base_version: accepted.version.version_ref().clone(),
        title: None,
    };
    let one = cabinet::update_item(&pool, next("Candidate one"))
        .await
        .unwrap();
    let two = cabinet::update_item(&pool, next("Candidate two"))
        .await
        .unwrap();
    let review = |version: den_cabinet::CabinetVersionRef, decision| ReviewRequest {
        scope: owner.clone(),
        cabinet_ref: child.item.cabinet_ref.clone(),
        version_ref: version,
        decision,
        rationale: "Explicit review decision".into(),
    };
    cabinet::pages::review(
        &pool,
        review(one.version.version_ref().clone(), ReviewDecision::Approved),
    )
    .await
    .unwrap();
    assert!(matches!(
        cabinet::pages::review(
            &pool,
            review(two.version.version_ref().clone(), ReviewDecision::Approved)
        )
        .await,
        Err(CabinetError::Conflict { .. })
    ));
    cabinet::pages::review(
        &pool,
        review(two.version.version_ref().clone(), ReviewDecision::Rejected),
    )
    .await
    .unwrap();
    cabinet::archive_item(&pool, &owner, &root.item.cabinet_ref)
        .await
        .unwrap();
    cabinet::restore_item(&pool, &owner, &root.item.cabinet_ref)
        .await
        .unwrap();
    let archived_child = cabinet::read(
        &pool,
        ReadRequest {
            scope: owner.clone(),
            cabinet_ref: child.item.cabinet_ref.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(archived_child.item.lifecycle, Lifecycle::Archived);
    assert!(cabinet::delete_item(&pool, &owner, &root.item.cabinet_ref)
        .await
        .is_err());
}
