use super::*;
use den_core::ids::UserId;
use den_memory::{scoped::MemoryReadGrant, MemorySource};
use den_service::{
    bears::{
        db,
        hats::{
            self,
            access::{HatAccessGrant, HttpsHost},
        },
    },
    work_surfaces::{self, NewWorkSurface},
};
use uuid::Uuid;

#[test]
fn a_provider_missing_the_run_ceiling_capability_is_not_eligible() {
    let old: HealthResponse = serde_json::from_value(serde_json::json!({
        "ok": true, "backend_available": true, "active_sandboxes": 0, "roots": []
    }))
    .unwrap();
    assert!(require_provider_run_ceiling(&old).is_err());
    let mut current = old;
    current.run_outbound_ceiling_supported = true;
    assert!(require_provider_run_ceiling(&current).is_ok());
    current.backend_available = false;
    assert!(require_provider_run_ceiling(&current).is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_hosts_are_bounded_by_both_the_assigned_surface_and_current_hat_grants(pool: PgPool) {
    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "runceilingbear",
            name: "Run ceiling",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = sqlx::query_scalar!(
            "INSERT INTO users (email, username) VALUES ('dispatch-hat@example.test', 'dispatchhat') RETURNING id"
        )
        .fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let surface = work_surfaces::create_surface(
        &pool,
        admin,
        NewWorkSurface {
            name: "runceilingroot".into(),
            description: None,
            upstream_url: "https://example.test/repo.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec!["docs.example.com".into(), "registry.example.com".into()],
            credential: None,
        },
        "",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(&pool, surface.id, bear, admin)
        .await
        .unwrap();
    let first = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Research",
        "Read docs",
    )
    .await
    .unwrap();
    let second = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Review",
        "Review code",
    )
    .await
    .unwrap();
    let context = WorkRunDispatchContext {
        bear_slug: "runceilingbear".into(),
        bear_name: "Run ceiling".into(),
        created_by_user_id: admin,
        job_goal: "Review".into(),
        work_surface_id: Some(surface.id),
        work_surface_name: Some(surface.name.clone()),
        commit_policy: None,
        work_branch: None,
        allow_default_ref: false,
        child_result_rollups: serde_json::json!([]),
    };
    let binding = |hat| {
        ResolvedMemoryBinding::Bound(MemoryReadGrant::new(
            MemorySource::WorkRun(Uuid::new_v4()),
            Some(hat),
        ))
    };
    assert_eq!(
        for_run(
            &pool,
            BearId::new(bear),
            binding(first.id),
            &context,
            &surface.name
        )
        .await
        .unwrap()
        .unwrap()
        .as_slice(),
        &[] as &[String]
    );
    let host = HatAccessGrant::HttpsHost(HttpsHost::parse("docs.example.com").unwrap());
    let grant_id = access::grant(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        &host,
        true,
    )
    .await
    .unwrap();
    let outside =
        HatAccessGrant::HttpsHost(HttpsHost::parse("not-on-surface.example.com").unwrap());
    access::grant(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        &outside,
        true,
    )
    .await
    .unwrap();
    let permitted = for_run(
        &pool,
        BearId::new(bear),
        binding(first.id),
        &context,
        &surface.name,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(permitted.as_slice(), &["docs.example.com".to_string()]);
    assert!(for_run(
        &pool,
        BearId::new(bear),
        binding(second.id),
        &context,
        &surface.name
    )
    .await
    .unwrap()
    .unwrap()
    .is_empty());
    assert!(for_run(
        &pool,
        BearId::new(Uuid::new_v4()),
        binding(first.id),
        &context,
        &surface.name
    )
    .await
    .is_err());
    assert!(for_run(
        &pool,
        BearId::new(bear),
        binding(first.id),
        &context,
        "different-root"
    )
    .await
    .is_err());
    let mut wrong_surface = context.clone();
    wrong_surface.work_surface_id = Some(Uuid::new_v4());
    assert!(for_run(
        &pool,
        BearId::new(bear),
        binding(first.id),
        &wrong_surface,
        &surface.name
    )
    .await
    .is_err());
    hats::allow_surface(&pool, BearId::new(bear), first.id, surface.id)
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        first.id.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Old unbound Work') RETURNING id",
        bear,
        admin,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job,
        surface.id,
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::bindings::bind_job_hat(&pool, BearId::new(bear), job, first.id)
        .await
        .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let run_id = sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id)
         VALUES ($1, $2, $3) RETURNING id",
        bear,
        job,
        job_run,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let missing_snapshot = den_docket::work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        active_run_still_authorized(&pool, &missing_snapshot)
            .await
            .is_err(),
        "a pre-cutover sandbox without a recorded hat ceiling fails closed"
    );
    den_docket::work_runs::merge_work_run_result_refs(
        &pool,
        run_id,
        &serde_json::json!({"hat_egress": snapshot(&permitted)}),
    )
    .await
    .unwrap();
    let queued = den_docket::work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(active_run_still_authorized(&pool, &queued).await.unwrap());
    let permitted_host = HttpsHost::parse("docs.example.com").unwrap();
    assert!(!host_allowed_for_live_run(&pool, &queued, &permitted_host)
        .await
        .unwrap());
    let run_token_id = Uuid::new_v4();
    den_docket::work_runs::merge_work_run_result_refs(
        &pool,
        run_id,
        &serde_json::json!({"armature_token_id": run_token_id}),
    )
    .await
    .unwrap();
    let claimed = den_docket::work_runs::claim_next_work_run(
        &pool,
        "egress-policy-test",
        std::time::Duration::from_mins(2),
    )
    .await
    .unwrap()
    .expect("test Work run is claimable");
    assert_eq!(claimed.id, run_id);
    den_docket::work_runs::record_work_run_provisioned(
        &pool,
        run_id,
        &den_docket::work_runs::WorkRunProvisioned {
            sandbox_server_url: "https://provider.example.test".into(),
            sandbox_id: "den-test-sandbox".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();
    let active = den_docket::work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(host_allowed_for_live_run(&pool, &active, &permitted_host)
        .await
        .unwrap());
    assert!(super::super::allow_work_egress_connection(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        run_token_id,
        run_id,
        &permitted_host,
    )
    .await
    .unwrap());
    for (actor, token, owner) in [
        (admin, Uuid::new_v4(), BearId::new(bear)),
        (admin + 1, run_token_id, BearId::new(bear)),
        (admin, run_token_id, BearId::new(Uuid::new_v4())),
    ] {
        assert!(!super::super::allow_work_egress_connection(
            &pool,
            owner,
            UserId::new(actor),
            token,
            run_id,
            &permitted_host,
        )
        .await
        .unwrap());
    }
    assert!(!host_allowed_for_live_run(
        &pool,
        &active,
        &HttpsHost::parse("registry.example.com").unwrap()
    )
    .await
    .unwrap());
    access::revoke(
        &pool,
        BearId::new(bear),
        first.id,
        UserId::new(admin),
        grant_id,
    )
    .await
    .unwrap();
    assert!(for_run(
        &pool,
        BearId::new(bear),
        binding(first.id),
        &context,
        &surface.name
    )
    .await
    .unwrap()
    .unwrap()
    .is_empty());
    let current = den_docket::work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !active_run_still_authorized(&pool, &current).await.unwrap(),
        "removing a provisioned host must stop the running Work sandbox"
    );
    assert!(!host_allowed_for_live_run(&pool, &current, &permitted_host)
        .await
        .unwrap());
    assert!(for_run(
        &pool,
        BearId::new(bear),
        ResolvedMemoryBinding::Legacy,
        &context,
        &surface.name
    )
    .await
    .unwrap()
    .is_none());
}
