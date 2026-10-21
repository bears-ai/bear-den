use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn export_snapshots_safe_source_labels_without_allocating_an_exporter_person(pool: PgPool) {
    let fixture = fixture(pool).await;
    let store = fixture.store().await;
    let destination = den_memory::resolve_authenticated_user_entity(&store, fixture.destination)
        .await
        .unwrap();
    let before = core::list_identities(&store).await.unwrap().len();
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/bear/identityreview/export.bear")
                .header(header::COOKIE, &fixture.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let (_, sqlite) = crate::bear::settings::bundle_io::read_bear_bundle(&bytes).unwrap();
    let path = std::env::temp_dir().join(format!(
        "den-export-identity-test-{}.sqlite",
        Uuid::new_v4()
    ));
    std::fs::write(&path, sqlite).unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .read_only(true),
        )
        .await
        .unwrap();
    let snapshot = BearMemoryStore::new(fixture.bear_id.as_uuid(), pool.clone());
    let identities = core::list_identities(&snapshot).await.unwrap();
    assert_eq!(identities.len(), before);
    assert!(!identities
        .iter()
        .any(|row| row.active_user_id == Some(fixture.actor)));
    let exported = identities
        .iter()
        .find(|row| row.entity_id == destination.entity_id())
        .unwrap();
    assert!(exported.origins.iter().any(|origin| origin.source_host
        == fixture.state.config.web_server_url
        && origin.source_user_id == fixture.destination
        && origin.username.as_deref() == Some("identitydestination")
        && !origin.was_exporter));
    assert!(identities
        .iter()
        .find(|row| row.entity_id == fixture.alice)
        .unwrap()
        .origins
        .iter()
        .any(|origin| origin.source_host == "https://source.example" && origin.was_exporter));
    pool.close().await;
    std::fs::remove_file(path).unwrap();
    assert_eq!(core::list_identities(&store).await.unwrap().len(), before);
}
