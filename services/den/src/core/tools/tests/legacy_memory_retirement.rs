use crate::{
    config::Config,
    core::tools::{
        activity_payloads::{activity_payload, plan_mode_workplan_payload},
        session::DenToolInvocationContext,
        workflow::list_task_lists,
    },
};
use den_core::RuntimeContextLabel;
use den_memory::{tools::sqlite_write_at_path, MemoryStoreManager};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn task_lists_do_not_read_or_promote_legacy_pair_plan_artifacts() {
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let mut config = Config::test_stub();
    let directory = std::env::temp_dir().join(format!("den-retired-plans-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = directory.to_string_lossy().into_owned();
    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": bear_id,
        "bear_slug": "test",
        "binding_id": "test",
        "profile": "pair",
        "user_id": 1,
        "conversation_id": "conversation",
        "session_id": "session"
    }))
    .unwrap();
    sqlite_write_at_path(
        &stores,
        bear_id,
        "pair/plans/legacy.md",
        "pair",
        "Legacy plan",
        "private legacy plan body",
        json!({"kind": "plan"}),
    )
    .await
    .unwrap();
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let before = den_memory::tools::sqlite_memory_read(&store, "pair/plans/legacy.md")
        .await
        .unwrap();
    for role in [
        RuntimeContextLabel::ArmatureConversation,
        RuntimeContextLabel::JobRun,
        RuntimeContextLabel::ChannelConversation,
    ] {
        for include_artifacts in [true, false] {
            let result = list_task_lists(
                &pool,
                &config,
                &stores,
                &context,
                role,
                json!({"include_plan_mode": false, "include_artifacts": include_artifacts}),
                activity_payload,
                plan_mode_workplan_payload,
            )
            .await
            .unwrap();
            assert!(result.get("plan_artifacts").is_none());
            assert_eq!(result["plan_mode_gates"], json!([]));
            assert_eq!(result["workplans"], json!([]));
            assert_eq!(result["linked_plan_artifact_paths"], json!([]));
            assert_eq!(result["task_lists"], json!([]));
            assert!(!result.to_string().contains("private legacy plan body"));
        }
    }
    let after = den_memory::tools::sqlite_memory_read(&store, "pair/plans/legacy.md")
        .await
        .unwrap();
    assert_eq!(before, after);
    drop(store);
    drop(stores);
    std::fs::remove_dir_all(directory).unwrap();
}

#[sqlx::test]
async fn plan_exit_persists_only_canonical_state_and_keeps_approval_and_links(pool: sqlx::PgPool) {
    use crate::core::tools::{
        activity_payloads::no_active_workplan_payload, plan_mode::DenPlanModeOps,
    };
    use den_core::tools::plan_mode::PlanModeOps;
    use den_service::client_sessions::{self, ClientSessionMode, UpsertClientSession};

    let suffix = Uuid::new_v4().simple().to_string();
    let bear_id = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ($1, 'Memory Test Bear') RETURNING id",
        format!("memory-test-{}", &suffix[..12]),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, 'Test', 'x') RETURNING id",
        format!("memory-{suffix}@example.invalid"),
        format!("memory{}", &suffix[..12]),
    ).fetch_one(&pool).await.unwrap();
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": bear_id, "bear_slug": format!("memory-test-{}", &suffix[..12]),
        "binding_id": "test", "profile": "pair", "user_id": user_id,
        "conversation_id": "conversation", "session_id": "session",
        "client_session_id": "client-test"
    }))
    .unwrap();
    client_sessions::upsert_session(
        &pool,
        UpsertClientSession {
            user_id,
            bear_id,
            bear_slug: context.bear_slug.clone(),
            client_session_id: "client-test".into(),
            runtime_session_id: "session".into(),
            conversation_id: "conversation".into(),
            resolved_conversation_id: None,
            client: "test".into(),
            cwd: None,
            current_mode: Some(ClientSessionMode::Ask),
        },
    )
    .await
    .unwrap();
    let ops = DenPlanModeOps {
        pool: &pool,
        workplan_payload: plan_mode_workplan_payload,
        no_active_workplan: no_active_workplan_payload,
    };
    let entered = ops
        .enter(
            &context,
            "client-test",
            "Implement".into(),
            Some("ask".into()),
        )
        .await
        .unwrap();
    let plan_id = serde_json::from_value::<Uuid>(entered.plan_mode["id"].clone()).unwrap();
    den_runtime::plan_mode::submit_plan_artifact(
        &pool,
        den_runtime::plan_mode::SubmitPlanModeParams {
            user_id,
            bear_id,
            client_session_id: "client-test".into(),
            plan_mode_id: Some(plan_id),
            title: "Existing plan".into(),
            body: "Existing canonical body".into(),
            artifact_path: "pair/plans/explicit-reference.md".into(),
            approval_request_id: Some(format!("plan-mode-{plan_id}")),
        },
    )
    .await
    .unwrap();
    let mut wrong_user = context.clone();
    wrong_user.user_id += 1;
    let mut wrong_bear = context.clone();
    wrong_bear.bear_id = Uuid::new_v4();
    for (caller, session) in [
        (&wrong_user, "client-test"),
        (&wrong_bear, "client-test"),
        (&context, "other-client"),
    ] {
        assert!(matches!(
            ops.exit(caller, session, Some(plan_id), "Forged", "Must not submit")
                .await,
            Err(den_core::DenError::NotFound(_))
        ));
    }
    let mut config = Config::test_stub();
    let directory = std::env::temp_dir().join(format!("den-plan-exit-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = directory.to_string_lossy().into_owned();
    let stores = MemoryStoreManager::new(&config);
    let submitted = ops
        .exit(
            &context,
            "client-test",
            Some(plan_id),
            "Plan",
            "Implement the change",
        )
        .await
        .unwrap();
    assert_eq!(submitted.storage, "postgres");
    assert_eq!(submitted.artifact_path, "pair/plans/explicit-reference.md");
    assert_eq!(submitted.plan_mode["state"], "submitted");
    assert_eq!(submitted.submitted_plan["body"], "Implement the change");
    assert_eq!(
        submitted.plan_mode["approval_request_id"],
        format!("plan-mode-{plan_id}")
    );
    assert!(!directory.exists(), "plan exit must not open Bear memory");
    let revised = ops
        .exit(
            &context,
            "client-test",
            Some(plan_id),
            "Revised",
            "Implement the revised change",
        )
        .await
        .unwrap();
    assert_eq!(revised.artifact_path, submitted.artifact_path);
    let listing = list_task_lists(
        &pool,
        &config,
        &stores,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        json!({}),
        activity_payload,
        plan_mode_workplan_payload,
    )
    .await
    .unwrap();
    assert_eq!(listing["plan_mode_gates"][0]["id"], plan_id.to_string());
    assert_eq!(
        listing["linked_plan_artifact_paths"],
        json!([submitted.artifact_path])
    );
    assert!(listing.get("plan_artifacts").is_none());
    assert!(!directory.exists());
    let approved = ops
        .record_approval(&context, "client-test", Some(plan_id))
        .await
        .unwrap();
    assert_eq!(approved.plan_mode["state"], "approved");
    assert_eq!(approved.plan_mode["approved_by_user_id"], user_id);
    let session =
        client_sessions::find_for_user_bear_session_id(&pool, user_id, bear_id, "client-test")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(session.current_mode, "write");
    assert!(session.current_task_id.is_none());
    assert!(!directory.exists());
}
