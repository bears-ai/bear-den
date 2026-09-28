use den_core::{BearCapability, CapabilitySet};
use den_docket::{
    DocketCommitPolicy, DocketCriterionKind, DocketEffortHint, DocketJobCreate,
    DocketJobCriterionInput, DocketJobOverlapResolution, DocketService, DocketTaskCreate,
    DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskScope, PgDocketService,
    RoutingStrategy, TaskListVisibility,
};
use den_http::errors::CustomError;
use den_runtime::current_task::{
    preview_session_current_task_selection, select_session_current_task,
};
use den_service::{
    bears::db::{create_bear, grant_membership, BearParams, BEAR_ROLE_MEMBER},
    client_sessions::{self, UpsertClientSession},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_user(pool: &PgPool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("task{}", &suffix[..20]);
    sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
        username
    )
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_session(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    bear_slug: &str,
) -> (String, Uuid) {
    let session_id = format!("session-{}", Uuid::new_v4());
    client_sessions::upsert_session(
        pool,
        UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: bear_slug.to_string(),
            client_session_id: session_id.clone(),
            runtime_session_id: session_id.clone(),
            conversation_id: format!("conversation-{session_id}"),
            resolved_conversation_id: None,
            client: "integration-test".to_string(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .expect("seed session");
    let id = client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists")
        .id;
    (session_id, id)
}

async fn seed_surface(pool: &PgPool, user_id: i32, bear_id: Uuid) {
    let name = format!("surface-{bear_id}");
    sqlx::query!(
        r"
        INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
        VALUES ($1, $2, 'git_workspace', $3, NOW(), NOW())
        ",
        bear_id,
        name,
        user_id,
    )
    .execute(pool)
    .await
    .expect("seed work surface");
    sqlx::query!(
        r"
        INSERT INTO git_work_surface_details (id, upstream_url)
        VALUES ($1, $2)
        ",
        bear_id,
        "https://example.test/docket.git",
    )
    .execute(pool)
    .await
    .expect("seed git work surface details");
    sqlx::query!(
        r"
        INSERT INTO work_surface_bears (surface_id, bear_id)
        VALUES ($1, $2)
        ",
        bear_id,
        bear_id,
    )
    .execute(pool)
    .await
    .expect("assign work surface");
}

async fn create_job_task(
    docket: &PgDocketService,
    bear_id: Uuid,
    owner: i32,
    title: &str,
    visibility: TaskListVisibility,
) -> Uuid {
    docket
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: owner,
            created_by_role: "pair".to_string(),
            goal: format!("{title} objective"),
            work_surface_id: Some(bear_id),
            work_surface_assignments: vec![],
            commit_policy: Some(DocketCommitPolicy::None),
            work_branch: None,
            visibility,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![DocketJobCriterionInput {
                kind: DocketCriterionKind::Narrative,
                description: "Complete the task".to_string(),
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
                title: title.to_string(),
                body: "Task details".to_string(),
                completion_criteria: vec!["Done".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            }],
        })
        .await
        .expect("create job")
        .tasks[0]
        .id
}

async fn create_standalone(
    docket: &PgDocketService,
    bear_id: Uuid,
    session_id: Uuid,
    owner: i32,
    title: &str,
) -> Uuid {
    docket
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: None,
            session_anchor_id: Some(session_id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: title.to_string(),
            body: "Standalone details".to_string(),
            completion_criteria: vec!["Done".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(owner),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .expect("create standalone task")
        .id
}

#[sqlx::test(migrations = "../../migrations")]
async fn session_selection_scopes_historical_attachments_and_does_not_leak_candidates(
    pool: PgPool,
) {
    let owner = seed_user(&pool).await;
    let viewer = seed_user(&pool).await;
    let suffix = Uuid::new_v4().simple().to_string();
    let bear_slug = format!("task-access-{}", &suffix[..16]);
    let bear_id = create_bear(
        &pool,
        BearParams {
            slug: &bear_slug,
            name: "Current task access test",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create bear");
    for user_id in [owner, viewer] {
        grant_membership(&pool, user_id, bear_id, Some(BEAR_ROLE_MEMBER))
            .await
            .expect("grant membership");
    }
    seed_surface(&pool, owner, bear_id).await;
    let (viewer_session, viewer_anchor) = seed_session(&pool, viewer, bear_id, &bear_slug).await;
    let (owner_session, owner_anchor) = seed_session(&pool, owner, bear_id, &bear_slug).await;
    let docket = PgDocketService::from_pool(&pool);
    let private = create_job_task(
        &docket,
        bear_id,
        owner,
        "foreign SameUser secret",
        TaskListVisibility::SameUser,
    )
    .await;
    let visible = create_job_task(
        &docket,
        bear_id,
        owner,
        "shared BearVisible task",
        TaskListVisibility::BearVisible,
    )
    .await;
    let unattached = create_job_task(
        &docket,
        bear_id,
        owner,
        "unattached BearVisible task",
        TaskListVisibility::BearVisible,
    )
    .await;
    let own = create_standalone(&docket, bear_id, viewer_anchor, viewer, "own standalone").await;
    // Reproduce a historically attached standalone task created by another
    // member; its creator, not the attachment alone, controls visibility.
    let foreign = create_standalone(
        &docket,
        bear_id,
        viewer_anchor,
        owner,
        "foreign standalone secret",
    )
    .await;
    docket
        .attach_task_to_session(bear_id, private, owner_anchor)
        .await
        .expect("attach owner's private task");
    let capabilities = CapabilitySet::from_capabilities([BearCapability::SelectSessionTask]);
    assert_eq!(
        preview_session_current_task_selection(&pool, owner, bear_id, &owner_session, private)
            .await
            .expect("owner can preview SameUser task"),
        "foreign SameUser secret",
    );
    assert_eq!(
        select_session_current_task(
            &pool,
            owner,
            bear_id,
            &owner_session,
            Some(private),
            &capabilities,
        )
        .await
        .expect("owner can select SameUser task")
        .title
        .as_deref(),
        Some("foreign SameUser secret"),
    );
    for task_id in [private, visible] {
        docket
            .attach_task_to_session(bear_id, task_id, viewer_anchor)
            .await
            .expect("attach historical job task");
    }

    // An old private selection must not be grandfathered in just because its
    // attachment and current_task_id are still persisted in the viewer's session.
    client_sessions::set_current_task(&pool, viewer, bear_id, &viewer_session, Some(private))
        .await
        .expect("seed stale private selection");
    for task_id in [private, foreign] {
        let preview = preview_session_current_task_selection(
            &pool,
            viewer,
            bear_id,
            &viewer_session,
            task_id,
        )
        .await
        .expect_err("foreign task preview must fail");
        let selected = select_session_current_task(
            &pool,
            viewer,
            bear_id,
            &viewer_session,
            Some(task_id),
            &capabilities,
        )
        .await
        .expect_err("foreign task selection must fail");
        for error in [preview, selected] {
            assert!(matches!(error, CustomError::NotFound(_)), "{error}");
            assert!(!error.to_string().contains("secret"), "{error}");
        }
    }
    assert_eq!(
        client_sessions::find_for_user_bear_session_id(&pool, viewer, bear_id, &viewer_session)
            .await
            .expect("load viewer session")
            .expect("viewer session exists")
            .current_task_id,
        Some(private),
        "denied selection must not mutate the session",
    );

    let error =
        preview_session_current_task_selection(&pool, viewer, bear_id, &viewer_session, unattached)
            .await
            .expect_err("authorized but unattached task is not selectable");
    assert!(matches!(error, CustomError::ValidationError(_)));
    let candidates = error.to_string();
    assert!(candidates.contains("own standalone"), "{candidates}");
    assert!(
        candidates.contains("shared BearVisible task"),
        "{candidates}"
    );
    for secret in [
        private.to_string(),
        foreign.to_string(),
        "secret".to_string(),
    ] {
        assert!(!candidates.contains(&secret), "{candidates}");
    }
    let error = select_session_current_task(
        &pool,
        viewer,
        bear_id,
        &viewer_session,
        Some(unattached),
        &capabilities,
    )
    .await
    .expect_err("authorized but unattached task cannot be selected");
    assert!(matches!(error, CustomError::ValidationError(_)));
    assert!(!error.to_string().contains("secret"), "{error}");

    let cleared_stale =
        select_session_current_task(&pool, viewer, bear_id, &viewer_session, None, &capabilities)
            .await
            .expect("clear stale private selection");
    assert!(cleared_stale.title.is_none());
    let items = &cleared_stale
        .task_list
        .expect("scoped task list on clear")
        .items;
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|item| item.id != private.to_string() && item.id != foreign.to_string()));
    assert_eq!(
        client_sessions::find_for_user_bear_session_id(&pool, viewer, bear_id, &viewer_session)
            .await
            .expect("load cleared session")
            .expect("cleared session exists")
            .current_task_id,
        None,
    );

    assert_eq!(
        preview_session_current_task_selection(&pool, viewer, bear_id, &viewer_session, visible)
            .await
            .expect("preview shared task"),
        "shared BearVisible task",
    );
    let selected = select_session_current_task(
        &pool,
        viewer,
        bear_id,
        &viewer_session,
        Some(visible),
        &capabilities,
    )
    .await
    .expect("select shared task");
    assert_eq!(selected.title.as_deref(), Some("shared BearVisible task"));
    let items = &selected.task_list.expect("scoped task list").items;
    assert!(items.iter().any(|item| item.id == visible.to_string()));
    assert!(items
        .iter()
        .all(|item| item.id != private.to_string() && item.id != foreign.to_string()));
    let cleared =
        select_session_current_task(&pool, viewer, bear_id, &viewer_session, None, &capabilities)
            .await
            .expect("clear selection");
    assert!(cleared.title.is_none());
    let items = &cleared.task_list.expect("scoped task list on clear").items;
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| item.id == own.to_string()));
    assert!(items.iter().any(|item| item.id == visible.to_string()));

    assert_eq!(
        select_session_current_task(
            &pool,
            viewer,
            bear_id,
            &viewer_session,
            Some(own),
            &capabilities,
        )
        .await
        .expect("owner can select standalone task")
        .title
        .as_deref(),
        Some("own standalone"),
    );
}
