use super::*;
mod transport;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use den_core::ids::UserId;
use secrecy::SecretString;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

const TOKEN: &str = "ghp_runtime_canary_NEVER_PROJECT";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Authority {
    snapshot: AuthorizedHead,
    allowed: Arc<AtomicBool>,
    calls: AtomicUsize,
    deny_on: usize,
    change_on: usize,
}
#[async_trait]
impl RepositoryAuthorizer for Authority {
    async fn authorize(&self, _: RepositorySurfaceId) -> Result<AuthorizedHead, RepositoryError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.allowed.load(Ordering::SeqCst) || call == self.deny_on {
            return Err(RepositoryError::NotAuthorized);
        }
        let mut snapshot = self.snapshot.clone();
        if call == self.change_on {
            snapshot.grant_id = Uuid::new_v4();
        }
        Ok(snapshot)
    }
}
struct Resolver {
    calls: AtomicUsize,
    validations: AtomicUsize,
    revoke_authority: Option<Arc<AtomicBool>>,
    mismatch: bool,
    revoked_after_io: bool,
}
#[async_trait]
impl ExternalCredentialResolver for Resolver {
    async fn resolve(
        &self,
        request: &CredentialRequest,
    ) -> Result<CredentialLease, RepositoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut request = request.clone();
        if self.mismatch {
            request.owner = UserId::new(request.owner.get() + 1);
        }
        if let Some(allowed) = &self.revoke_authority {
            allowed.store(false, Ordering::SeqCst);
        }
        CredentialLease::new(
            request,
            SecretString::from(TOKEN.to_owned()),
            Duration::from_secs(30),
        )
    }
    async fn validate(&self, _: &CredentialLease) -> Result<(), RepositoryError> {
        let call = self.validations.fetch_add(1, Ordering::SeqCst) + 1;
        if self.revoked_after_io && call == 3 {
            return Err(RepositoryError::CredentialRevoked);
        }
        Ok(())
    }
}
fn fixture() -> (Authority, Resolver) {
    let snapshot = AuthorizedHead {
        surface: RepositorySurfaceId(Uuid::new_v4()),
        credential: CredentialRequest {
            connection_id: Uuid::new_v4(),
            owner: UserId::new(12),
            connection_revision: 1,
            reference: ExternalReference::new(Uuid::new_v4(), Uuid::new_v4(), 1).unwrap(),
            repository: GithubRepository::parse("https://github.com/acme/widget", "main").unwrap(),
        },
        source_id: Uuid::new_v4(),
        hat_id: Uuid::new_v4(),
        grant_id: Uuid::new_v4(),
    };
    (
        Authority {
            snapshot,
            allowed: Arc::new(AtomicBool::new(true)),
            calls: AtomicUsize::new(0),
            deny_on: 0,
            change_on: 0,
        },
        Resolver {
            calls: AtomicUsize::new(0),
            validations: AtomicUsize::new(0),
            revoke_authority: None,
            mismatch: false,
            revoked_after_io: false,
        },
    )
}

#[derive(Clone)]
struct Provider {
    calls: Arc<AtomicUsize>,
    authenticated: Arc<AtomicBool>,
    status: StatusCode,
    body: String,
}
async fn provider(
    State(state): State<Provider>,
    uri: Uri,
    headers: HeaderMap,
) -> impl IntoResponse {
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.authenticated.store(
        headers
            .get("authorization")
            .is_some_and(|value| value == format!("Bearer {TOKEN}").as_str())
            && uri.path() == "/repos/acme/widget/git/ref/heads/main",
        Ordering::SeqCst,
    );
    (
        state.status,
        [
            ("location", "http://127.0.0.1/credential-escape"),
            ("x-provider-echo", TOKEN),
        ],
        state.body,
    )
}
async fn server(
    status: StatusCode,
    body: String,
) -> (std::net::SocketAddr, Provider, tokio::task::JoinHandle<()>) {
    let state = Provider {
        calls: Arc::new(AtomicUsize::new(0)),
        authenticated: Arc::new(AtomicBool::new(false)),
        status,
        body,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .fallback(get(provider))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, state, task)
}
fn response() -> String {
    serde_json::json!({"ref":"refs/heads/main", "object":{"type":"commit", "sha":SHA}, "message":TOKEN, "url":TOKEN}).to_string()
}

