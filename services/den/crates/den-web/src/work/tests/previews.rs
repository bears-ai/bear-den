//! Permission and content-safety checks for human Cabinet file inspection.

use super::uploads::{file_form, send, ByteStore};
use super::{get_page, login_cookie, seed_member, test_app, test_pool, TEST_DB_LOCK};
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use den_cabinet::{
    ActorScope, AttachmentRole, CabinetAttachmentRef, CabinetItemRef, CabinetPolicy,
};
use den_core::{ids::UserId, RuntimeContextLabel};
use den_service::{
    artifacts::{
        self, ArtifactRef, ArtifactStorageKind, ArtifactVisibility, CreateJsonArtifactInput,
        ReserveArtifactInput,
    },
    bears::db as bears_db,
    cabinet,
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

async fn request(app: &Router, cookie: &str, url: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(url)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn protected(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
}

async fn upload(
    app: &Router,
    cookie: &str,
    pool: &sqlx::PgPool,
    owner: i32,
    bear: Uuid,
    page: &CabinetItemRef,
    content_type: &str,
    bytes: &[u8],
    share: bool,
) -> CabinetAttachmentRef {
    let response = send(
        app,
        cookie,
        page,
        file_form(bear, "Preview file", content_type, bytes, share, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let links = cabinet::attachments::list(pool, &ActorScope::user(UserId::new(owner)), page)
        .await
        .unwrap();
    links.last().unwrap().reference.clone()
}

#[tokio::test]
async fn cabinet_json_inspection_is_escaped_private_and_omits_backing_metadata() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let scope = ActorScope::user(UserId::new(owner));
    let page = super::knowledge::page(&pool, owner, "Inspection page", "Source").await;
    let metadata = artifacts::create_json_artifact(&pool, CreateJsonArtifactInput {
        reserve: ReserveArtifactInput {
            bear_id: bear, created_by_user_id: Some(owner), owner_profile: RuntimeContextLabel::ChannelConversation,
            kind: "inspection_fixture".into(), title: Some("<private>.json".into()), summary: None, content_type: Some("application/json".into()),
            storage_kind: ArtifactStorageKind::DbText, visibility: ArtifactVisibility::SameUser,
            provenance: serde_json::json!({"source_secret":"DO_NOT_LEAK_PROVENANCE"}),
            metadata: serde_json::json!({"storage_secret":"DO_NOT_LEAK_METADATA"}), expires_at: None,
        },
        payload: serde_json::json!({"html":"<script>unsafeMarker()</script>","note":"Readable JSON"}),
    }).await.unwrap();
    let reference = ArtifactRef::parse(&metadata.artifact_ref).unwrap();
    let attached =
        cabinet::attachments::link(&pool, &scope, &page, &reference, AttachmentRole::Data)
            .await
            .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let url = format!("/cabinet/{page}/attachments/{attached}");
    let response = request(&app, &cookie, &url).await;
    assert_eq!(response.status(), StatusCode::OK);
    protected(&response);
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("&lt;private&gt;.json"));
    assert!(
        html.contains("&lt;script&gt;unsafeMarker()&lt;&#x2f;script&gt;"),
        "{}",
        html.split("cabinet-attachment-text\">")
            .nth(1)
            .unwrap_or(&html)
    );
    assert!(!html.contains("<script>unsafeMarker()"));
    assert!(html.contains("Readable JSON"));
    assert!(html.contains("Private to its creator"));
    assert!(html.contains("<summary>Details</summary>"));
    for hidden in [
        "DO_NOT_LEAK_PROVENANCE",
        "DO_NOT_LEAK_METADATA",
        "storage_key",
        "content_sha256",
    ] {
        assert!(!html.contains(hidden), "leaked {hidden}");
    }
    let (_, page_html) = get_page(&app, &cookie, &format!("/cabinet/{page}")).await;
    assert!(page_html.contains(&url));
    let (status, _) = get_page(&app, &peer_cookie, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_page(&app, &cookie, &format!("{url}/preview")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "JSON is never served as an inline active document"
    );
    let other_page = super::knowledge::page(&pool, owner, "Other inspection page", "Source").await;
    let (status, _) = get_page(
        &app,
        &cookie,
        &format!("/cabinet/{other_page}/attachments/{attached}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    cabinet::attachments::unlink(&pool, &scope, &page, &attached)
        .await
        .unwrap();
    let (status, _) = get_page(&app, &cookie, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cabinet_text_markdown_diff_html_script_and_svg_previews_are_source_only() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Source previews", "Source").await;
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    for (content_type, bytes) in [
        (
            "TEXT/PLAIN; charset=utf-8",
            b"Text <b>sourceOnly()</b>".as_slice(),
        ),
        (
            "text/markdown",
            b"# Heading\n<img src=\"https://example.test/tracker\">".as_slice(),
        ),
        (
            "text/x-diff",
            b"@@ diff @@\n+<script>sourceOnly()</script>".as_slice(),
        ),
        ("text/html", b"<script>sourceOnly()</script>".as_slice()),
        (
            "application/javascript",
            b"sourceOnly(); // <tag>".as_slice(),
        ),
        (
            "image/svg+xml",
            b"<svg onload=\"sourceOnly()\"></svg>".as_slice(),
        ),
    ] {
        let attached = upload(
            &app,
            &cookie,
            &pool,
            owner,
            bear,
            &page,
            content_type,
            bytes,
            false,
        )
        .await;
        let url = format!("/cabinet/{page}/attachments/{attached}");
        let (status, html) = get_page(&app, &cookie, &url).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains("cabinet-attachment-text"));
        assert!(html.contains("&lt;"));
        assert!(!html.contains(std::str::from_utf8(bytes).unwrap()));
        let (status, _) = get_page(&app, &cookie, &format!("{url}/preview")).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "active inline document for {content_type}"
        );
        let response = request(&app, &cookie, &format!("{url}/content")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert!(response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;"));
    }
}

#[tokio::test]
async fn cabinet_image_and_pdf_inline_bytes_are_signature_checked_and_sandboxed() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Binary previews", "Source").await;
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    // These fixtures exercise MIME/signature routing, not a full browser decoder.
    for (content_type, bytes) in [
        ("image/png", b"\x89PNG\r\n\x1a\nfixture".as_slice()),
        ("image/jpeg", b"\xff\xd8\xfffixture".as_slice()),
        ("image/gif", b"GIF89afixture".as_slice()),
        ("image/webp", b"RIFF1234WEBPfixture".as_slice()),
        ("application/pdf", b"%PDF-1.4\nfixture".as_slice()),
    ] {
        let attached = upload(
            &app,
            &cookie,
            &pool,
            owner,
            bear,
            &page,
            content_type,
            bytes,
            false,
        )
        .await;
        let url = format!("/cabinet/{page}/attachments/{attached}");
        let (status, html) = get_page(&app, &cookie, &url).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            html.contains(&format!(
                "src=\"{}\"",
                format!("{url}/preview").replace('/', "&#x2f;")
            )),
            "{content_type}: {}",
            html.split("<section class=\"cabinet-attachment-preview\">")
                .nth(1)
                .unwrap_or(&html)
        );
        if content_type == "application/pdf" {
            assert!(html.contains("sandbox referrerpolicy=\"no-referrer\""));
        }
        let response = request(&app, &cookie, &format!("{url}/preview")).await;
        assert_eq!(response.status(), StatusCode::OK);
        protected(&response);
        assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
        assert!(response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline;"));
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(policy.contains("default-src 'none'"));
        assert!(policy.contains("sandbox"));
        assert!(policy.contains("frame-ancestors 'self'"));
        assert_eq!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            bytes
        );
    }
    let markup = b"<script>mimeConfusion()</script>";
    let attached = upload(
        &app,
        &cookie,
        &pool,
        owner,
        bear,
        &page,
        "image/png",
        markup,
        false,
    )
    .await;
    let url = format!("/cabinet/{page}/attachments/{attached}");
    let (status, html) = get_page(&app, &cookie, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("cannot be previewed as its recorded type"));
    assert!(!html.contains("mimeConfusion()"));
    let (status, _) = get_page(&app, &cookie, &format!("{url}/preview")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cabinet_preview_rechecks_page_artifact_and_membership_at_each_byte_request() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let page = super::knowledge::page(&pool, owner, "Rechecked preview", "Source").await;
    let scope = ActorScope::user(UserId::new(owner));
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let attached = upload(
        &app,
        &cookie,
        &pool,
        owner,
        bear,
        &page,
        "image/png",
        b"\x89PNG\r\n\x1a\nfixture",
        true,
    )
    .await;
    let url = format!("/cabinet/{page}/attachments/{attached}");
    let (status, _) = get_page(&app, &peer_cookie, &url).await;
    assert_eq!(status, StatusCode::OK);
    cabinet::pages::configure(
        &pool,
        &scope,
        &page,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let (status, _) = get_page(&app, &peer_cookie, &format!("{url}/preview")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    cabinet::pages::configure(
        &pool,
        &scope,
        &page,
        CabinetPolicy::default(),
        &[],
        &[],
        &[],
    )
    .await
    .unwrap();
    bears_db::revoke_membership(&pool, peer, bear)
        .await
        .unwrap();
    let (status, _) = get_page(&app, &peer_cookie, &format!("{url}/preview")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    store
        .restrict_page_on_read(pool.clone(), owner, page.clone())
        .await;
    let (status, html) = get_page(&app, &peer_cookie, &format!("{url}/preview")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "revocation during storage read must block bytes"
    );
    assert!(!html.contains("fixture"));
}

#[tokio::test]
async fn cabinet_preview_fallbacks_do_not_claim_unconfigured_or_unsupported_content_is_ready() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Fallback previews", "Source").await;
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    let attached = upload(
        &app,
        &cookie,
        &pool,
        owner,
        bear,
        &page,
        "application/zip",
        b"zip fixture",
        false,
    )
    .await;
    let url = format!("/cabinet/{page}/attachments/{attached}");
    store.corrupt_reads().await;
    let (status, html) = get_page(&app, &cookie, &url).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unsupported previews should not fetch blob bytes"
    );
    assert!(html.contains("This format has no preview"));
    assert!(html.contains("Download file"));
    let no_storage = test_app(pool.clone()).await;
    let no_storage_cookie = login_cookie(&no_storage, owner).await;
    let (status, html) = get_page(&no_storage, &no_storage_cookie, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("this Den has no file storage configured"));
    assert!(!html.contains("Download file"));
}

#[tokio::test]
async fn cabinet_large_text_previews_are_utf8_bounded_without_truncating_downloads() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Bounded previews", "Source").await;
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    let text = format!("a{}END_OF_FULL_FILE", "€".repeat(90_000));
    let attached = upload(
        &app,
        &cookie,
        &pool,
        owner,
        bear,
        &page,
        "text/plain",
        text.as_bytes(),
        false,
    )
    .await;
    let url = format!("/cabinet/{page}/attachments/{attached}");
    let (status, html) = get_page(&app, &cookie, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Showing the first 256 KiB"));
    assert!(!html.contains("END_OF_FULL_FILE"));
    let response = request(&app, &cookie, &format!("{url}/content")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        text.as_bytes()
    );
}
