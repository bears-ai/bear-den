use super::super::session_state::{MAX_PAGES, MAX_PAGES_PER_ACTOR, MAX_PAGES_PER_SCOPE};
use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn one_owners_repeated_full_pages_do_not_block_another_owners_inspection_or_optout(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    f.enable().await;
    let owner = f.inspect().await.owner;
    for index in 0..100 {
        personal_memory::apply_owner_memory_action(
            f.state.sqlx_pool(),
            f.state.memory_stores().unwrap(),
            f.bear_id,
            f.owner,
            &owner,
            OwnerMemoryAction::RequestPreferredName {
                request_id: Uuid::new_v4(),
                name: PreferredName::new(&format!("Pressure name {index}")).unwrap(),
            },
        )
        .await
        .unwrap();
    }
    let other_off = request(&f.app, &f.other_cookie, "GET", BASE, "").await;
    assert_eq!(other_off.status, StatusCode::OK);
    let enabled = request(
        &f.app,
        &f.other_cookie,
        "POST",
        &format!("{BASE}/capture"),
        &form(&other_off.body, "capture", "acknowledged=true"),
    )
    .await;
    assert_eq!(enabled.status, StatusCode::SEE_OTHER);
    let other_on = request(&f.app, &f.other_cookie, "GET", BASE, "").await;
    let optout = form(&other_on.body, "capture", "acknowledged=true");
    let first = f.get().await;
    let retired = form(&first.body, "capture", "acknowledged=true");
    // Each page has 100 Pending candidates / about 402 tokens. This used to
    // exhaust the shared 4096-token limit in about ten refreshes.
    for _ in 0..20 {
        let page = f.get().await;
        assert_eq!(
            page.body.matches("Approve preferred name</button>").count(),
            100
        );
    }
    let other_page = request(&f.app, &f.other_cookie, "GET", BASE, "").await;
    assert_eq!(other_page.status, StatusCode::OK);
    assert!(other_page.body.contains("id=\"request-name\""));
    assert!(!other_page.body.contains("Pressure name"));
    let disabled = request(
        &f.app,
        &f.other_cookie,
        "POST",
        &format!("{BASE}/capture"),
        &optout,
    )
    .await;
    assert_eq!(disabled.status, StatusCode::SEE_OTHER, "{}", disabled.body);
    let other = personal_memory::inspect_personal_memory(
        f.state.sqlx_pool(),
        f.state.memory_stores().unwrap(),
        f.bear_id,
        f.other,
    )
    .await
    .unwrap();
    assert!(!other.capture_enabled);
    assert_eq!(
        f.submit("capture", &retired).await.status,
        StatusCode::BAD_REQUEST
    );
    assert!(f.inspect().await.capture_enabled);
}

#[sqlx::test(migrations = "../../migrations")]
async fn scope_and_account_quotas_retire_only_their_own_old_pages(pool: PgPool) {
    let f = fixture(pool).await;
    let owner = f.inspect().await.owner;
    let registry = Registry::default();
    let other_owner = personal_memory::inspect_personal_memory(
        f.state.sqlx_pool(),
        f.state.memory_stores().unwrap(),
        f.bear_id,
        f.other,
    )
    .await
    .unwrap()
    .owner;
    let other_token = ActionNonce::new();
    registry
        .register(
            Scope::new(f.bear_id, f.other, "other-session".into()),
            other_owner,
            vec![(other_token, Intent::Capture(true))],
        )
        .unwrap();
    let mut tokens = Vec::new();
    for _ in 0..=MAX_PAGES_PER_SCOPE {
        let token = ActionNonce::new();
        registry
            .register(
                Scope::new(f.bear_id, f.owner, "same-session".into()),
                owner.clone(),
                vec![(token, Intent::Capture(true))],
            )
            .unwrap();
        tokens.push(token);
    }
    assert!(registry
        .authorize(
            tokens[0],
            Operation::Capture,
            f.bear_id,
            f.owner,
            "same-session"
        )
        .is_err());
    assert!(registry
        .authorize(
            tokens[1],
            Operation::Capture,
            f.bear_id,
            f.owner,
            "same-session"
        )
        .is_ok());
    for index in 0..=MAX_PAGES_PER_ACTOR {
        registry
            .register(
                Scope::new(f.bear_id, f.owner, format!("extra-session-{index}")),
                owner.clone(),
                vec![(ActionNonce::new(), Intent::Capture(true))],
            )
            .unwrap();
    }
    assert!(registry
        .authorize(
            tokens[1],
            Operation::Capture,
            f.bear_id,
            f.owner,
            "same-session"
        )
        .is_err());
    assert!(registry
        .authorize(
            other_token,
            Operation::Capture,
            f.bear_id,
            f.other,
            "other-session"
        )
        .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn global_capacity_retires_an_old_page_instead_of_refusing_a_new_one(pool: PgPool) {
    let f = fixture(pool).await;
    let owner = f.inspect().await.owner;
    let registry = Registry::default();
    let first = ActionNonce::new();
    registry
        .register(
            Scope::new(f.bear_id, f.owner, "first-session".into()),
            owner.clone(),
            vec![(first, Intent::Capture(true))],
        )
        .unwrap();
    // Only exercise registry allocation here; synthetic scopes never reach the
    // service, which independently rejects an owner/principal mismatch.
    let mut last = first;
    let mut actor = f.owner;
    for index in 0..MAX_PAGES {
        actor = UserId::new(10_000 + i32::try_from(index).unwrap());
        last = ActionNonce::new();
        registry
            .register(
                Scope::new(f.bear_id, actor, "capacity-session".into()),
                owner.clone(),
                vec![(last, Intent::Capture(true))],
            )
            .unwrap();
    }
    assert!(registry
        .authorize(
            first,
            Operation::Capture,
            f.bear_id,
            f.owner,
            "first-session"
        )
        .is_err());
    assert!(registry
        .authorize(
            last,
            Operation::Capture,
            f.bear_id,
            actor,
            "capacity-session"
        )
        .is_ok());
}
