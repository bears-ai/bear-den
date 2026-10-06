//! Browser form → signed S3 write/read substitute → registry finalization/link.

use super::{
    get_page, login_cookie, seed_member, test_app, test_app_with_state, test_pool, TEST_DB_LOCK,
};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::put,
    Router,
};
use den_cabinet::{ActorScope, AttachmentRole, CabinetItemRef};
use den_core::ids::{BearId, UserId};
use den_service::{
    artifacts::{
        self, ArtifactAccessLevel, ArtifactLifecycle, ArtifactReader, ArtifactRef,
        ArtifactVisibility,
    },
    bears::db as bears_db,
    cabinet,
};
use http_body_util::BodyExt;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Clone, Default)]
enum Fault {
    #[default]
    None,
    WriteDenied,
    DeleteDenied,
    DeleteMissingBucket,
    CorruptRead,
    ArchiveDuringRead {
        pool: sqlx::PgPool,
        scope: ActorScope,
        page: CabinetItemRef,
    },
    RestrictPageDuringRead {
        pool: sqlx::PgPool,
        owner: i32,
        page: CabinetItemRef,
    },
    RevokeMembershipDuringRead {
        pool: sqlx::PgPool,
        user: i32,
        bear: Uuid,
    },
}

#[derive(Default)]
struct Storage {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    fault: Mutex<Fault>,
    puts: AtomicUsize,
}