#[tokio::test]
async fn real_http_authentication_returns_only_the_business_projection_and_replay_rechecks() {
    let (authority, resolver) = fixture();
    let (address, provider, task) = server(StatusCode::OK, response()).await;
    for _ in 0..2 {
        let result = execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address),
        )
        .await
        .unwrap();
        let projected = serde_json::to_string(&result).unwrap();
        assert!(!projected.contains(TOKEN));
        assert!(!projected.contains("backend"));
        assert!(!projected.contains("ref"));
        assert_eq!(
            serde_json::to_value(&result)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(result.commit_sha.as_str(), SHA);
        assert_model_projection(&result);
    }
    assert!(provider.authenticated.load(Ordering::SeqCst));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(authority.calls.load(Ordering::SeqCst), 8);
    authority.allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address)
        )
        .await
        .err(),
        Some(RepositoryError::NotAuthorized)
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn denied_or_substituted_scope_never_reaches_the_provider() {
    for stage in [1, 2, 3] {
        let (mut authority, resolver) = fixture();
        authority.deny_on = stage;
        let (address, provider, task) = server(StatusCode::OK, response()).await;
        assert_eq!(
            execute(
                &authority,
                &resolver,
                authority.snapshot.surface,
                http::Transport::Loopback(address)
            )
            .await
            .err(),
            Some(RepositoryError::NotAuthorized)
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            resolver.calls.load(Ordering::SeqCst),
            usize::from(stage != 1)
        );
        task.abort();
    }
    let (authority, mut resolver) = fixture();
    resolver.mismatch = true;
    let (address, provider, task) = server(StatusCode::OK, response()).await;
    assert_eq!(
        execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address)
        )
        .await
        .err(),
        Some(RepositoryError::CredentialScopeMismatch)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn revocation_during_lookup_and_io_suppresses_delivery() {
    let (authority, mut resolver) = fixture();
    resolver.revoke_authority = Some(authority.allowed.clone());
    let (address, provider, task) = server(StatusCode::OK, response()).await;
    assert_eq!(
        execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address)
        )
        .await
        .err(),
        Some(RepositoryError::NotAuthorized)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    task.abort();
    for backend_revoke in [false, true] {
        let (mut authority, mut resolver) = fixture();
        authority.deny_on = if backend_revoke { 0 } else { 4 };
        resolver.revoked_after_io = backend_revoke;
        let (address, provider, task) = server(StatusCode::OK, response()).await;
        let result = execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address),
        )
        .await;
        assert_eq!(
            result.err(),
            Some(if backend_revoke {
                RepositoryError::CredentialRevoked
            } else {
                RepositoryError::NotAuthorized
            })
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn changes_at_every_revalidation_are_not_new_authority() {
    for stage in [2, 3, 4] {
        let (mut authority, resolver) = fixture();
        authority.change_on = stage;
        let (address, provider, task) = server(StatusCode::OK, response()).await;
        assert_eq!(
            execute(
                &authority,
                &resolver,
                authority.snapshot.surface,
                http::Transport::Loopback(address)
            )
            .await
            .err(),
            Some(RepositoryError::ResourceChanged)
        );
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            usize::from(stage == 4)
        );
        task.abort();
    }
}

