use super::{bindings::*, *};

async fn test_pool() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrate test database");
    Some(pool)
}

#[tokio::test]
async fn hats_only_attenuate_bear_surface_grants() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let nonce = Uuid::new_v4().simple().to_string();
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, 'x') RETURNING id",
        format!("hat-{nonce}@example.invalid"),
        format!("hat{}", &nonce[..12]),
        "Hat Test User",
    )
    .fetch_one(&pool)
    .await
    .expect("create user");
    let bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ($1, $2) RETURNING id",
        format!("hat-bear-{}", &nonce[..12]),
        "Hat Test Bear",
    )
    .fetch_one(&pool)
    .await
    .expect("create bear");
    let other_bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ($1, $2) RETURNING id",
        format!("hat-other-{}", &nonce[..12]),
        "Other Bear",
    )
    .fetch_one(&pool)
    .await
    .expect("create other bear");
    let surface = sqlx::query_scalar!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now()) RETURNING id",
        Uuid::new_v4(),
        format!("hat-surface-{}", &nonce[..12]),
        user,
    )
    .fetch_one(&pool)
    .await
    .expect("create surface");

    let hat = create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Security review",
        "Review repo",
    )
    .await
    .expect("create hat");
    assert!(!hat.work_enabled);
    let other_hat = create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Another responsibility",
        "Do something else",
    )
    .await
    .expect("create another hat");
    let conversation_id = sqlx::query_scalar!(
        "INSERT INTO conversations (bear_id, created_by_user_id) VALUES ($1, $2) RETURNING id",
        bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .expect("create conversation");
    let other_conversation_id = sqlx::query_scalar!(
        "INSERT INTO conversations (bear_id, created_by_user_id) VALUES ($1, $2) RETURNING id",
        other_bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .expect("create other Bear conversation");
    assert_eq!(
        conversation_hat(&pool, BearId::new(bear), conversation_id)
            .await
            .unwrap(),
        None
    );
    assert!(
        bind_conversation_hat(&pool, BearId::new(bear), other_conversation_id, hat.id)
            .await
            .is_err()
    );
    bind_conversation_hat(&pool, BearId::new(bear), conversation_id, hat.id)
        .await
        .expect("bind conversation");
    bind_conversation_hat(&pool, BearId::new(bear), conversation_id, hat.id)
        .await
        .expect("reconnection keeps the same binding");
    assert!(
        bind_conversation_hat(&pool, BearId::new(bear), conversation_id, other_hat.id)
            .await
            .is_err()
    );
    assert_eq!(
        conversation_hat(&pool, BearId::new(bear), conversation_id)
            .await
            .unwrap(),
        Some(hat.id)
    );
    assert_eq!(
        conversation_hat(&pool, BearId::new(other_bear), conversation_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        list_hats(&pool, BearId::new(bear)).await.unwrap(),
        vec![other_hat, hat.clone()]
    );
    assert!(list_hats(&pool, BearId::new(other_bear))
        .await
        .unwrap()
        .is_empty());
    assert!(create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "  ",
        "Review repo"
    )
    .await
    .is_err());
    assert!(allow_surface(&pool, BearId::new(bear), hat.id, surface)
        .await
        .is_err());

    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        other_bear,
    )
    .execute(&pool)
    .await
    .expect("assign surface to other Bear");
    assert!(allow_surface(&pool, BearId::new(bear), hat.id, surface)
        .await
        .is_err());
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        bear,
    )
    .execute(&pool)
    .await
    .expect("assign surface to Bear");
    assert!(
        allow_surface(&pool, BearId::new(other_bear), hat.id, surface)
            .await
            .is_err()
    );
    allow_surface(&pool, BearId::new(bear), hat.id, surface)
        .await
        .expect("hat may use Bear-assigned surface");
    let job_id = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Review the repository') RETURNING id",
        bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .expect("create draft Job");
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job_id,
        surface,
    )
    .execute(&pool)
    .await
    .expect("bind Job surface");
    assert!(bind_job_hat(&pool, BearId::new(bear), job_id, hat.id)
        .await
        .is_err());
    // No user-facing Work-enablement route exists yet; only the test fixture
    // simulates a hat that has already passed the review gate.
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.id.as_uuid()
    )
    .execute(&pool)
    .await
    .expect("enable Work for the fixture");
    assert!(bind_job_hat(&pool, BearId::new(other_bear), job_id, hat.id)
        .await
        .is_err());
    let second_surface = sqlx::query_scalar!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now()) RETURNING id",
        Uuid::new_v4(),
        format!("hat-second-surface-{}", &nonce[..12]),
        user,
    )
    .fetch_one(&pool)
    .await
    .expect("create second surface");
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        second_surface,
        bear,
    )
    .execute(&pool)
    .await
    .expect("assign second surface to Bear");
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job_id,
        second_surface,
    )
    .execute(&pool)
    .await
    .expect("assign second surface to Job");
    assert!(bind_job_hat(&pool, BearId::new(bear), job_id, hat.id)
        .await
        .is_err());
    allow_surface(&pool, BearId::new(bear), hat.id, second_surface)
        .await
        .expect("allow second Bear-assigned surface");
    bind_job_hat(&pool, BearId::new(bear), job_id, hat.id)
        .await
        .expect("bind eligible draft Job");
    assert_eq!(
        eligible_job_hat(&pool, BearId::new(bear), job_id)
            .await
            .unwrap(),
        Some(hat.id)
    );
    sqlx::query!(
        "DELETE FROM work_surface_bears WHERE surface_id = $1 AND bear_id = $2",
        surface,
        bear,
    )
    .execute(&pool)
    .await
    .expect("revoking the Bear's grant must not be blocked by the hat");
    let remaining = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_hat_work_surfaces WHERE hat_id = $1 AND surface_id = $2",
        hat.id.as_uuid(),
        surface,
    )
    .fetch_one(&pool)
    .await
    .expect("inspect hat restriction after revocation");
    assert_eq!(remaining, Some(0));
    assert_eq!(
        eligible_job_hat(&pool, BearId::new(bear), job_id)
            .await
            .unwrap(),
        None
    );

    sqlx::query!("DELETE FROM bears WHERE id = ANY($1)", &[bear, other_bear])
        .execute(&pool)
        .await
        .expect("delete test Bears with their hats and assignments");
    sqlx::query!("DELETE FROM work_surfaces WHERE id = $1", surface)
        .execute(&pool)
        .await
        .expect("delete test surface");
    sqlx::query!("DELETE FROM work_surfaces WHERE id = $1", second_surface)
        .execute(&pool)
        .await
        .expect("delete second test surface");
    sqlx::query!("DELETE FROM users WHERE id = $1", user)
        .execute(&pool)
        .await
        .expect("delete test user");
}
