use super::*;
use den_cabinet::HistoryRequest;
use std::time::Duration;
use tokio::task::JoinSet;

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_appends_preserve_entries_and_page_isolation(pool: PgPool) {
    let (owner, first) = fixture(&pool).await;
    let second = cabinet::create_item(&pool, request(&owner))
        .await
        .unwrap()
        .item
        .cabinet_ref;
    let mut tasks = JoinSet::new();
    for index in 0..12 {
        for reference in [&first, &second] {
            let pool = pool.clone();
            let owner = owner.clone();
            let reference = reference.clone();
            tasks.spawn(async move {
                post(
                    &pool,
                    &owner,
                    &reference,
                    &format!("Concurrent entry {index}"),
                )
                .await
                .unwrap()
            });
        }
    }
    let entries = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut entries = Vec::new();
        while let Some(result) = tasks.join_next().await {
            entries.push(result.unwrap());
        }
        entries
    })
    .await
    .expect("concurrent appends must complete without deadlock");
    let refs = entries
        .iter()
        .map(|entry| entry.discussion_ref.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(refs.len(), 24);
    assert_eq!(entry_count(&pool).await, 24);
    for reference in [first, second] {
        let mut expected = entries
            .iter()
            .filter(|entry| entry.cabinet_ref == reference)
            .cloned()
            .collect::<Vec<_>>();
        expected.sort_by(|a, b| {
            (a.authored_at, a.discussion_ref.as_str())
                .cmp(&(b.authored_at, b.discussion_ref.as_str()))
        });
        assert_eq!(expected.len(), 12);
        assert_eq!(list(&pool, &owner, &reference).await.unwrap(), expected);
        let history = cabinet::history(
            &pool,
            HistoryRequest {
                scope: owner.clone(),
                cabinet_ref: reference,
            },
        )
        .await
        .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].revision, 1);
    }
}
