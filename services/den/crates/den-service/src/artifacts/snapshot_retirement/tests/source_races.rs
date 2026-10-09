//! Deterministic interleavings at canonical proof, FK-target and link fences.
use super::*;
use crate::bears::db::deletion as removal;
use sqlx::{postgres::PgPoolOptions, Postgres, Transaction};
use std::time::Duration;

mod links;
mod proof_states;
mod reverse_references;

#[derive(Clone, Copy)]
enum Action {
    Retire,
    DeleteBear,
}
enum Attempt {
    Retire(RetireCabinetSnapshot),
    DeleteBear(removal::ConfirmBearDeletion),
}
async fn prepare(pool: &PgPool, f: &Fixture, action: Action) -> Attempt {
    let view = preview(pool, f.owner, &f.reference).await.unwrap();
    assert!(view.can_retire());
    let request = request(f, view.fingerprint);
    match action {
        Action::Retire => Attempt::Retire(request),
        Action::DeleteBear => {
            retire(pool, request).await.unwrap();
            let view = removal::preview(pool, f.owner, f.bear).await.unwrap();
            assert!(view.can_delete, "{:?}", view.blockers);
            Attempt::DeleteBear(removal::ConfirmBearDeletion {
                actor: f.owner,
                bear_id: f.bear,
                expected: view.fingerprint,
                confirm_slug: view.slug,
                acknowledged: true,
            })
        }
    }
}
async fn perform(pool: &PgPool, attempt: Attempt) -> Result<(), DenError> {
    match attempt {
        Attempt::Retire(request) => retire(pool, request).await.map(|_| ()),
        Attempt::DeleteBear(request) => removal::delete_confirmed(pool, request).await,
    }
}
async fn single_backend(pool: &PgPool) -> (PgPool, i32) {
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let pid = sqlx::query_scalar!(r#"SELECT pg_backend_pid() AS "pid!""#)
        .fetch_one(&single)
        .await
        .unwrap();
    (single, pid)
}
async fn backend(tx: &mut Transaction<'_, Postgres>) -> i32 {
    sqlx::query_scalar!(r#"SELECT pg_backend_pid() AS "pid!""#)
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}
async fn blocked_by(pool: &PgPool, waiter: i32, holder: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sqlx::query_scalar!(
                r#"SELECT $2=ANY(pg_blocking_pids($1)) AS "blocked!""#,
                waiter,
                holder
            )
            .fetch_one(pool)
            .await
            .unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected the exact PostgreSQL lock interleaving");
}
fn assert_busy(error: DenError) {
    let DenError::ValidationError(message) = error else {
        panic!("expected safe contention refusal, got {error:?}");
    };
    assert_eq!(
        message,
        "Work or evidence is changing. Review again after it settles; no changes were made."
    );
}
async fn assert_audit_present(pool: &PgPool, f: &Fixture, action: Action) {
    assert!(db::get_bear(pool, f.bear.as_uuid())
        .await
        .unwrap()
        .is_some());
    let row = sqlx::query!(
        "SELECT lifecycle,content_sha256 FROM artifacts WHERE artifact_ref=$1",
        f.reference.as_str()
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(row.content_sha256.is_some());
    assert_eq!(
        row.lifecycle,
        match action {
            Action::Retire => "finalized",
            Action::DeleteBear => "deleted",
        }
    );
    assert!(sqlx::query_scalar!("SELECT payload FROM artifact_json_payloads WHERE artifact_id=(SELECT id FROM artifacts WHERE artifact_ref=$1)",f.reference.as_str()).fetch_optional(pool).await.unwrap().is_some());
}