#[tokio::test]
async fn redirect_errors_oversize_and_provider_echoes_are_closed_failures() {
    for (status, body, expected) in [
        (
            StatusCode::FOUND,
            TOKEN.to_owned(),
            RepositoryError::DestinationDenied,
        ),
        (
            StatusCode::UNAUTHORIZED,
            TOKEN.to_owned(),
            RepositoryError::ProviderAuthenticationFailed,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            TOKEN.to_owned(),
            RepositoryError::RateLimited,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            TOKEN.to_owned(),
            RepositoryError::ProviderUnavailable,
        ),
        (
            StatusCode::OK,
            "x".repeat(16 * 1024 + 1),
            RepositoryError::ResponseTooLarge,
        ),
        (
            StatusCode::OK,
            TOKEN.to_owned(),
            RepositoryError::InvalidProviderResponse,
        ),
    ] {
        let (authority, resolver) = fixture();
        let (address, provider, task) = server(status, body).await;
        let error = execute(
            &authority,
            &resolver,
            authority.snapshot.surface,
            http::Transport::Loopback(address),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[test]
fn every_reference_and_scope_dimension_changes_the_target_fingerprint() {
    let (authority, _) = fixture();
    let request = &authority.snapshot.credential;
    let key = AuthorizedHead::target_key(authority.snapshot.surface, request);
    let mut changed = request.clone();
    changed.reference = ExternalReference::new(
        request.reference.backend_binding_id(),
        request.reference.secret_id(),
        2,
    )
    .unwrap();
    assert_ne!(
        key,
        AuthorizedHead::target_key(authority.snapshot.surface, &changed)
    );
    changed = request.clone();
    changed.owner = UserId::new(13);
    assert_ne!(
        key,
        AuthorizedHead::target_key(authority.snapshot.surface, &changed)
    );
    changed = request.clone();
    changed.connection_revision += 1;
    assert_ne!(
        key,
        AuthorizedHead::target_key(authority.snapshot.surface, &changed)
    );
    changed = request.clone();
    changed.repository = GithubRepository::parse("https://github.com/acme/other", "main").unwrap();
    assert_ne!(
        key,
        AuthorizedHead::target_key(authority.snapshot.surface, &changed)
    );
    let lease = CredentialLease::new(
        changed,
        SecretString::from(TOKEN.to_owned()),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(
        lease.check(request),
        Err(RepositoryError::CredentialScopeMismatch)
    );
    for reference in [
        ExternalReference::new(
            Uuid::new_v4(),
            request.reference.secret_id(),
            request.reference.version(),
        )
        .unwrap(),
        ExternalReference::new(
            request.reference.backend_binding_id(),
            Uuid::new_v4(),
            request.reference.version(),
        )
        .unwrap(),
        ExternalReference::new(
            request.reference.backend_binding_id(),
            request.reference.secret_id(),
            2,
        )
        .unwrap(),
    ] {
        let mut substituted = request.clone();
        substituted.reference = reference;
        let lease = CredentialLease::new(
            substituted,
            SecretString::from(TOKEN.to_owned()),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(
            lease.check(request),
            Err(RepositoryError::CredentialScopeMismatch)
        );
    }
}

fn assert_model_projection(result: &RepositoryHeadResult) {
    use den_core::tools::{
        descriptor::builtin_den_tool_descriptor_for_provider_name,
        result_compaction::compact_json_tool_result,
    };
    use den_llm::{
        ChatCompletionRequest, ChatMessage, ChatToolCall, ChatToolCallFunction, LlmToolDefinition,
    };
    let descriptor = builtin_den_tool_descriptor_for_provider_name("repository_head").unwrap();
    let mut request = ChatCompletionRequest {
        model: "openai/test".into(),
        messages: vec![],
        tools: vec![LlmToolDefinition {
            name: descriptor.provider_name.clone(),
            description: Some(descriptor.description.into()),
            parameters: descriptor.input_schema,
        }],
        stream: false,
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        thinking_effort: None,
        telemetry: None,
    };
    assert!(!request.to_body().to_string().contains(TOKEN));
    let compacted = compact_json_tool_result(serde_json::to_value(result).unwrap());
    request.messages = vec![
        ChatMessage {
            role: "assistant".into(),
            content: None,
            tool_call_id: None,
            name: None,
            tool_calls: Some(vec![ChatToolCall {
                id: "head-call".into(),
                call_type: "function".into(),
                function: ChatToolCallFunction {
                    name: "repository_head".into(),
                    arguments: serde_json::json!({"work_surface_id":result.work_surface_id})
                        .to_string(),
                },
            }]),
        },
        ChatMessage {
            role: "tool".into(),
            content: Some(compacted.content),
            tool_call_id: Some("head-call".into()),
            name: Some("repository_head".into()),
            tool_calls: None,
        },
    ];
    for body in [request.to_body(), request.to_responses_body()] {
        let body = body.to_string();
        assert!(body.contains(SHA));
        assert!(!body.contains(TOKEN));
        assert!(!body.contains("ghp_runtime_canary"));
    }
    assert!(!compacted.payload.to_string().contains(TOKEN));
}

#[tokio::test(start_paused = true)]
async fn total_timeout_includes_an_unresponsive_backend() {
    struct Stalled;
    #[async_trait]
    impl ExternalCredentialResolver for Stalled {
        async fn resolve(&self, _: &CredentialRequest) -> Result<CredentialLease, RepositoryError> {
            std::future::pending().await
        }
        async fn validate(&self, _: &CredentialLease) -> Result<(), RepositoryError> {
            unreachable!()
        }
    }
    let (authority, _) = fixture();
    assert_eq!(
        repository_head(&authority, &Stalled, authority.snapshot.surface)
            .await
            .err(),
        Some(RepositoryError::Timeout)
    );
}

#[tokio::test]
async fn production_default_never_falls_back() {
    let (authority, _) = fixture();
    assert_eq!(
        repository_head(&authority, &Unavailable, authority.snapshot.surface)
            .await
            .err(),
        Some(RepositoryError::CredentialUnavailable)
    );
}