async fn storage_request(
    State(storage): State<Arc<Storage>>,
    Path((_bucket, key)): Path<(String, String)>,
    method: Method,
    bytes: Bytes,
) -> Response {
    let fault = storage.fault.lock().await.clone();
    match method {
        Method::PUT => {
            storage.puts.fetch_add(1, Ordering::Relaxed);
            if matches!(fault, Fault::WriteDenied) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            storage.objects.lock().await.insert(key, bytes.to_vec());
            StatusCode::OK.into_response()
        }
        Method::GET => {
            match fault {
                Fault::CorruptRead => return b"corrupt bytes".to_vec().into_response(),
                Fault::ArchiveDuringRead { pool, scope, page } => {
                    cabinet::archive_item(&pool, &scope, &page).await.unwrap();
                }
                Fault::RevokeMembershipDuringRead { pool, user, bear } => {
                    bears_db::revoke_membership(&pool, user, bear)
                        .await
                        .unwrap();
                }
                Fault::RestrictPageDuringRead { pool, owner, page } => {
                    cabinet::pages::configure(
                        &pool,
                        &ActorScope::user(UserId::new(owner)),
                        &page,
                        den_cabinet::CabinetPolicy::default(),
                        &[owner],
                        &[],
                        &[],
                    )
                    .await
                    .unwrap();
                }
                _ => {}
            }
            storage.objects.lock().await.get(&key).cloned().map_or_else(
                || StatusCode::NOT_FOUND.into_response(),
                IntoResponse::into_response,
            )
        }
        Method::DELETE => {
            if matches!(fault, Fault::DeleteMissingBucket) {
                return StatusCode::NOT_FOUND.into_response();
            }
            if matches!(fault, Fault::DeleteDenied) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            storage.objects.lock().await.remove(&key);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

pub(super) struct ByteStore {
    state: Arc<Storage>,
    endpoint: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ByteStore {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ByteStore {
    pub(super) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Storage::default());
        let app = Router::new()
            .route(
                "/{bucket}/{*key}",
                put(storage_request)
                    .get(storage_request)
                    .delete(storage_request),
            )
            .layer(DefaultBodyLimit::max(cabinet::uploads::MAX_FILE_BYTES))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            endpoint,
            task,
        }
    }

    pub(super) async fn restrict_page_on_read(
        &self,
        pool: sqlx::PgPool,
        owner: i32,
        page: CabinetItemRef,
    ) {
        *self.state.fault.lock().await = Fault::RestrictPageDuringRead { pool, owner, page };
    }

    pub(super) async fn corrupt_reads(&self) {
        *self.state.fault.lock().await = Fault::CorruptRead;
    }

    pub(super) async fn fail_deletes(&self, fail: bool) {
        *self.state.fault.lock().await = if fail {
            Fault::DeleteDenied
        } else {
            Fault::None
        };
    }

    pub(super) async fn missing_bucket_deletes(&self) {
        *self.state.fault.lock().await = Fault::DeleteMissingBucket;
    }

    pub(super) async fn contains(&self, key: &str) -> bool {
        self.state.objects.lock().await.contains_key(key)
    }

    pub(super) fn app_state(&self, pool: &sqlx::PgPool) -> crate::AppState {
        let mut config = crate::config::Config::test_stub();
        config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
        config.s3_endpoint = self.endpoint.clone();
        config.s3_bucket = "cabinet-fixture".into();
        config.s3_region = "us-east-1".into();
        config.s3_force_path_style = true;
        config.s3_access_key_id = "fixture-key".into();
        config.s3_secret_access_key = "fixture-secret".into();
        let media = crate::core::s3::MediaStore::new(&config);
        let config = Arc::new(config);
        let mut state = crate::AppState::test_with_template_env(
            pool.clone(),
            crate::template_environment(&config),
            config,
        );
        state.media = media;
        state
    }

    pub(super) async fn app(&self, pool: &sqlx::PgPool) -> Router {
        test_app_with_state(pool.clone(), self.app_state(pool)).await
    }
}

fn form(bear: Uuid, filename: &str, bytes: &[u8], share: bool, extra: &str) -> Vec<u8> {
    file_form(bear, filename, "text/plain", bytes, share, extra)
}

pub(super) fn file_form(
    bear: Uuid,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
    share: bool,
    extra: &str,
) -> Vec<u8> {
    let sharing = if share {
        "--upload-fixture\r\nContent-Disposition: form-data; name=\"share_with_bear\"\r\n\r\ntrue\r\n"
    } else {
        ""
    };
    let mut body = format!("--upload-fixture\r\nContent-Disposition: form-data; name=\"bear_id\"\r\n\r\n{bear}\r\n--upload-fixture\r\nContent-Disposition: form-data; name=\"role\"\r\n\r\ndata\r\n{sharing}{extra}--upload-fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--upload-fixture--\r\n");
    body
}

pub(super) async fn send(
    app: &Router,
    cookie: &str,
    page: &CabinetItemRef,
    body: Vec<u8>,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/cabinet/{page}/attachments/upload"))
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=upload-fixture",
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn cabinet_upload_is_private_by_default_and_sharing_is_explicit() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (peer, _, _, _) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, peer, bear, Some("member"))
        .await
        .unwrap();
    let page = super::knowledge::page(&pool, owner, "Upload page", "Upload content").await;
    let scope = ActorScope::user(UserId::new(owner));
    let peer_scope = ActorScope::user(UserId::new(peer));
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    let peer_cookie = login_cookie(&app, peer).await;
    let (_, html) = get_page(&app, &cookie, &format!("/cabinet/{page}")).await;
    assert!(html.contains("Upload file"));
    assert!(html.contains("enctype=\"multipart/form-data\""));
    assert!(!html.contains("name=\"share_with_bear\" value=\"true\" checked"));
    let bytes = b"Private file content";
    let response = send(
        &app,
        &cookie,
        &page,
        form(bear, "../../résumé.txt", bytes, false, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("/cabinet/{page}")
    );
    let links = cabinet::attachments::list(&pool, &scope, &page)
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    let attachment = &links[0];
    assert_eq!(attachment.role, AttachmentRole::Data);
    assert_eq!(attachment.artifact.title.as_deref(), Some("résumé.txt"));
    let reference = ArtifactRef::parse(&attachment.artifact.artifact_ref).unwrap();
    let metadata = artifacts::authorize_for_reader(
        &pool,
        &reference,
        ArtifactReader::Human(UserId::new(owner)),
        ArtifactAccessLevel::Content,
    )
    .await
    .unwrap();
    assert_eq!(metadata.lifecycle, ArtifactLifecycle::Finalized);
    assert_eq!(metadata.visibility, ArtifactVisibility::SameUser);
    assert_eq!(metadata.provenance["cabinet_ref"], page.as_str());
    assert_eq!(
        metadata.content_bytes,
        Some(i64::try_from(bytes.len()).unwrap())
    );
    let location = artifacts::content_location_for_reader(
        &pool,
        &reference,
        ArtifactReader::Human(UserId::new(owner)),
    )
    .await
    .unwrap();
    artifacts::verify_content_bytes(&location, bytes).unwrap();
    assert!(cabinet::attachments::list(&pool, &peer_scope, &page)
        .await
        .unwrap()
        .is_empty());
    let url = format!(
        "/cabinet/{page}/attachments/{}/content",
        attachment.reference
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&url)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
    let disposition = response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap();
    assert!(disposition.contains("filename*=UTF-8''r%C3%A9sum%C3%A9.txt"));
    assert!(!disposition.contains("../"));
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
    let (status, _) = get_page(&app, &peer_cookie, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, peer_html) = get_page(&app, &peer_cookie, &format!("/cabinet/{page}")).await;
    assert!(!peer_html.contains("résumé.txt"));
    assert!(!peer_html.contains(reference.as_str()));
    assert!(!peer_html.contains(&store.endpoint));
    let shared = send(
        &app,
        &cookie,
        &page,
        form(bear, "Shared report.txt", b"Shared report", true, ""),
    )
    .await;
    assert_eq!(shared.status(), StatusCode::SEE_OTHER);
    let visible = cabinet::attachments::list(&pool, &peer_scope, &page)
        .await
        .unwrap();
    assert_eq!(visible.len(), 1);
    let shared_ref = ArtifactRef::parse(&visible[0].artifact.artifact_ref).unwrap();
    assert!(artifacts::authorize_for_reader(
        &pool,
        &shared_ref,
        ArtifactReader::Bear(BearId::new(bear)),
        ArtifactAccessLevel::Content
    )
    .await
    .is_ok());
    assert!(
        artifacts::mark_artifact_deleted(&pool, bear, shared_ref.as_str())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cabinet_upload_rejects_missing_storage_access_and_invalid_forms_before_writes() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (other, foreign_bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Restricted upload page", "Private page").await;
    let scope = ActorScope::user(UserId::new(owner));
    cabinet::pages::configure(
        &pool,
        &scope,
        &page,
        den_cabinet::CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let store = ByteStore::start().await;
    let app = store.app(&pool).await;
    let cookie = login_cookie(&app, owner).await;
    let other_cookie = login_cookie(&app, other).await;
    let forbidden = send(
        &app,
        &other_cookie,
        &page,
        form(foreign_bear, "secret.txt", b"not uploaded", false, ""),
    )
    .await;
    assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
    let foreign = send(
        &app,
        &cookie,
        &page,
        form(foreign_bear, "foreign.txt", b"not uploaded", false, ""),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let duplicate = format!(
        "--upload-fixture\r\nContent-Disposition: form-data; name=\"bear_id\"\r\n\r\n{bear}\r\n"
    );
    let bad = send(
        &app,
        &cookie,
        &page,
        form(bear, "duplicate.txt", b"not uploaded", false, &duplicate),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let empty = send(
        &app,
        &cookie,
        &page,
        form(bear, "empty.txt", b"", false, ""),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    let big = send(
        &app,
        &cookie,
        &page,
        form(
            bear,
            "big.txt",
            &vec![0; cabinet::uploads::MAX_FILE_BYTES + 1],
            false,
            "",
        ),
    )
    .await;
    assert!(matches!(
        big.status(),
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
    ));
    let disabled = test_app(pool.clone()).await;
    let disabled_cookie = login_cookie(&disabled, owner).await;
    let (_, disabled_html) =
        get_page(&disabled, &disabled_cookie, &format!("/cabinet/{page}")).await;
    assert!(disabled_html.contains("File uploads are unavailable"));
    assert!(!disabled_html.contains("enctype=\"multipart/form-data\""));
    let response = send(
        &disabled,
        &disabled_cookie,
        &page,
        form(bear, "disabled.txt", b"not uploaded", false, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.state.puts.load(Ordering::Relaxed), 0);
    assert!(cabinet::attachments::list(&pool, &scope, &page)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn cabinet_upload_does_not_publish_failed_corrupt_or_revoked_transfers() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    for index in 0..5 {
        let (owner, bear, _, _) = seed_member(&pool).await;
        let page_owner = if index == 4 {
            seed_member(&pool).await.0
        } else {
            owner
        };
        let page = super::knowledge::page(&pool, page_owner, "Interrupted upload", "Source").await;
        let scope = ActorScope::user(UserId::new(owner));
        let page_scope = ActorScope::user(UserId::new(page_owner));
        let store = ByteStore::start().await;
        *store.state.fault.lock().await = match index {
            0 => Fault::WriteDenied,
            1 => Fault::CorruptRead,
            2 => Fault::ArchiveDuringRead {
                pool: pool.clone(),
                scope: scope.clone(),
                page: page.clone(),
            },
            3 => Fault::RevokeMembershipDuringRead {
                pool: pool.clone(),
                user: owner,
                bear,
            },
            _ => Fault::RestrictPageDuringRead {
                pool: pool.clone(),
                owner: page_owner,
                page: page.clone(),
            },
        };
        let app = store.app(&pool).await;
        let cookie = login_cookie(&app, owner).await;
        let response = send(
            &app,
            &cookie,
            &page,
            form(bear, "Failed file.txt", b"actual uploaded bytes", false, ""),
        )
        .await;
        assert!(
            !response.status().is_success() && !response.status().is_redirection(),
            "fault {index} unexpectedly published"
        );
        assert!(
            store.state.objects.lock().await.is_empty(),
            "fault {index} left bytes without cleanup"
        );
        let rows = sqlx::query!("SELECT artifact_ref,lifecycle FROM artifacts WHERE created_by_user_id=$1 AND kind='cabinet_file'", owner).fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].lifecycle, "deleted");
        let reference = ArtifactRef::parse(&rows[0].artifact_ref).unwrap();
        assert!(artifacts::authorize_for_reader(
            &pool,
            &reference,
            ArtifactReader::Human(UserId::new(owner)),
            ArtifactAccessLevel::Content
        )
        .await
        .is_err());
        assert!(cabinet::attachments::list(&pool, &page_scope, &page)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn cabinet_pending_uploads_cannot_be_read_or_linked_and_cleanup_never_deletes_published_files(
) {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Pending upload", "Source").await;
    let scope = ActorScope::user(UserId::new(owner));
    let bytes = b"receipt bytes";
    let pending = cabinet::uploads::prepare(
        &pool,
        &scope,
        &page,
        cabinet::uploads::UploadInput {
            bear_id: BearId::new(bear),
            title: "Receipt.txt".into(),
            content_type: "text/plain".into(),
            bytes,
            role: AttachmentRole::Data,
            audience: cabinet::uploads::UploadAudience::Private,
        },
    )
    .await
    .unwrap();
    let reference = ArtifactRef::parse(&pending.location().artifact_ref).unwrap();
    assert!(artifacts::authorize_for_reader(
        &pool,
        &reference,
        ArtifactReader::Human(UserId::new(owner)),
        ArtifactAccessLevel::Content
    )
    .await
    .is_err());
    assert!(
        cabinet::attachments::link(&pool, &scope, &page, &reference, AttachmentRole::Data)
            .await
            .is_err()
    );
    let store = ByteStore::start().await;
    let mut config = crate::config::Config::test_stub();
    config.s3_endpoint = store.endpoint.clone();
    config.s3_bucket = "cabinet-fixture".into();
    config.s3_force_path_style = true;
    let media = crate::core::s3::MediaStore::new(&config).unwrap();
    media.write_artifact(&pool, &pending, bytes).await.unwrap();
    cabinet::uploads::publish(&pool, &scope, &pending)
        .await
        .unwrap();
    assert!(!cabinet::uploads::abandon(&pool, &pending).await.unwrap());
    assert!(cabinet::uploads::publish(&pool, &scope, &pending)
        .await
        .is_err());
    assert_eq!(
        media.read_artifact(pending.location()).await.unwrap(),
        bytes
    );
}
