use super::*;
use crate::bears::db::{
    create_bear, grant_membership, revoke_membership, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER,
};
use crate::conversation::persistence::ensure_conversation_for_external_id;

async fn setup(pool: &PgPool) -> (BearId, BearId, UserId, UserId, UserId) {
    let first_bear = create_bear(
        pool,
        BearParams {
            slug: "viewerfirst",
            name: "First",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let other_bear = create_bear(
        pool,
        BearParams {
            slug: "viewerother",
            name: "Other",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut users = Vec::new();
    for username in ["viewerone", "viewertwo", "vieweradmin"] {
        let id = sqlx::query_scalar!(
            "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
            username
        )
        .fetch_one(pool)
        .await
        .unwrap();
        users.push(UserId::new(id));
    }
    grant_membership(pool, users[0].get(), first_bear, Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    grant_membership(pool, users[1].get(), first_bear, Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    grant_membership(pool, users[2].get(), first_bear, Some(BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    (
        first_bear.into(),
        other_bear.into(),
        users[0],
        users[1],
        users[2],
    )
}

async fn ensure(
    pool: &PgPool,
    bear: BearId,
    owner: Option<UserId>,
    external: &str,
    session: &str,
) -> Uuid {
    ensure_conversation_for_external_id(
        pool,
        bear.as_uuid(),
        owner.map(UserId::get),
        external,
        Some(session),
        None,
    )
    .await
    .unwrap()
    .id
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_admin_and_bear_boundaries(pool: PgPool) {
    let (bear, other_bear, one, two, admin) = setup(&pool).await;
    let own = ensure(&pool, bear, Some(one), "viewertwo:own", "viewertwo:session").await;
    let others = ensure(
        &pool,
        bear,
        Some(two),
        "viewerone:others",
        "viewerone:session",
    )
    .await;
    let unowned = ensure(&pool, bear, None, "viewerone:unowned", "viewerone:session").await;
    let foreign = ensure(&pool, other_bear, Some(one), "foreign", "viewerone:session").await;
    let viewer = ConversationViewer::resolve(&pool, bear, one)
        .await
        .unwrap()
        .unwrap();
    let second = ConversationViewer::resolve(&pool, bear, two)
        .await
        .unwrap()
        .unwrap();
    let administrator = ConversationViewer::resolve(&pool, bear, admin)
        .await
        .unwrap()
        .unwrap();
    assert!(ConversationViewer::resolve(&pool, other_bear, one)
        .await
        .unwrap()
        .is_none());
    assert!(viewer.may_access_id(&pool, own).await.unwrap());
    assert!(viewer
        .may_access_external(&pool, "viewertwo:own")
        .await
        .unwrap());
    for (id, external) in [
        (others, "viewerone:others"),
        (unowned, "viewerone:unowned"),
        (foreign, "foreign"),
    ] {
        assert!(!viewer.may_access_id(&pool, id).await.unwrap());
        assert!(!viewer.may_access_external(&pool, external).await.unwrap());
    }
    assert!(!viewer.may_access_id(&pool, Uuid::new_v4()).await.unwrap());
    assert!(!viewer.may_access_external(&pool, "missing").await.unwrap());
    assert!(second.may_access_id(&pool, others).await.unwrap());
    assert!(!second.may_access_id(&pool, own).await.unwrap());
    assert!(!second.may_access_id(&pool, unowned).await.unwrap());
    for id in [own, others, unowned] {
        assert!(administrator.may_access_id(&pool, id).await.unwrap());
    }
    assert!(!administrator.may_access_id(&pool, foreign).await.unwrap());
    assert_eq!(
        viewer
            .list_visible(&pool, 20)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![own]
    );
    assert_eq!(
        second
            .list_visible(&pool, 20)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![others]
    );
    let admin_list = administrator.list_visible(&pool, 20).await.unwrap();
    assert_eq!(admin_list.len(), 3);
    assert!(admin_list.iter().any(|r| r.id == unowned));
    assert!(!admin_list.iter().any(|r| r.id == foreign));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_before_limit_and_rechecks_membership(pool: PgPool) {
    let (bear, _, one, two, admin) = setup(&pool).await;
    let own = ensure(&pool, bear, Some(one), "own", "session").await;
    let foreign = ensure(&pool, bear, Some(two), "other", "session").await;
    let unowned = ensure(&pool, bear, None, "unowned", "session").await;
    sqlx::query!(
        "UPDATE conversations SET updated_at = NOW() - INTERVAL '2 hours' WHERE id = $1",
        own
    )
    .execute(&pool)
    .await
    .unwrap();
    let viewer = ConversationViewer::resolve(&pool, bear, one)
        .await
        .unwrap()
        .unwrap();
    let administrator = ConversationViewer::resolve(&pool, bear, admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(viewer.list_visible(&pool, 1).await.unwrap()[0].id, own);
    assert_eq!(administrator.list_visible(&pool, 1).await.unwrap().len(), 1);
    assert!(!viewer.may_access_id(&pool, foreign).await.unwrap());
    assert!(!viewer.may_access_id(&pool, unowned).await.unwrap());

    // A resolved admin cannot retain access after a role change or revocation.
    grant_membership(&pool, admin.get(), bear.as_uuid(), Some(BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    assert!(!administrator.may_access_id(&pool, unowned).await.unwrap());
    assert!(administrator
        .list_visible(&pool, 20)
        .await
        .unwrap()
        .is_empty());
    revoke_membership(&pool, one.get(), bear.as_uuid())
        .await
        .unwrap();
    assert!(!viewer.may_access_id(&pool, own).await.unwrap());
    assert!(!viewer.may_access_external(&pool, "own").await.unwrap());
    assert!(viewer.list_visible(&pool, 20).await.unwrap().is_empty());
    assert!(ConversationViewer::resolve(&pool, bear, one)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn private_note_sources_require_active_hat_owner_and_current_membership(pool: PgPool) {
    let (bear, _, one, two, admin) = setup(&pool).await;
    let hat = crate::bears::hats::create_hat(&pool, bear, admin, "Notes", "Test owner-only notes")
        .await
        .unwrap();
    let own = ensure(&pool, bear, Some(one), "conv-own", "session-a").await;
    let other = ensure(&pool, bear, Some(two), "conv-other", "session-b").await;
    let admin_own = ensure(&pool, bear, Some(admin), "conv-admin", "session-c").await;
    let ownerless = ensure(&pool, bear, None, "conv-ownerless", "session-d").await;
    let archived = ensure(&pool, bear, Some(one), "conv-archived", "session-e").await;
    let legacy = ensure(&pool, bear, Some(one), "conv-legacy", "session-f").await;
    for id in [own, other, admin_own, ownerless, archived] {
        crate::bears::hats::bindings::bind_conversation_hat(&pool, bear, id, hat.id)
            .await
            .unwrap();
    }
    sqlx::query!(
        "UPDATE conversations SET status = 'archived' WHERE id = $1",
        archived
    )
    .execute(&pool)
    .await
    .unwrap();
    let first = ConversationViewer::resolve(&pool, bear, one)
        .await
        .unwrap()
        .unwrap();
    let administrator = ConversationViewer::resolve(&pool, bear, admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first
            .recent_own_note_sources(&pool)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![own]
    );
    assert_eq!(
        administrator
            .recent_own_note_sources(&pool)
            .await
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        vec![admin_own]
    );
    assert!(administrator.may_access_id(&pool, other).await.unwrap());
    assert!(administrator.may_access_id(&pool, ownerless).await.unwrap());
    assert!(!first.may_read_own_source(&pool, archived).await.unwrap());
    assert!(first.may_read_own_source(&pool, legacy).await.unwrap());
    revoke_membership(&pool, one.get(), bear.as_uuid())
        .await
        .unwrap();
    assert!(first
        .recent_own_note_sources(&pool)
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn ensure_conflict_never_transfers_ownership(pool: PgPool) {
    let (bear, _, one, two, admin) = setup(&pool).await;
    let original = ensure(&pool, bear, Some(one), "same-external", "session-one").await;
    let conflict = ensure(&pool, bear, Some(two), "same-external", "session-two").await;
    assert_eq!(original, conflict);
    let first = ConversationViewer::resolve(&pool, bear, one)
        .await
        .unwrap()
        .unwrap();
    let second = ConversationViewer::resolve(&pool, bear, two)
        .await
        .unwrap()
        .unwrap();
    assert!(first.may_access_id(&pool, original).await.unwrap());
    assert!(!second.may_access_id(&pool, original).await.unwrap());
    assert!(!second
        .may_access_external(&pool, "same-external")
        .await
        .unwrap());
    let owner = sqlx::query_scalar!(
        "SELECT created_by_user_id FROM conversations WHERE id = $1",
        original
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner, Some(one.get()));
    let null_owner = ensure(&pool, bear, None, "null-external", "session-one").await;
    assert_eq!(
        ensure(&pool, bear, Some(two), "null-external", "session-two").await,
        null_owner
    );
    assert!(!second.may_access_id(&pool, null_owner).await.unwrap());
    assert!(ConversationViewer::resolve(&pool, bear, admin)
        .await
        .unwrap()
        .unwrap()
        .may_access_id(&pool, null_owner)
        .await
        .unwrap());
}
