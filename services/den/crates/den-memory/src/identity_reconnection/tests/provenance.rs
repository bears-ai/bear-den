use super::*;

#[derive(Clone, Copy, Debug)]
enum ReviewedIdentity {
    Selected,
    Displaced,
}

#[derive(Clone, Copy, Debug)]
enum ProvenanceChange {
    OriginLabel,
    AdditionalOrigin,
    DisplayName,
}

async fn change_provenance(store: &BearMemoryStore, entity_id: &str, change: ProvenanceChange) {
    let bear_id = store.bear_id().to_string();
    match change {
        ProvenanceChange::OriginLabel => {
            let changed = sqlx::query!(
                "UPDATE user_identity_origins SET display_name = 'changed after review' WHERE bear_id = ? AND entity_id = ?",
                bear_id, entity_id,
            ).execute(store.pool()).await.unwrap();
            assert_eq!(changed.rows_affected(), 1);
        }
        ProvenanceChange::AdditionalOrigin => {
            let origin_id = Uuid::new_v4().to_string();
            sqlx::query!(
                "INSERT INTO user_identity_origins (origin_id, bear_id, entity_id, source_host, source_bear_id, source_user_id, username, display_name, was_exporter, recorded_at) VALUES (?, ?, ?, 'changed.example', 'historical-source-bear', 77, 'new-source-label', 'new source provenance', 0, '2026-01-01T00:00:00Z')",
                origin_id, bear_id, entity_id,
            ).execute(store.pool()).await.unwrap();
        }
        ProvenanceChange::DisplayName => {
            sqlx::query!(
                "UPDATE entities SET display_name = 'changed after review' WHERE bear_id = ? AND entity_id = ?",
                bear_id, entity_id,
            ).execute(store.pool()).await.unwrap();
        }
    }
}

async fn stale_provenance_rejects_commit(target: ReviewedIdentity) {
    for change in [
        ProvenanceChange::OriginLabel,
        ProvenanceChange::AdditionalOrigin,
        ProvenanceChange::DisplayName,
    ] {
        let store = new_test_store().await;
        let portable = person(&store, ALICE).await;
        let portable_data = seed_context(&store, &portable, "reviewed-portable-provenance").await;
        record_export_origins(
            &store,
            "source.example",
            ALICE,
            &[ExportUserLabel {
                user_id: ALICE,
                username: Some("alice".into()),
                display_name: Some("original Alice label".into()),
            }],
        )
        .await
        .unwrap();
        quarantine_imported_authenticated_users(&store)
            .await
            .unwrap();
        let displaced = person(&store, DESTINATION).await;
        let displaced_data =
            seed_context(&store, &displaced, "reviewed-displaced-provenance").await;
        record_export_origins(
            &store,
            "destination.example",
            DESTINATION,
            &[ExportUserLabel {
                user_id: DESTINATION,
                username: Some("local-person".into()),
                display_name: Some("original local label".into()),
            }],
        )
        .await
        .unwrap();
        let displaced_access = context(&store, DESTINATION).await;
        let review = review_reconnection(&store, ACTOR, &portable, DESTINATION)
            .await
            .unwrap();
        let changed_entity = match target {
            ReviewedIdentity::Selected => &portable,
            ReviewedIdentity::Displaced => &displaced,
        };
        change_provenance(&store, changed_entity, change).await;
        let fresh = review_reconnection(&store, ACTOR, &portable, DESTINATION)
            .await
            .unwrap();
        assert_eq!(fresh.selected.generation, review.selected.generation);
        assert_eq!(
            fresh.displaced.as_ref().unwrap().generation,
            review.displaced.as_ref().unwrap().generation
        );
        match target {
            ReviewedIdentity::Selected => {
                assert_ne!(fresh.selected, review.selected);
                assert_eq!(fresh.displaced, review.displaced);
            }
            ReviewedIdentity::Displaced => {
                assert_eq!(fresh.selected, review.selected);
                assert_ne!(fresh.displaced, review.displaced);
            }
        }
        let before = authority_snapshot(&store).await;
        let context_before = context_snapshot(&store).await;
        assert_validation(
            commit_review(
                &store,
                &review,
                ReplacementConsent::PreserveDisplacedIdentity,
            )
            .await,
        );
        assert_eq!(
            authority_snapshot(&store).await,
            before,
            "{target:?} {change:?}: stale provenance must not partly park or activate an identity"
        );
        assert_eq!(context_snapshot(&store).await, context_before);
        assert_visible(
            &store,
            &displaced_data.active_memory_id,
            &displaced_access,
            true,
        )
        .await;
        assert_visible(
            &store,
            &portable_data.active_memory_id,
            &displaced_access,
            false,
        )
        .await;
        assert_eq!(summary(&store, &portable).await.active_user_id, None);
        assert_eq!(
            summary(&store, &displaced).await.active_user_id,
            Some(DESTINATION)
        );

        // A new human review of the changed provenance remains usable; the rejected
        // review neither consumes authority nor restores the old label behind the caller.
        let receipt = commit_review(
            &store,
            &fresh,
            ReplacementConsent::PreserveDisplacedIdentity,
        )
        .await
        .unwrap();
        assert_receipt(&store, &fresh, &receipt).await;
        assert_eq!(person(&store, DESTINATION).await, portable);
        assert_eq!(summary(&store, &displaced).await.active_user_id, None);
        assert_visible(
            &store,
            &portable_data.active_memory_id,
            &context(&store, DESTINATION).await,
            true,
        )
        .await;
        assert_visible(
            &store,
            &displaced_data.active_memory_id,
            &displaced_access,
            false,
        )
        .await;
        assert_eq!(context_snapshot(&store).await, context_before);
    }
}

#[tokio::test]
async fn selected_origin_or_display_name_changes_invalidate_review_without_authority_changes() {
    stale_provenance_rejects_commit(ReviewedIdentity::Selected).await;
}

#[tokio::test]
async fn displaced_origin_or_display_name_changes_invalidate_review_without_authority_changes() {
    stale_provenance_rejects_commit(ReviewedIdentity::Displaced).await;
}

#[tokio::test]
async fn selected_origin_changes_also_invalidate_revocation_without_revoking_access() {
    let store = new_test_store().await;
    let entity_id = person(&store, ALICE).await;
    let data = seed_context(&store, &entity_id, "revoke-provenance").await;
    record_export_origins(&store, "source.example", ALICE, &[])
        .await
        .unwrap();
    let access = context(&store, ALICE).await;
    let review = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    record_export_origins(
        &store,
        "source.example",
        ALICE,
        &[ExportUserLabel {
            user_id: ALICE,
            username: Some("newly-reviewed-alice".into()),
            display_name: Some("new label".into()),
        }],
    )
    .await
    .unwrap();
    let before = authority_snapshot(&store).await;
    let context_before = context_snapshot(&store).await;
    assert_validation(commit_review(&store, &review, ReplacementConsent::NoReplacement).await);
    assert_eq!(authority_snapshot(&store).await, before);
    assert_eq!(context_snapshot(&store).await, context_before);
    assert_visible(&store, &data.active_memory_id, &access, true).await;
    let fresh = review_revocation(&store, ACTOR, &entity_id).await.unwrap();
    assert_eq!(fresh.selected.generation, review.selected.generation);
    assert_ne!(fresh.selected.origins, review.selected.origins);
    commit_review(&store, &fresh, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_visible(&store, &data.active_memory_id, &access, false).await;
}
