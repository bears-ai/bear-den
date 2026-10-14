use super::*;

#[test]
fn discussion_refs_are_distinct_validated_and_serde_roundtrip() {
    let reference = CabinetDiscussionRef::mint();
    assert!(reference.as_str().starts_with("cabinet_discussion_"));
    assert_eq!(
        reference.as_str().len(),
        CabinetDiscussionRef::PREFIX.len() + 32
    );
    assert_eq!(
        CabinetDiscussionRef::parse(reference.as_str()).unwrap(),
        reference
    );
    assert_eq!(
        serde_json::from_str::<CabinetDiscussionRef>(&serde_json::to_string(&reference).unwrap())
            .unwrap(),
        reference
    );
    for invalid in [
        CabinetItemRef::mint().to_string(),
        "cabinet_discussion_ABCDEF".into(),
        format!("{}{}", CabinetDiscussionRef::PREFIX, "g".repeat(32)),
    ] {
        assert!(CabinetDiscussionRef::parse(&invalid).is_err());
        assert!(serde_json::from_str::<CabinetDiscussionRef>(
            &serde_json::to_string(&invalid).unwrap()
        )
        .is_err());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn trim_blank_oversize_and_unicode_scalar_boundaries(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    for blank in ["", " \n\t\r ", "\u{2003}\u{3000}"] {
        assert!(matches!(
            post(&pool, &owner, &reference, blank).await,
            Err(CabinetError::Validation(
                ContractViolation::EmptyField { .. }
            ))
        ));
    }
    for too_long in [
        "x".repeat(MAX_DISCUSSION_CHARS + 1),
        "🦀".repeat(MAX_DISCUSSION_CHARS + 1),
    ] {
        assert!(matches!(
            post(&pool, &owner, &reference, &too_long).await,
            Err(CabinetError::Policy(_))
        ));
    }
    assert!(matches!(
        post(&pool, &owner, &reference, "NUL\0value").await,
        Err(CabinetError::Policy(_))
    ));
    assert_eq!(entry_count(&pool).await, 0);
    let boundary = "🦀".repeat(MAX_DISCUSSION_CHARS);
    let entry = post(
        &pool,
        &owner,
        &reference,
        &format!("\u{2003}{boundary}\u{3000}"),
    )
    .await
    .unwrap();
    assert_eq!(entry.content, boundary);
    assert_eq!(entry.content.chars().count(), MAX_DISCUSSION_CHARS);
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn content_is_parameterized_plain_text_and_wire_time_is_rfc3339(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let content = "<script>alert('quoted')</script> & \"double\" \\ % _\n'; DROP TABLE users; --\n雪🦀e\u{301}";
    let entry = post(&pool, &owner, &reference, content).await.unwrap();
    assert_eq!(entry.content, content);
    assert_eq!(entry.authored_by, owner);
    let encoded = serde_json::to_value(&entry).unwrap();
    assert!(encoded["authored_at"].is_string());
    assert_eq!(
        serde_json::from_value::<DiscussionEntry>(encoded).unwrap(),
        entry
    );
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn latest_window_timestamp_ties_use_discussion_refs(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let mut references = Vec::new();
    for _ in 0..105 {
        let discussion_ref = CabinetDiscussionRef::mint();
        sqlx::query!(
            "INSERT INTO cabinet_discussion_entries(discussion_ref,item_id,content,authored_by,authored_by_user_id,authored_at)
             SELECT $1,id,'Same timestamp',$3,$4,'2026-10-13T13:01:43Z'::timestamptz FROM cabinet_items WHERE cabinet_ref=$2",
            discussion_ref.as_str(), reference.as_str(), Json(&owner) as _, user_id(&owner),
        ).execute(&pool).await.unwrap();
        references.push(discussion_ref);
    }
    references.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let entries = list(&pool, &owner, &reference).await.unwrap();
    assert_eq!(
        entries
            .into_iter()
            .map(|entry| entry.discussion_ref)
            .collect::<Vec<_>>(),
        references[5..]
    );
    assert_eq!(entry_count(&pool).await, 105);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_returns_only_newest_hundred_oldest_first(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let mut entries = Vec::new();
    for index in 0..105 {
        entries.push(
            post(&pool, &owner, &reference, &format!("Entry {index}"))
                .await
                .unwrap(),
        );
    }
    entries.sort_by(|a, b| {
        (a.authored_at, a.discussion_ref.as_str()).cmp(&(b.authored_at, b.discussion_ref.as_str()))
    });
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), entries[5..]);
    assert_eq!(entry_count(&pool).await, 105);
}
