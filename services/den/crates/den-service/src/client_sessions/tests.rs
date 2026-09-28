use super::*;
use crate::bears::db::{create_bear, BearParams};

async fn bear(pool: &PgPool, slug: &str) -> Uuid {
    create_bear(
        pool,
        BearParams {
            slug,
            name: "Session ownership test",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
}

async fn user(pool: &PgPool, name: &str) -> i32 {
    sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
        name
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn session(user_id: i32, bear_id: Uuid, id: &str) -> UpsertClientSession {
    UpsertClientSession {
        user_id,
        bear_id,
        bear_slug: "session-guard-test".to_string(),
        client_session_id: id.to_string(),
        runtime_session_id: format!("runtime-{id}"),
        conversation_id: format!("conv-{id}"),
        resolved_conversation_id: None,
        client: "test".to_string(),
        cwd: None,
        current_mode: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_id_cannot_be_claimed_by_a_second_owner_or_bear(pool: PgPool) {
    let bear_id = bear(&pool, "session-guard-first").await;
    let other_bear_id = bear(&pool, "session-guard-second").await;
    let owner = user(&pool, "sessionguardowner").await;
    let other = user(&pool, "sessionguardother").await;
    let id = "client-session-shared-id";

    upsert_session(&pool, session(owner, bear_id, id))
        .await
        .unwrap();
    upsert_session(&pool, session(owner, bear_id, id))
        .await
        .expect("same canonical owner may reconnect");
    assert!(upsert_session(&pool, session(other, bear_id, id))
        .await
        .is_err());
    assert!(upsert_session(&pool, session(owner, other_bear_id, id))
        .await
        .is_err());
    let rows = sqlx::query!(
        "SELECT user_id, bear_id FROM client_sessions WHERE client_session_id = $1",
        id
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].user_id, rows[0].bear_id), (owner, bear_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn simultaneous_session_claims_have_one_canonical_owner(pool: PgPool) {
    let bear_id = bear(&pool, "session-guard-race").await;
    let first = user(&pool, "sessionguardracefirst").await;
    let second = user(&pool, "sessionguardracesecond").await;
    let id = "client-session-race-id";
    let (left, right) = tokio::join!(
        upsert_session(&pool, session(first, bear_id, id)),
        upsert_session(&pool, session(second, bear_id, id)),
    );
    assert_ne!(
        left.is_ok(),
        right.is_ok(),
        "exactly one claim must succeed"
    );
    let owners = sqlx::query_scalar!(
        "SELECT user_id FROM client_sessions WHERE client_session_id = $1",
        id
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(owners.len(), 1);
    assert!(owners[0] == first || owners[0] == second);
}
