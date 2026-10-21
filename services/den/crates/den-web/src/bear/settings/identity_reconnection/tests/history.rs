use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_imported_receipt_does_not_disable_identity_management(pool: PgPool) {
    let fixture = fixture(pool).await;
    let store = fixture.store().await;
    let identities_before = core::list_identities(&store).await.unwrap();
    den_memory::test_support::inject_history_fixture(&store, true, 1)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(core::list_identities(&store).await.unwrap()).unwrap(),
        serde_json::to_value(identities_before).unwrap(),
    );
    let history = core::receipt_history(&store, 100).await.unwrap();
    assert_eq!(history.unreadable_entries, 1);
    assert!(history.receipts.is_empty());
    let (status, page, _) = request(&fixture.app, &fixture.cookie, "GET", LIST, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains(&fixture.alice));
    assert!(page.contains(&fixture.bob));
    assert!(page
        .contains("Unreadable entries skipped in this bounded history window: <strong>1</strong>"));
    assert!(page.contains("retained in backups, not deleted"));
    assert!(!page.contains("Receipt history unavailable"));
    let form = format!(
        "prepare_token={}&entity_id={}&action=reconnect&destination_user_id={}",
        input(&page, "prepare_token"),
        fixture.alice,
        fixture.destination,
    );
    let (status, page, location) = request(
        &fixture.app,
        &fixture.cookie,
        "POST",
        &format!("{LIST}/prepare"),
        &form,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{page}");
    let form = fixture.reviewed_form(&location.unwrap(), false).await;
    assert_eq!(fixture.confirm(&form).await.0, StatusCode::OK);
    let history = core::receipt_history(&store, 100).await.unwrap();
    assert_eq!(
        history.unreadable_entries, 1,
        "unreadable history is retained, not deleted"
    );
    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.receipts[0].entity_id, fixture.alice);
    let (status, page, _) = request(&fixture.app, &fixture.cookie, "GET", LIST, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains(&history.receipts[0].receipt_id.to_string()));
}
