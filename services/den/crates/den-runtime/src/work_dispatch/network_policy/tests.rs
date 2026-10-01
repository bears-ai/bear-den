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
