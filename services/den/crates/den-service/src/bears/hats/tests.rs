use super::*;

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
    assert_eq!(
        list_hats(&pool, BearId::new(bear)).await.unwrap(),
        vec![hat.clone()]
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

    sqlx::query!("DELETE FROM bears WHERE id = ANY($1)", &[bear, other_bear])
        .execute(&pool)
        .await
        .expect("delete test Bears with their hats and assignments");
    sqlx::query!("DELETE FROM work_surfaces WHERE id = $1", surface)
        .execute(&pool)
        .await
        .expect("delete test surface");
    sqlx::query!("DELETE FROM users WHERE id = $1", user)
        .execute(&pool)
        .await
        .expect("delete test user");
}
