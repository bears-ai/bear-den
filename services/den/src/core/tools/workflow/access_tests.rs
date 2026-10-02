use den_docket::{
    DocketCommitPolicy, DocketJobCreate, DocketJobOverlapResolution, DocketService,
    DocketTaskInput, DocketTaskKind, DocketTaskScope, PgDocketService, RoutingStrategy,
    TaskListSourceRef, TaskListVisibility,
};
use den_service::{
    bears::{db, db::BearParams, BearProfile},
    work_surfaces::{self, NewWorkSurface},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::core::tools::session::invoke_den_tool_for_origin;
use den_core::tools::constants::DEN_TASK_LIST_SYNC;
use den_core::{ArmatureAvailability, Governance, TurnExecutionOrigin};

async fn fixture(
    pool: &PgPool,
) -> (
    DenToolInvocationContext,
    DenToolInvocationContext,
    DenToolInvocationContext,
    Uuid,
    Uuid,
    Uuid,
) {
    let nonce = Uuid::new_v4().simple().to_string();
    let bear_id = db::create_bear(
        pool,
        BearParams {
            slug: &format!("workflow-access-{}", &nonce[..12]),
            name: "Workflow access test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut users = Vec::new();
    for (index, role) in ["member", "member", "admin"].into_iter().enumerate() {
        let user_id = crate::core::user::db::create_user(
            pool,
            &format!("wf-{index}-{nonce}@example.invalid"),
            &format!("wf{index}{}", &nonce[..12]),
            "Workflow Tester",
            "test-hash",
        )
        .await
        .unwrap();
        db::grant_membership(pool, user_id, bear_id, Some(role))
            .await
            .unwrap();
        let context: DenToolInvocationContext = serde_json::from_value(json!({
            "bear_id": bear_id,
            "bear_slug": "workflow-access",
            "binding_id": "pair-test",
            "profile": "pair",
            "user_id": user_id,
            "membership_role": role,
            "conversation_id": format!("wf-{nonce}-{index}"),
            "session_id": format!("wf-session-{nonce}-{index}"),
            "channel": {}
        }))
        .unwrap();
        users.push(context);
    }
    let surface = work_surfaces::create_surface(
        pool,
        users[0].user_id,
        NewWorkSurface {
            name: format!("wf-{nonce}"),
            description: None,
            upstream_url: "https://example.invalid/workflow.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        "unused-test-secret",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(pool, surface.id, bear_id, users[0].user_id)
        .await
        .unwrap();
    let service = PgDocketService::from_pool(pool);
    let create = |visibility, goal: &str| DocketJobCreate {
        bear_id,
        created_by_user_id: users[0].user_id,
        created_by_role: "pair".into(),
        goal: format!("{goal}-{nonce}"),
        work_surface_id: Some(surface.id),
        work_surface_assignments: vec![],
        commit_policy: Some(DocketCommitPolicy::None),
        work_branch: None,
        visibility,
        source_conversation_id: None,
        objective_kind: None,
        supersedes_job_id: None,
        overlap_resolution: DocketJobOverlapResolution::Reject,
        criteria: vec![],
        tasks: vec![DocketTaskInput {
            client_key: None,
            parent_client_key: None,
            parent_task_id: None,
            sibling_order: Some(0),
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: format!("{goal} task"),
            body: "Keep private".into(),
            completion_criteria: vec!["done".into()],
            difficulty: None,
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
        }],
    };
    let private = service
        .create_job(create(TaskListVisibility::SameUser, "private"))
        .await
        .unwrap();
    let visible = service
        .create_job(create(TaskListVisibility::BearVisible, "visible"))
        .await
        .unwrap();
    (
        users.remove(0),
        users.remove(0),
        users.remove(0),
        private.job.id,
        private.tasks[0].id,
        visible.job.id,
    )
}

fn denied(result: Result<serde_json::Value, CustomError>) {
    assert!(
        matches!(result, Err(CustomError::NotFound(_))),
        "expected not found, got {result:?}"
    );
}

#[sqlx::test]
async fn pair_job_tools_hide_private_jobs_before_reads_or_mutations(pool: PgPool) {
    let (owner, member, mut admin, private, task_id, visible) = fixture(&pool).await;
    // The database, not a stale session role, decides whether this viewer is an admin.
    admin.membership_role = Some("member".into());
    let config = Config::test_stub();
    let service = PgDocketService::from_pool(&pool);
    let before = service
        .get_job(owner.bear_id, private)
        .await
        .unwrap()
        .unwrap();
    let private_ref = private.simple().to_string();
    let private_ref = &private_ref[..8];
    let listed = list_jobs(
        &pool,
        &config,
        &member,
        json!({"limit": 1, "include_cancelled": true}),
    )
    .await
    .unwrap();
    assert_eq!(
        listed["count"], 1,
        "private jobs must not consume the SQL limit"
    );
    assert_eq!(listed["jobs"][0]["id"], json!(visible));
    denied(find_job(&pool, &member, json!({"job_ref": private_ref})).await);
    denied(get_job(&pool, &member, json!({"job_id": private})).await);
    denied(
        update_job(
            &pool,
            &member,
            BearProfile::Pair,
            json!({"job_id": private, "goal": "stolen"}),
        )
        .await,
    );
    denied(
        set_job_lifecycle(
            &pool,
            &member,
            BearProfile::Pair,
            json!({"job_id": private}),
            DocketJobStatus::Cancelled,
        )
        .await,
    );
    denied(
        cancel_job_run(
            &pool,
            &member,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    denied(
        execute_job(
            &pool,
            &member,
            BearProfile::Pair,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    denied(
        reconcile_job_execution(
            &pool,
            &member,
            BearProfile::Pair,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    denied(evaluate_criterion(&pool, &member, BearProfile::Pair, json!({
        "job_id": private, "run_id": Uuid::new_v4(), "criterion_id": Uuid::new_v4(), "status": "met"
    })).await);
    denied(
        list_tasks(
            &pool,
            &config,
            &member,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    denied(
        update_task(
            &pool,
            &member,
            BearProfile::Pair,
            json!({"task_id": task_id, "title": "stolen"}),
        )
        .await,
    );
    denied(
        dispatch_work(
            &pool,
            &config,
            &member,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    denied(
        settle_execution_task(
            &pool,
            &member,
            BearProfile::Pair,
            json!({
                "job_id": private, "task_id": task_id, "status": "done"
            }),
        )
        .await,
    );
    denied(
        update_current_task_status(
            &pool,
            &member,
            BearProfile::Pair,
            json!({
                "job_id": private, "task_id": task_id,
                "run_id": before.job.current_run_id, "status": "done"
            }),
        )
        .await,
    );
    denied(
        create_task(&pool, &member, BearProfile::Pair, WorkflowAuthority { origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected), governance: Governance::Interactive }, json!({
            "job_id": private, "title": "stolen", "body": "stolen", "completion_criteria": ["done"]
        }))
        .await,
    );
    denied(
        append_docket_entry(
            &pool,
            &member,
            BearProfile::Pair,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({
                "job_id": private, "scope": "job_notebook", "kind": "finding", "summary": "stolen"
            }),
        )
        .await,
    );
    denied(list_docket_entries(&pool, &member, json!({"job_id": private})).await);
    denied(
        checkout_task_list(
            &pool,
            &member,
            BearProfile::Pair,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );

    let unchanged = service
        .get_job(owner.bear_id, private)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.job.goal.starts_with("private-"));
    assert_eq!(unchanged.tasks[0].title, "private task");
    assert_eq!(unchanged.job.current_run_id, before.job.current_run_id);
    assert_eq!(unchanged.job.status, before.job.status);
    assert_eq!(
        unchanged.current_run.as_ref().map(|run| &run.state),
        before.current_run.as_ref().map(|run| &run.state)
    );
    assert_eq!(
        get_job(&pool, &owner, json!({"job_id": private}))
            .await
            .unwrap()["found"],
        true
    );
    assert_eq!(
        get_job(&pool, &admin, json!({"job_id": private}))
            .await
            .unwrap()["found"],
        true
    );
    assert_eq!(
        get_job(&pool, &member, json!({"job_id": visible}))
            .await
            .unwrap()["found"],
        true
    );
    assert_eq!(
        list_jobs(&pool, &config, &admin, json!({"limit": 10}))
            .await
            .unwrap()["count"],
        2
    );
    assert_eq!(
        update_job(
            &pool,
            &owner,
            BearProfile::Pair,
            json!({
                "job_id": private, "goal": "owner update"
            })
        )
        .await
        .unwrap()["job"]["job"]["goal"],
        "owner update"
    );
    assert_eq!(
        update_job(
            &pool,
            &admin,
            BearProfile::Pair,
            json!({
                "job_id": private, "goal": "admin update"
            })
        )
        .await
        .unwrap()["job"]["job"]["goal"],
        "admin update"
    );
    assert_eq!(
        update_job(
            &pool,
            &member,
            BearProfile::Pair,
            json!({
                "job_id": visible, "goal": "member visible update"
            })
        )
        .await
        .unwrap()["job"]["job"]["goal"],
        "member visible update"
    );
}

#[sqlx::test]
async fn hosted_pair_sync_rejects_forged_private_job_without_mutation(pool: PgPool) {
    let (owner, member, _, private, _, _) = fixture(&pool).await;
    let service = PgDocketService::from_pool(&pool);
    let before = service
        .get_job(owner.bear_id, private)
        .await
        .unwrap()
        .unwrap();
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let mut task_list = docket::task_list_projection_from_docket_job(&before, None);
    task_list.items[0].title = "Owner's edited title".into();
    task_list.items[0].summary = Some("Owner's edited body".into());
    task_list.updated_at = time::OffsetDateTime::now_utc();

    let mut wrong_bear = task_list.clone();
    wrong_bear.bear_id = Uuid::new_v4();
    let mut wrong_job = task_list.clone();
    wrong_job.source_ref.docket_job_id = Some(Uuid::new_v4().to_string());
    let mut item_only_job = task_list.clone();
    item_only_job.source_ref = TaskListSourceRef::local(vec![]);

    for (context, projection) in [
        (&member, task_list.clone()),
        (&member, item_only_job),
        (&owner, wrong_bear),
        (&owner, wrong_job),
    ] {
        let result = invoke_den_tool_for_origin(
            &pool,
            &config,
            &stores,
            DEN_TASK_LIST_SYNC,
            json!({"task_list": projection}),
            context.clone(),
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            Governance::Interactive,
        )
        .await;
        denied(result);
    }

    let unchanged = service
        .get_job(owner.bear_id, private)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.tasks.len(), before.tasks.len());
    assert_eq!(unchanged.tasks[0].title, before.tasks[0].title);
    assert_eq!(unchanged.tasks[0].body, before.tasks[0].body);
    assert_eq!(unchanged.tasks[0].updated_at, before.tasks[0].updated_at);
    assert_eq!(
        unchanged
            .task_states
            .iter()
            .map(|state| (state.task_id, &state.status, state.updated_at))
            .collect::<Vec<_>>(),
        before
            .task_states
            .iter()
            .map(|state| (state.task_id, &state.status, state.updated_at))
            .collect::<Vec<_>>()
    );
    assert_eq!(unchanged.job.current_run_id, before.job.current_run_id);
    assert_eq!(unchanged.job.updated_at, before.job.updated_at);

    let outcome = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        DEN_TASK_LIST_SYNC,
        json!({"task_list": task_list}),
        owner,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        Governance::Interactive,
    )
    .await
    .unwrap();
    assert_eq!(outcome["sync"]["applied"], true);
    let after = service
        .get_job(member.bear_id, private)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.tasks[0].title, "Owner's edited title");
    assert_eq!(after.tasks[0].body, "Owner's edited body");
}

#[sqlx::test]
async fn historical_private_attachment_is_not_a_pair_task_grant(pool: PgPool) {
    let (_, mut member, _, private, task_id, _) = fixture(&pool).await;
    let client_id = format!("client-{}", Uuid::new_v4());
    den_service::client_sessions::upsert_session(
        &pool,
        den_service::client_sessions::UpsertClientSession {
            user_id: member.user_id,
            bear_id: member.bear_id,
            bear_slug: member.bear_slug.clone(),
            client_session_id: client_id.clone(),
            runtime_session_id: format!("runtime-{client_id}"),
            conversation_id: member.conversation_id.clone(),
            resolved_conversation_id: None,
            client: "test".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .unwrap();
    member.client_session_id = Some(client_id.clone());
    let session = den_service::client_sessions::find_for_user_bear_session_id(
        &pool,
        member.user_id,
        member.bear_id,
        &client_id,
    )
    .await
    .unwrap()
    .unwrap();
    let service = PgDocketService::from_pool(&pool);
    // Reproduce a historical attachment made by a trusted caller, not a Pair grant.
    service
        .attach_task_to_session(member.bear_id, task_id, session.id)
        .await
        .unwrap();
    let status = get_task_list_status(&pool, &member, BearProfile::Pair, json!({}), |_| json!({}))
        .await
        .unwrap();
    assert_eq!(status["count"], 0);
    assert_eq!(status["found"], false);
    denied(
        update_task_list(
            &pool,
            &member,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"task_id": task_id}),
            |_| json!({}),
        )
        .await,
    );
    denied(
        select_current_task(
            &pool,
            &member,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"task_id": task_id}),
        )
        .await,
    );
    denied(
        checkout_task_list(
            &pool,
            &member,
            BearProfile::Pair,
            WorkflowAuthority {
                origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
                governance: Governance::Interactive,
            },
            json!({"job_id": private}),
        )
        .await,
    );
    let session = den_service::client_sessions::find_for_user_bear_session_id(
        &pool,
        member.user_id,
        member.bear_id,
        &client_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(session.current_task_id.is_none());
}
