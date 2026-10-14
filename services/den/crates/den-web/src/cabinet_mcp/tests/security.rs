#[path = "attachments.rs"]
mod attachments;
#[path = "sdk.rs"]
mod sdk;

use super::{auth, protocol, tools};
use axum::{
    body::to_bytes,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use den_cabinet::{
    ActorScope, AttachmentRole, CabinetAttachmentRef, CabinetItemRef, CabinetPolicy,
    CreateItemRequest, ItemKind, ReviewDecision, ReviewRequest, ReviewState, UpdateItemRequest,
};
use den_core::ids::UserId;
use den_oauth::oauth::{db as oauth, jwt::JwtManager, OAuthScope};
use den_service::{
    artifacts::{bytes::ArtifactByteReader, bytes::ArtifactReadFuture, ArtifactContentLocation},
    bears::db::{self as bears, BearParams},
    cabinet,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn user(pool: &PgPool, prefix: &str) -> i32 {
    let name = format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..16]);
    den_http::user::db::create_user(
        pool,
        &format!("{name}@example.test"),
        &name,
        prefix,
        "unused-test-passhash",
    )
    .await
    .unwrap()
}

async fn bear(pool: &PgPool) -> Uuid {
    bears::create_bear(
        pool,
        BearParams {
            slug: &format!("mcp-{}", Uuid::new_v4().simple()),
            name: "MCP test Bear",
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

async fn page(pool: &PgPool, owner: i32) -> cabinet::ItemView {
    cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(UserId(owner)),
            kind: ItemKind::Document,
            title: "MCP test page".into(),
            content: "Published content".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: Vec::new(),
        },
    )
    .await
    .unwrap()
}

fn call(name: &str, arguments: Value) -> tools::Call {
    tools::parse_call(json!({"name": name, "arguments": arguments})).unwrap()
}

async fn payload(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres; do not run against the live stack"]
async fn immutable_reads_and_history_apply_current_page_and_review_authority(pool: PgPool) {
    let owner = user(&pool, "owner").await;
    let reader = user(&pool, "reader").await;
    let reviewer = user(&pool, "reviewer").await;
    let bear = bear(&pool).await;
    let owner_scope = ActorScope::user(UserId(owner));
    let reader_scope = ActorScope::user(UserId(reader));
    let reviewer_scope = ActorScope::user(UserId(reviewer));
    let item = page(&pool, owner).await;
    let page = item.item.cabinet_ref;
    let published = item.version.version_ref().clone();
    let policy = CabinetPolicy {
        review_required: true,
        ..CabinetPolicy::default()
    };
    cabinet::pages::configure(
        &pool,
        &owner_scope,
        &page,
        policy,
        &[owner, reader, reviewer],
        &[bear],
        &[reviewer],
    )
    .await
    .unwrap();
    let mut proposal = None;
    // More than a full page of hidden revisions must not displace the published
    // revision, reveal their existence through `truncated`, or expose hashes.
    for _ in 0..101 {
        let pending = cabinet::update_item(
            &pool,
            UpdateItemRequest {
                scope: ActorScope::bear(bear.into(), "work".parse().unwrap()),
                cabinet_ref: page.clone(),
                content: "PRIVATE PROPOSAL CONTENT".into(),
                base_version: published.clone(),
                title: Some("PRIVATE PROPOSAL TITLE".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(pending.version.review(), ReviewState::Pending);
        proposal = Some(pending.version.version_ref().clone());
    }
    let proposal = proposal.unwrap();
    let history = || call("cabinet_history", json!({"cabinet_ref": page}));
    let result = tools::dispatch(&pool, None, &reader_scope, history())
        .await
        .unwrap();
    assert_eq!(result["structuredContent"]["truncated"], false);
    let versions = result["structuredContent"]["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["version_ref"], json!(published));
    assert!(!result.to_string().contains(proposal.as_str()));
    let result = tools::dispatch(&pool, None, &reviewer_scope, history())
        .await
        .unwrap();
    assert_eq!(result["structuredContent"]["truncated"], true);
    assert_eq!(
        result["structuredContent"]["versions"]
            .as_array()
            .unwrap()
            .len(),
        100
    );

    let read = || {
        call(
            "cabinet_read",
            json!({"cabinet_ref": page, "version_ref": proposal}),
        )
    };
    let denied = tools::dispatch(&pool, None, &reader_scope, read())
        .await
        .unwrap();
    assert_eq!(denied["isError"], true);
    assert!(!denied.to_string().contains("PRIVATE PROPOSAL"));
    assert_eq!(
        tools::dispatch(&pool, None, &reviewer_scope, read())
            .await
            .unwrap()["isError"],
        false
    );
    cabinet::pages::review(
        &pool,
        ReviewRequest {
            scope: reviewer_scope.clone(),
            cabinet_ref: page.clone(),
            version_ref: proposal.clone(),
            decision: ReviewDecision::Rejected,
            rationale: "Test rejection".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        tools::dispatch(&pool, None, &reader_scope, read())
            .await
            .unwrap()["isError"],
        true
    );
    cabinet::pages::configure(
        &pool,
        &owner_scope,
        &page,
        CabinetPolicy {
            review_required: true,
            ..CabinetPolicy::default()
        },
        &[owner, reader, reviewer],
        &[bear],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        tools::dispatch(&pool, None, &reviewer_scope, read())
            .await
            .unwrap()["isError"],
        true
    );
    assert_eq!(
        tools::dispatch(&pool, None, &reviewer_scope, history())
            .await
            .unwrap()["structuredContent"]["versions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    cabinet::pages::configure(
        &pool,
        &owner_scope,
        &page,
        CabinetPolicy::default(),
        &[owner],
        &[],
        &[],
    )
    .await
    .unwrap();
    let published_read = call(
        "cabinet_read",
        json!({"cabinet_ref": page, "version_ref": published}),
    );
    assert_eq!(
        tools::dispatch(&pool, None, &reader_scope, published_read)
            .await
            .unwrap()["isError"],
        true
    );
    assert_eq!(
        tools::dispatch(&pool, None, &reader_scope, history())
            .await
            .unwrap()["isError"],
        true
    );
}

struct Fixture {
    pool: PgPool,
    owner: ActorScope,
    user: i32,
    client: i32,
    token: String,
    page: CabinetItemRef,
    attachment: CabinetAttachmentRef,
    bear: Uuid,
}

impl Fixture {
    async fn new(pool: &PgPool) -> Self {
        let owner = user(pool, "owner").await;
        let user = user(pool, "delegate").await;
        let bear = bear(pool).await;
        bears::grant_membership(pool, owner, bear, Some("admin"))
            .await
            .unwrap();
        bears::grant_membership(pool, user, bear, None)
            .await
            .unwrap();
        let page = page(pool, owner).await.item.cabinet_ref;
        let owner = ActorScope::user(UserId(owner));
        let pending = cabinet::uploads::prepare(
            pool,
            &owner,
            &page,
            cabinet::uploads::UploadInput {
                bear_id: bear.into(),
                title: "text.txt".into(),
                content_type: "text/plain".into(),
                bytes: b"PRIVATE ATTACHMENT BYTES",
                role: AttachmentRole::Other,
                audience: cabinet::uploads::UploadAudience::BearAndMembers,
            },
        )
        .await
        .unwrap();
        let attachment = cabinet::uploads::publish(pool, &owner, &pending)
            .await
            .unwrap();
        let client_name = format!("mcp-{user}");
        let redirects = json!(["http://127.0.0.1/callback"]);
        let client = oauth::create_oauth_client(
            pool,
            &client_name,
            None,
            "MCP client",
            &redirects,
            &[OAuthScope::DataRead],
            true,
        )
        .await
        .unwrap();
        let manager = JwtManager::new(
            std::env::var("JWT_SECRET")
                .expect("set a test-only JWT_SECRET for HTTP boundary tests")
                .as_bytes(),
        );
        let expiry = OffsetDateTime::now_utc() + Duration::hours(1);
        let token = manager
            .generate_access_token(user, &client_name, &[OAuthScope::DataRead], expiry)
            .unwrap();
        oauth::create_access_token(pool, &token, client, user, &[OAuthScope::DataRead], expiry)
            .await
            .unwrap();
        Self {
            pool: pool.clone(),
            owner,
            user,
            client,
            token,
            page,
            attachment,
            bear,
        }
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token)).unwrap(),
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static("2025-06-18"),
        );
        headers
    }

    async fn request(
        &self,
        method: Method,
        reader: Option<&dyn ArtifactByteReader>,
        message: Value,
    ) -> Response {
        super::serve(
            &self.pool,
            reader,
            &method,
            &Uri::from_static("/mcp"),
            &self.headers(),
            message.to_string().as_bytes(),
        )
        .await
    }
}

#[derive(Clone, Copy, Debug)]
enum Revoke {
    None,
    Token,
    Client,
    ClientScope,
    User,
    Page,
    Attachment,
    FileMembership,
}

struct ReadHook<'a> {
    fixture: &'a Fixture,
    revoke: Revoke,
    called: std::sync::atomic::AtomicBool,
}
impl ArtifactByteReader for ReadHook<'_> {
    fn read<'a>(&'a self, _: &'a ArtifactContentLocation) -> ArtifactReadFuture<'a> {
        Box::pin(async move {
            let f = self.fixture;
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            match self.revoke {
                Revoke::None => {}
                Revoke::Token => oauth::revoke_access_token(&f.pool, &f.token).await.unwrap(),
                Revoke::Client => oauth::deactivate_oauth_client(&f.pool, f.client)
                    .await
                    .unwrap(),
                Revoke::ClientScope => oauth::update_oauth_client(
                    &f.pool,
                    f.client,
                    "MCP client",
                    &json!(["http://127.0.0.1/callback"]),
                    &[OAuthScope::ProfileRead],
                )
                .await
                .unwrap(),
                Revoke::User => den_http::user::db::deletion::delete_user(&f.pool, UserId(f.user))
                    .await
                    .unwrap(),
                Revoke::Page => {
                    let den_cabinet::Actor::User { user_id } = f.owner.actor else {
                        unreachable!()
                    };
                    cabinet::pages::configure(
                        &f.pool,
                        &f.owner,
                        &f.page,
                        CabinetPolicy::default(),
                        &[user_id.get()],
                        &[],
                        &[],
                    )
                    .await
                    .unwrap();
                }
                Revoke::Attachment => {
                    cabinet::attachments::unlink(&f.pool, &f.owner, &f.page, &f.attachment)
                        .await
                        .unwrap();
                }
                Revoke::FileMembership => bears::revoke_membership(&f.pool, f.user, f.bear)
                    .await
                    .unwrap(),
            }
            Ok(b"PRIVATE ATTACHMENT BYTES".to_vec())
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres and test-only JWT_SECRET; do not run against the live stack"]
async fn credentials_and_independent_acls_are_rechecked_after_attachment_io(pool: PgPool) {
    for revoke in [
        Revoke::None,
        Revoke::Token,
        Revoke::Client,
        Revoke::ClientScope,
        Revoke::User,
        Revoke::Page,
        Revoke::Attachment,
        Revoke::FileMembership,
    ] {
        let f = Fixture::new(&pool).await;
        let hook = ReadHook {
            fixture: &f,
            revoke,
            called: false.into(),
        };
        assert!(auth::authenticate(&pool, &f.headers()).await.is_ok());
        let response = f.request(Method::POST, Some(&hook), json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cabinet_read","arguments":{"cabinet_ref":f.page,"attachment_ref":f.attachment}}})).await;
        assert!(
            hook.called.load(std::sync::atomic::Ordering::SeqCst),
            "{revoke:?}"
        );
        let expected = match revoke {
            Revoke::Token | Revoke::Client | Revoke::User => StatusCode::UNAUTHORIZED,
            Revoke::ClientScope => StatusCode::FORBIDDEN,
            _ => StatusCode::OK,
        };
        assert_eq!(response.status(), expected, "{revoke:?}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = payload(response).await;
        if matches!(revoke, Revoke::None) {
            assert_eq!(
                body["result"]["structuredContent"]["attachment"]["text"],
                "PRIVATE ATTACHMENT BYTES"
            );
        } else {
            assert!(
                !body.to_string().contains("PRIVATE ATTACHMENT BYTES"),
                "{revoke:?}"
            );
            if expected == StatusCode::OK {
                assert_eq!(body["result"]["isError"], true, "{revoke:?}");
            }
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires isolated Postgres and test-only JWT_SECRET; do not run against the live stack"]
async fn authenticated_http_handshake_and_every_method_reload_revocation(pool: PgPool) {
    let f = Fixture::new(&pool).await;
    let response = f.request(Method::POST, None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"native-client","version":"1","user_id":"spoofed"}}})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        payload(response).await["result"]["protocolVersion"],
        "2025-06-18"
    );
    let response = f
        .request(
            Method::POST,
            None,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(to_bytes(response.into_body(), 100)
        .await
        .unwrap()
        .is_empty());
    for method in [Method::GET, Method::DELETE, Method::HEAD, Method::OPTIONS] {
        let response = f.request(method, None, json!({})).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::ALLOW], "POST");
    }
    assert_eq!(
        payload(
            f.request(
                Method::POST,
                None,
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
            )
            .await
        )
        .await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    oauth::revoke_access_token(&pool, &f.token).await.unwrap();
    for method in [
        Method::POST,
        Method::GET,
        Method::DELETE,
        Method::HEAD,
        Method::OPTIONS,
    ] {
        let response = f
            .request(
                method,
                None,
                json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    }
    // The protocol-only helper is deliberately not the authentication boundary.
    assert_eq!(
        protocol::version(&f.headers()).unwrap(),
        protocol::Version::June2025
    );
}
