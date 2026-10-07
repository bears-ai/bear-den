use axum::http::{header, HeaderMap};
use bearwire_protocol::session::ExpectedWorkSource;
use den_core::ids::{BearId, HatId, UserId};
use den_docket::{
    work_runs::{self, WorkExecutionTarget, WorkJobEnqueue, WorkRunProvisioned},
    DocketCommitPolicy, DocketCriterionKind, DocketEffortHint, DocketJobCreate,
    DocketJobCreationAuthority, DocketJobCriterionInput, DocketJobOverlapResolution,
    DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskScope, PgDocketService,
    RoutingStrategy, TaskListVisibility,
};
use den_http::armature_tokens::{self, CreatedArmatureToken};
use den_service::{
    bears::{db as bears_db, db::BearParams, hats},
    work_surfaces::{self, NewWorkSurface},
    DenState,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

pub(super) async fn user(pool: &PgPool) -> UserId {
    let name = format!("ews{}", &Uuid::new_v4().simple().to_string()[..16]);
    UserId::new(
        den_http::user::db::create_user(
            pool,
            &format!("{name}@example.test"),
            &name,
            "Expected Work source test",
            "unused",
        )
        .await
        .unwrap(),
    )
}

pub(super) struct Fixture {
    pub bear: BearId,
    pub slug: String,
    pub user: UserId,
    pub token: CreatedArmatureToken,
    pub session: String,
    pub expected: ExpectedWorkSource,
    pub hat: HatId,
    pub job: Uuid,
    pub task: Uuid,
}

impl Fixture {
    pub async fn new(pool: &PgPool) -> Self {
        let user = user(pool).await;
        let slug = format!("expected-work-{}", Uuid::new_v4().simple());
        let bear = BearId::new(
            bears_db::create_bear(
                pool,
                BearParams {
                    slug: &slug,
                    name: "Expected Work source test",
                    description: "Exact checkout regression fixture",
                    system_prompt: "test",
                    default_model: None,
                    tools_enabled: None,
                    context_profile: None,
                },
            )
            .await
            .unwrap(),
        );
        bears_db::grant_membership(
            pool,
            user.get(),
            bear.as_uuid(),
            Some(bears_db::BEAR_ROLE_ADMIN),
        )
        .await
        .unwrap();
        let ide_hat = hats::create_hat(pool, bear, user, "IDE default", "Ordinary collaboration")
            .await
            .unwrap();
        hats::set_ide_default_hat(pool, bear, ide_hat.id)
            .await
            .unwrap();
        let token =
            armature_tokens::create_for_bear(pool, user.get(), bear.as_uuid(), "Dispatched Work")
                .await
                .unwrap();
        let session = format!("expected-work-{}", Uuid::new_v4().simple());
        let (expected, hat, job, task) = checkout(pool, bear, user, token.id, &session).await;
        Self {
            bear,
            slug,
            user,
            token,
            session,
            expected,
            hat,
            job,
            task,
        }
    }

    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", self.token.raw_token).parse().unwrap(),
        );
        headers
    }
}

