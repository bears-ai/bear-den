use super::*;
use den_cabinet::{
    ActorScope, AttachmentRole, CabinetPolicy, CreateItemRequest, ItemKind, ReadRequest,
    UpdateItemRequest,
};
use den_core::ids::{BearId, UserId};
use den_service::{artifacts as registry, cabinet};
use registry::{
    ArtifactReader, ArtifactRef, ArtifactStorageKind, ArtifactVisibility, CreateJsonArtifactInput,
    ReserveArtifactInput,
};

async fn page(
    pool: &sqlx::PgPool,
    user: i32,
    title: &str,
    content: &str,
) -> den_cabinet::CabinetItemRef {
    cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(UserId::new(user)),
            kind: ItemKind::Document,
            title: title.into(),
            content: content.into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap()
    .item
    .cabinet_ref
}
async fn document(
    pool: &sqlx::PgPool,
    bear: Uuid,
    user: i32,
    visibility: ArtifactVisibility,
) -> registry::ArtifactMetadata {
    registry::create_json_artifact(
        pool,
        CreateJsonArtifactInput {
            reserve: ReserveArtifactInput {
                bear_id: bear,
                created_by_user_id: Some(user),
                owner_profile: RuntimeContextLabel::ChannelConversation,
                kind: "test_document".into(),
                title: Some("PRIVATE ATTACHMENT TITLE".into()),
                summary: Some("PRIVATE ATTACHMENT SUMMARY".into()),
                content_type: Some("application/json".into()),
                storage_kind: ArtifactStorageKind::DbText,
                visibility,
                provenance: serde_json::json!({}),
                metadata: serde_json::json!({}),
                expires_at: Some(time::OffsetDateTime::now_utc() + time::Duration::hours(1)),
            },
            payload: serde_json::json!({"content":"private document bytes"}),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn attachments_enforce_independent_artifact_acl_and_retention() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (peer, peer_bear, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let reference = page(&pool, owner, "Attachment page", "Shared page").await;
    let owner_scope = ActorScope::user(UserId::new(owner));
    let peer_scope = ActorScope::user(UserId::new(peer));
    let artifact = document(&pool, bear, owner, ArtifactVisibility::SameUser).await;
    let artifact_ref = ArtifactRef::parse(&artifact.artifact_ref).unwrap();
    assert!(cabinet::attachments::link(
        &pool,
        &peer_scope,
        &reference,
        &artifact_ref,
        AttachmentRole::Data
    )
    .await
    .is_err());
    let attached = cabinet::attachments::link(
        &pool,
        &owner_scope,
        &reference,
        &artifact_ref,
        AttachmentRole::Data,
    )
    .await
    .unwrap();
    assert_eq!(
        cabinet::attachments::link(
            &pool,
            &owner_scope,
            &reference,
            &artifact_ref,
            AttachmentRole::Data
        )
        .await
        .unwrap(),
        attached
    );
    let visible = cabinet::attachments::list(&pool, &owner_scope, &reference)
        .await
        .unwrap();
    assert_eq!(visible.len(), 1);
    let encoded = serde_json::to_string(&visible).unwrap();
    for hidden in ["storage_key", "content_sha256", "provenance", "metadata"] {
        assert!(!encoded.contains(hidden), "leaked {hidden}");
    }
    assert!(cabinet::attachments::list(&pool, &peer_scope, &reference)
        .await
        .unwrap()
        .is_empty());
    assert!(
        cabinet::attachments::json_content(&pool, &peer_scope, &reference, &attached)
            .await
            .is_err()
    );
    assert!(registry::authorize_for_reader(
        &pool,
        &artifact_ref,
        ArtifactReader::Bear(BearId::new(bear)),
        registry::ArtifactAccessLevel::Content
    )
    .await
    .is_err());
    let payload = cabinet::attachments::json_content(&pool, &owner_scope, &reference, &attached)
        .await
        .unwrap();
    assert_eq!(payload["content"], "private document bytes");
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let content_url = format!("/cabinet/{reference}/attachments/{attached}/content");
    let (status, content) = get_page(&app, &cookie, &content_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&content).unwrap()["content"],
        "private document bytes"
    );
    let (status, _) = get_page(&app, &peer_cookie, &content_url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        registry::mark_artifact_deleted(&pool, bear, &artifact.artifact_ref)
            .await
            .is_err()
    );
    assert!(registry::list_expired_artifact_gc_candidates(
        &pool,
        bear,
        time::OffsetDateTime::now_utc() + time::Duration::hours(2),
        100
    )
    .await
    .unwrap()
    .iter()
    .all(|row| row.artifact_ref != artifact.artifact_ref));
    assert!(sqlx::query!(
        "DELETE FROM artifacts WHERE artifact_ref=$1",
        artifact.artifact_ref
    )
    .execute(&pool)
    .await
    .is_err());
    let another = page(&pool, owner, "Other page", "Other content").await;
    assert!(
        cabinet::attachments::unlink(&pool, &owner_scope, &another, &attached)
            .await
            .is_err()
    );
    let shared = document(&pool, bear, owner, ArtifactVisibility::BearVisible).await;
    let shared_ref = ArtifactRef::parse(&shared.artifact_ref).unwrap();
    let shared_link = cabinet::attachments::link(
        &pool,
        &owner_scope,
        &reference,
        &shared_ref,
        AttachmentRole::Data,
    )
    .await
    .unwrap();
    assert!(registry::authorize_for_reader(
        &pool,
        &shared_ref,
        ArtifactReader::Bear(BearId::new(peer_bear)),
        registry::ArtifactAccessLevel::Content
    )
    .await
    .is_err());
    assert_eq!(
        cabinet::attachments::list(&pool, &peer_scope, &reference)
            .await
            .unwrap()
            .len(),
        1
    );
    cabinet::pages::configure(
        &pool,
        &owner_scope,
        &reference,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert!(
        cabinet::attachments::json_content(&pool, &peer_scope, &reference, &shared_link)
            .await
            .is_err()
    );
    cabinet::attachments::unlink(&pool, &owner_scope, &reference, &attached)
        .await
        .unwrap();
    assert!(registry::list_expired_artifact_gc_candidates(
        &pool,
        bear,
        time::OffsetDateTime::now_utc() + time::Duration::hours(2),
        100
    )
    .await
    .unwrap()
    .iter()
    .any(|row| row.artifact_ref == artifact.artifact_ref));
    registry::mark_artifact_deleted(&pool, bear, &artifact.artifact_ref)
        .await
        .unwrap();
}

#[tokio::test]
async fn mission_link_and_snapshot_forms_preserve_visibility_and_version_identity() {
    use http_body_util::BodyExt;
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, slug, hat) = seed_member(&pool).await;
    let surface = assigned_surface_id(&pool, owner, bear).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let created=post_form(&app,&cookie,&format!("/bear/{slug}/jobs/new"),format!("hat_id={hat}&goal=Knowledge+fixture+{}&surface_id={surface}&commit_policy=none&task_title=Inspect+knowledge&task_criteria=record+inspection",Uuid::new_v4())).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let job_url = created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    let denied = post_form(
        &app,
        &peer_cookie,
        &format!("{job_url}/mission"),
        "cabinet_ref=&revision=0".into(),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let reference = page(
        &pool,
        owner,
        "PRIVATE MISSION TITLE",
        "First immutable content",
    )
    .await;
    let owner_scope = ActorScope::user(UserId::new(owner));
    cabinet::pages::configure(
        &pool,
        &owner_scope,
        &reference,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let saved = post_form(
        &app,
        &cookie,
        &format!("{job_url}/mission"),
        format!("cabinet_ref={reference}&revision=0"),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let stale = post_form(
        &app,
        &cookie,
        &format!("{job_url}/mission"),
        "cabinet_ref=&revision=0".into(),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::BAD_REQUEST);
    let (_, owner_html) = get_page(&app, &cookie, &job_url).await;
    assert!(owner_html.contains("PRIVATE MISSION TITLE"));
    let first = cabinet::read(
        &pool,
        ReadRequest {
            scope: owner_scope.clone(),
            cabinet_ref: reference.clone(),
            version_ref: None,
        },
    )
    .await
    .unwrap();
    let captured = post_form(
        &app,
        &cookie,
        &format!("{job_url}/mission/snapshot"),
        format!("revision=1&version={}", first.version.version_ref()),
    )
    .await;
    assert_eq!(captured.status(), StatusCode::SEE_OTHER);
    let jobs = PgDocketService::from_pool(&pool)
        .list_jobs_for_viewer(bear, owner, false, DocketJobListFilter::default())
        .await
        .unwrap();
    let job = jobs
        .iter()
        .find(|row| format!("/bear/{slug}/jobs/{}", route_id(row.id)) == job_url)
        .unwrap();
    let links = registry::list_artifact_links(&pool, bear, "docket_job", &job.id.to_string())
        .await
        .unwrap();
    let evidence = links.iter().find(|row| row.role == "source").unwrap();
    let copied = ArtifactRef::parse(&evidence.artifact_ref).unwrap();
    let evidence_url = format!("{job_url}/evidence/{}/content", copied.as_str());
    let (status, captured_html) = get_page(&app, &cookie, &job_url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(captured_html.contains(&evidence_url));
    assert!(captured_html.contains("Download saved copy"));
    let download = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&evidence_url)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(download.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(download.headers()["X-Content-Type-Options"], "nosniff");
    let downloaded: cabinet::snapshots::DocumentSnapshot =
        serde_json::from_slice(&download.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(downloaded.content, "First immutable content");
    assert_eq!(downloaded.version_ref, *first.version.version_ref());
    let (status, _) = get_page(&app, &peer_cookie, &evidence_url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query!(
        "UPDATE bear_jobs SET visibility = 'bear_visible' WHERE id = $1",
        job.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, peer_html) = get_page(&app, &peer_cookie, &job_url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!peer_html.contains("PRIVATE MISSION TITLE"));
    assert!(!peer_html.contains(reference.as_str()));
    assert!(!peer_html.contains(copied.as_str()));
    let (status, _) = get_page(&app, &peer_cookie, &evidence_url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let other = post_form(&app, &cookie, &format!("/bear/{slug}/jobs/new"),
        format!("hat_id={hat}&goal=Other+knowledge+fixture&surface_id={surface}&commit_policy=none&task_title=Inspect&task_criteria=record"),
    ).await;
    assert_eq!(other.status(), StatusCode::SEE_OTHER);
    let other_url = other.headers()[header::LOCATION].to_str().unwrap();
    let (status, _) = get_page(
        &app,
        &cookie,
        &format!("{other_url}/evidence/{}/content", copied.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    cabinet::update_item(
        &pool,
        UpdateItemRequest {
            scope: owner_scope.clone(),
            cabinet_ref: reference.clone(),
            content: "Newer page content".into(),
            base_version: first.version.version_ref().clone(),
            title: None,
        },
    )
    .await
    .unwrap();
    let value = registry::json_content_for_reader(
        &pool,
        &copied,
        ArtifactReader::Human(UserId::new(owner)),
    )
    .await
    .unwrap();
    let snapshot: cabinet::snapshots::DocumentSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(snapshot.version_ref, *first.version.version_ref());
    assert_eq!(snapshot.content, "First immutable content");
    assert_eq!(snapshot.content_sha256, first.version.content_sha256());
    assert!(registry::json_content_for_reader(
        &pool,
        &copied,
        ArtifactReader::Human(UserId::new(peer))
    )
    .await
    .is_err());
    cabinet::archive_item(&pool, &owner_scope, &reference)
        .await
        .unwrap();
    cabinet::delete_item(&pool, &owner_scope, &reference)
        .await
        .unwrap();
    assert!(registry::json_content_for_reader(
        &pool,
        &copied,
        ArtifactReader::Human(UserId::new(owner))
    )
    .await
    .is_ok());
    let foreign = post_form(
        &app,
        &peer_cookie,
        &format!("{job_url}/mission/snapshot"),
        format!("revision=1&version={}", first.version.version_ref()),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&job_url)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(
        body.contains("PRIVATE MISSION TITLE"),
        "retained evidence keeps its captured title"
    );
    assert!(
        !body.contains(&format!("/cabinet/{reference}")),
        "tombstoned Mission is not linked"
    );
    assert!(body.contains(&evidence_url));
    let (status, content) = get_page(&app, &cookie, &evidence_url).await;
    assert_eq!(status, StatusCode::OK);
    let retained: cabinet::snapshots::DocumentSnapshot = serde_json::from_str(&content).unwrap();
    assert_eq!(retained.content, "First immutable content");
    let (status, peer_html) = get_page(&app, &peer_cookie, &job_url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!peer_html.contains("PRIVATE MISSION TITLE"));
    assert!(!peer_html.contains(reference.as_str()));
    assert!(!peer_html.contains(copied.as_str()));
}