pub(super) async fn checkout(
    pool: &PgPool,
    bear: BearId,
    user: UserId,
    token_id: Uuid,
    session: &str,
) -> (ExpectedWorkSource, HatId, Uuid, Uuid) {
    let surface = work_surfaces::create_surface(
        pool,
        user.get(),
        NewWorkSurface {
            name: format!("expected-work-{}", Uuid::new_v4().simple()),
            description: None,
            upstream_url: "https://example.test/expected-work.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        "unused-no-credential",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(pool, surface.id, bear.as_uuid(), user.get())
        .await
        .unwrap();
    let hat = hats::create_hat(
        pool,
        bear,
        user,
        &format!("Work {}", surface.id),
        "Execute assigned surface",
    )
    .await
    .unwrap();
    hats::allow_surface(pool, bear, hat.id, surface.id)
        .await
        .unwrap();
    // Reuse the existing checkout fixture's cached SQL; production enablement also checks memory.
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.id.as_uuid()
    )
    .execute(pool)
    .await
    .unwrap();
    let job = PgDocketService::from_pool(pool)
        .create_job_with_hat(
            DocketJobCreate {
                bear_id: bear.as_uuid(),
                created_by_user_id: user.get(),
                created_by_role: "pair".into(),
                goal: "Execute this exact checked-out source".into(),
                work_surface_id: Some(surface.id),
                work_surface_assignments: vec![],
                commit_policy: Some(DocketCommitPolicy::PerTask),
                work_branch: None,
                visibility: TaskListVisibility::SameUser,
                source_conversation_id: None,
                objective_kind: None,
                supersedes_job_id: None,
                overlap_resolution: DocketJobOverlapResolution::Reject,
                criteria: vec![DocketJobCriterionInput {
                    kind: DocketCriterionKind::Narrative,
                    description: "Exact source executes".into(),
                    spec: None,
                    sibling_order: 0,
                }],
                tasks: vec![DocketTaskInput {
                    client_key: None,
                    parent_client_key: None,
                    parent_task_id: None,
                    sibling_order: Some(0),
                    kind: DocketTaskKind::Execution,
                    scope: DocketTaskScope::Template,
                    title: "Checked-out Work".into(),
                    body: "Work task".into(),
                    completion_criteria: vec!["Work completes".into()],
                    difficulty: Some(DocketTaskDifficulty::Trivial),
                    effort_hint: Some(DocketEffortHint::Low),
                    routing_strategy: RoutingStrategy::Auto,
                    expected_context_size: None,
                    result_rollup_policy: None,
                }],
            },
            hat.id,
            DocketJobCreationAuthority::HumanRequest,
        )
        .await
        .unwrap();
    let runs = work_runs::enqueue_work_job(
        pool,
        WorkJobEnqueue {
            bear_id: bear.as_uuid(),
            job_id: job.job.id,
            durable_result: den_docket::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user.get()),
            execution_target: WorkExecutionTarget::Sandbox,
            attachment_warning: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    let claimed = work_runs::claim_next_work_run(
        pool,
        "expected-work-test",
        std::time::Duration::from_secs(60),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(claimed.id, run.id);
    work_runs::record_work_run_provisioned(
        pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox.test".into(),
            sandbox_id: "expected-work-test".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "test".into(),
            work_surface: json!({"is_git": true}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();
    work_runs::merge_work_run_result_refs(
        pool,
        run.id,
        &json!({
            "armature_token_id": token_id,
            "armature_token_user_id": user.get(),
        }),
    )
    .await
    .unwrap();
    let checked = work_runs::checkout_work_run_for_session(pool, run.id, bear.as_uuid(), session)
        .await
        .unwrap();
    let attempt = checked
        .execution_attempt
        .expect("checkout admitted an execution attempt");
    (
        ExpectedWorkSource {
            work_run_id: run.id,
            execution_attempt_id: attempt.id,
            fence_epoch: attempt.fence_epoch,
        },
        hat.id,
        run.job_id,
        attempt.task_id,
    )
}

pub(super) async fn model_ready_state(pool: &PgPool, fixture: &Fixture) -> DenState {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        assert!(request.starts_with("GET /models"));
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "catalog request ended before its headers"
            );
            if line == "\r\n" {
                break;
            }
        }
        let body = r#"{"data":[{"id":"openai/bearwire-test-model","owned_by":"openai","context_length":128000,"max_output_tokens":4096,"supported_parameters":["tools"],"supported_methods":["chat_completion"]}]}"#;
        write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let mut config = den_core::config::Config::test_stub();
    config.llm_api_url = format!("http://{address}");
    config.default_llm_model = "openai/bearwire-test-model".into();
    config.den_secret_encryption_key = "expected-source-test-secret".into();
    bears_db::set_bear_bifrost_virtual_key(
        pool,
        fixture.bear.as_uuid(),
        Some("vk-test"),
        Some("Startup test"),
        Some("sk-bf-startup-test"),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    let config = std::sync::Arc::new(config);
    let state = DenState::new(
        pool.clone(),
        config.clone(),
        std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
        den_memory::MemoryStoreManager::new(config.as_ref()),
    );
    let catalog = state
        .bifrost
        .bear_catalog_snapshot(
            pool,
            fixture.bear.as_uuid(),
            &config.den_secret_encryption_key,
        )
        .await
        .unwrap();
    assert!(catalog.resolve("openai/bearwire-test-model").is_some());
    server.join().unwrap();
    state
}

pub(super) fn state(pool: PgPool) -> DenState {
    let config = std::sync::Arc::new(den_core::config::Config::test_stub());
    DenState::new(
        pool,
        config.clone(),
        std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
        den_memory::MemoryStoreManager::new(config.as_ref()),
    )
}
