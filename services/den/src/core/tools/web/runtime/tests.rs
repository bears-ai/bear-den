use super::*;
use den_core::tools::{arguments::DenToolChannelContext, context::DenToolInvocationContext};
use den_runtime::agent_loop::{
    create_native_approval, decide_native_approval, NativeApprovalDecision,
};
use den_service::bears::{db, RuntimeContextLabel};
use serde_json::{json, Value};

struct MockTransport<'a>(DenWebFetcher<'a>);

impl WebFetcher for MockTransport<'_> {
    async fn decide_fetch_approval(
        &self,
        context: &DenToolInvocationContext,
        raw_url: &str,
    ) -> Result<(WebUrl, WebApproval), DenError> {
        self.0.decide_fetch_approval(context, raw_url).await
    }

    async fn record_fetch_attempt(&self, audit: WebFetchAudit<'_>) -> Result<(), DenError> {
        self.0.record_fetch_attempt(audit).await
    }

    async fn http_get(&self, url: &str) -> Result<WebHttpResponse, DenError> {
        Ok(WebHttpResponse {
            final_url: url.to_string(),
            final_host: "example.com".into(),
            status: 200,
            content_type: "text/plain".into(),
            body: b"Synthetic response".to_vec(),
            total_bytes: 18,
            body_truncated: false,
        })
    }

    async fn authorize_search(&self, context: &DenToolInvocationContext) -> Result<(), DenError> {
        self.0.authorize_search(context).await
    }

    async fn preferred_hosts(&self, _bear_id: Uuid) -> Result<Vec<String>, DenError> {
        Ok(Vec::new())
    }

    fn normalize_host(&self, _url: &str) -> Option<String> {
        None
    }

    fn default_search_max_results(&self) -> usize {
        5
    }

    async fn provider_search(&self, query: &str, _limit: usize) -> Result<Value, DenError> {
        Ok(
            json!({"results": [{"url": "https://example.com/docs", "title": "Synthetic result", "snippet": query}]}),
        )
    }
}

fn context(bear_id: Uuid, session_id: &str, request_id: Uuid) -> DenToolInvocationContext {
    DenToolInvocationContext {
        bear_id,
        bear_slug: "one-shot-web".into(),
        binding_id: "den-native:one-shot-web:pair".into(),
        profile: Some(RuntimeContextLabel::ArmatureConversation),
        user_id: 7,
        username: None,
        membership_role: None,
        conversation_id: "den-conv-one-shot".into(),
        session_id: session_id.into(),
        work_run_id: None,
        client_session_id: Some(session_id.into()),
        conversation_selection: None,
        runtime_target: None,
        workspace_roots: Vec::new(),
        session_capabilities: Vec::new(),
        session_policy: None,
        activity: None,
        runtime: None,
        context_budget: None,
        projected_memory: None,
        recalled_memory: None,
        request_id: Some(request_id.to_string()),
        channel: DenToolChannelContext::default(),
    }
}

#[sqlx::test]
async fn exact_web_fetch_approval_is_consumed_once_without_a_bear_wide_grant(pool: PgPool) {
    use den_core::ids::{BearId, UserId};
    use den_service::{bears::hats, conversation::persistence};

    let bear_id = db::create_bear(
        &pool,
        db::BearParams {
            slug: "oneshotwebpolicy",
            name: "One shot web policy",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let user = sqlx::query_scalar!("INSERT INTO users (username, email) VALUES ('fetchhatadmin', 'fetchhatadmin@example.test') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, user, bear_id, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let conversation = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(user),
        "den-conv-one-shot",
        None,
        None,
    )
    .await
    .unwrap();
    let config = Config::test_stub();
    let fetcher = DenWebFetcher {
        pool: &pool,
        config: &config,
    };
    let mut unbound = context(bear_id, "client-session-one", Uuid::new_v4());
    unbound.user_id = user;
    assert!(matches!(
        fetcher
            .decide_fetch_approval(&unbound, "https://example.com/first")
            .await,
        Err(DenError::Authorization(_))
    ));
    assert!(matches!(
        fetcher.authorize_search(&unbound).await,
        Err(DenError::Authorization(_))
    ));
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Research",
        "Fetch approved URLs",
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), conversation.id, hat.id)
        .await
        .unwrap();
    let url = "https://example.com/first";
    let approval_id = create_native_approval(
        &pool,
        bear_id,
        "den-conv-one-shot",
        "client-session-one",
        "call-one",
        "web_fetch",
        &json!({"url": url}),
    )
    .await
    .unwrap();
    decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
        .await
        .unwrap();
    let request_id = Uuid::new_v4();
    sqlx::query!(
        "UPDATE runtime_approvals SET execution_request_id = $1 WHERE approval_id = $2",
        request_id,
        approval_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    let config = Config::test_stub();
    let fetcher = DenWebFetcher {
        pool: &pool,
        config: &config,
    };
    let mut correct = context(bear_id, "client-session-one", request_id);
    correct.user_id = user;
    assert!(matches!(
        den_core::tools::web::web_search(
            &MockTransport(DenWebFetcher {
                pool: &pool,
                config: &config
            }),
            &correct,
            json!({"query": "test"}),
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    for (candidate, target) in [
        (
            {
                let mut other = correct.clone();
                other.session_id = "client-session-two".into();
                other
            },
            url,
        ),
        (correct.clone(), "https://example.com/other"),
        (
            {
                let mut other = correct.clone();
                other.request_id = Some(Uuid::new_v4().to_string());
                other
            },
            url,
        ),
    ] {
        assert_eq!(
            fetcher
                .decide_fetch_approval(&candidate, target)
                .await
                .unwrap()
                .1,
            WebApproval::RequiresApproval
        );
    }
    let mut unknown_bear = correct.clone();
    unknown_bear.bear_id = Uuid::new_v4();
    assert!(fetcher
        .decide_fetch_approval(&unknown_bear, url)
        .await
        .is_err());
    let mut missing_conversation = correct.clone();
    missing_conversation.conversation_id = "missing-conversation".into();
    assert!(fetcher
        .decide_fetch_approval(&missing_conversation, url)
        .await
        .is_err());
    assert!(fetcher
        .authorize_search(&missing_conversation)
        .await
        .is_err());
    let response = den_core::tools::web::web_fetch(
        &MockTransport(DenWebFetcher {
            pool: &pool,
            config: &config,
        }),
        &correct,
        json!({"url": url}),
    )
    .await
    .unwrap();
    assert_eq!(response["approval"], "approved_once");
    assert_eq!(response["text_excerpt"], "Synthetic response");
    let audit_kind = sqlx::query_scalar!(
        "SELECT approval_kind FROM bear_web_fetches WHERE bear_id = $1 ORDER BY fetched_at DESC LIMIT 1",
        bear_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_kind, "approved_once");
    assert!(den_core::tools::web::web_fetch(
        &MockTransport(DenWebFetcher {
            pool: &pool,
            config: &config
        }),
        &correct,
        json!({"url": url}),
    )
    .await
    .is_err());
    assert_eq!(
        fetcher
            .decide_fetch_approval(&correct, url)
            .await
            .unwrap()
            .1,
        WebApproval::RequiresApproval
    );
    assert_eq!(
        web_policy::decide_web_fetch_approval(&pool, bear_id, url)
            .await
            .unwrap()
            .1,
        web_policy::WebApprovalDecision::RequiresApproval,
    );
    assert!(
        decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
            .await
            .is_err(),
        "a consumed approval cannot be rearmed by a replay"
    );
}

#[sqlx::test]
async fn configured_hat_fetch_ignores_bear_wide_allows_and_checks_owner_and_revocation(
    pool: PgPool,
) {
    use den_core::ids::{BearId, UserId};
    use den_service::{
        bears::hats::{
            self,
            access::{HatAccessGrant, HttpsHost, ToolActionKey},
        },
        conversation::persistence,
    };

    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "hatwebfetch",
            name: "Hat web fetch",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let other_bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "otherhatwebfetch",
            name: "Other hat web fetch",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = sqlx::query_scalar!("INSERT INTO users (username, email) VALUES ('fetchhatadmin', 'fetchhatadmin@example.test') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let member = sqlx::query_scalar!("INSERT INTO users (username, email) VALUES ('fetchhatmember', 'fetchhatmember@example.test') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, admin, other_bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    db::grant_membership(&pool, member, bear, Some(db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    web_policy::record_web_approval(
        &pool,
        bear,
        "host",
        "example.com",
        Some(admin),
        "admin",
        None,
    )
    .await
    .unwrap();
    hats::create_hat(
        &pool,
        BearId::new(other_bear),
        UserId::new(admin),
        "Other",
        "Other Bear",
    )
    .await
    .unwrap();
    let a = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Review",
        "Review docs",
    )
    .await
    .unwrap();
    let b = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(admin),
        "Support",
        "Support docs",
    )
    .await
    .unwrap();
    let own = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(member),
        "hat-fetch-own",
        None,
        None,
    )
    .await
    .unwrap();
    let other = persistence::ensure_conversation_for_external_id(
        &pool,
        bear,
        Some(admin),
        "hat-fetch-other",
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), own.id, a.id)
        .await
        .unwrap();
    hats::bindings::bind_conversation_hat(&pool, BearId::new(bear), other.id, b.id)
        .await
        .unwrap();
    let url = "https://example.com/first";
    // The first hat revokes historical unscoped grants; later writes cannot
    // resurrect them, and an allowed source is not hat authority either.
    let active_legacy = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_web_approvals WHERE bear_id = $1 AND revoked_at IS NULL",
        bear,
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(active_legacy, 0);
    assert!(web_policy::record_web_approval(
        &pool,
        bear,
        "host",
        "example.com",
        Some(admin),
        "admin",
        None
    )
    .await
    .is_err());
    sqlx::query!("INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, policy) VALUES ($1, 'host', 'example.com', 'allowed')", bear).execute(&pool).await.unwrap();
    let mut config = Config::test_stub();
    config.den_search_provider = "brave".into();
    let fetcher = DenWebFetcher {
        pool: &pool,
        config: &config,
    };
    let mut ctx = context(bear, "hat-fetch-client", Uuid::new_v4());
    ctx.user_id = member;
    ctx.conversation_id = "hat-fetch-own".into();
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::RequiresApproval
    );
    assert!(
        matches!(
            den_core::tools::web::web_search(
                &MockTransport(DenWebFetcher {
                    pool: &pool,
                    config: &config
                }),
                &ctx,
                json!({"query": "private Bear notes"})
            )
            .await,
            Err(DenError::Authorization(_))
        ),
        "a configured hat must reject provider egress before any query is sent"
    );
    let tool = HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name("web_fetch").unwrap());
    let host = HatAccessGrant::HttpsHost(HttpsHost::parse("example.com").unwrap());
    access::grant(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        &tool,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::RequiresApproval
    );
    let host_id = access::grant(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        &host,
        true,
    )
    .await
    .unwrap();
    assert!(
        matches!(
            den_core::tools::web::web_search(
                &MockTransport(DenWebFetcher {
                    pool: &pool,
                    config: &config
                }),
                &ctx,
                json!({"query": "private Bear notes"})
            )
            .await,
            Err(DenError::Authorization(_))
        ),
        "a web-fetch grant cannot authorize a different network tool"
    );
    let search_tool =
        HatAccessGrant::ToolForHat(ToolActionKey::from_provider_name("web_search").unwrap());
    let provider_host =
        HatAccessGrant::HttpsHost(HttpsHost::parse("api.search.brave.com").unwrap());
    access::grant(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        &search_tool,
        true,
    )
    .await
    .unwrap();
    assert!(
        fetcher.authorize_search(&ctx).await.is_err(),
        "a search-tool grant alone is insufficient"
    );
    let provider_host_id = access::grant(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        &provider_host,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        den_core::tools::web::web_search(
            &MockTransport(DenWebFetcher {
                pool: &pool,
                config: &config
            }),
            &ctx,
            json!({"query": "reviewed public data"}),
        )
        .await
        .unwrap()["results"][0]["snippet"],
        "reviewed public data",
    );
    let mut other_hat_search = ctx.clone();
    other_hat_search.conversation_id = "hat-fetch-other".into();
    other_hat_search.user_id = admin;
    assert!(fetcher.authorize_search(&other_hat_search).await.is_err());
    sqlx::query!("INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, policy) VALUES ($1, 'host', $2, 'blocked')", bear, "api.search.brave.com").execute(&pool).await.unwrap();
    assert!(
        matches!(
            fetcher.authorize_search(&ctx).await,
            Err(DenError::Authorization(_))
        ),
        "a Bear web block beats both hat grants"
    );
    sqlx::query!(
        "DELETE FROM bear_web_sources WHERE bear_id = $1 AND scope_value = $2",
        bear,
        "api.search.brave.com"
    )
    .execute(&pool)
    .await
    .unwrap();
    access::revoke(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        provider_host_id,
    )
    .await
    .unwrap();
    assert!(
        fetcher.authorize_search(&ctx).await.is_err(),
        "provider-host revocation takes effect on the next call"
    );
    let persistent_choice_approval = create_native_approval(
        &pool,
        bear,
        "hat-fetch-own",
        "hat-fetch-client",
        "persistent-hat-call",
        "web_fetch",
        &json!({"url": url}),
    )
    .await
    .unwrap();
    decide_native_approval(
        &pool,
        &persistent_choice_approval,
        NativeApprovalDecision::Approve,
        None,
    )
    .await
    .unwrap();
    let persistent_request_id = Uuid::new_v4();
    sqlx::query!(
        "UPDATE runtime_approvals SET execution_request_id = $1 WHERE approval_id = $2",
        persistent_request_id,
        persistent_choice_approval,
    )
    .execute(&pool)
    .await
    .unwrap();
    ctx.request_id = Some(persistent_request_id.to_string());
    let response = den_core::tools::web::web_fetch(
        &MockTransport(DenWebFetcher {
            pool: &pool,
            config: &config,
        }),
        &ctx,
        json!({"url": url}),
    )
    .await
    .unwrap();
    assert_eq!(response["approval"], "hat_host");
    assert!(sqlx::query_scalar!(
        "SELECT consumed_at IS NOT NULL AS \"consumed!\" FROM runtime_approvals WHERE approval_id = $1",
        persistent_choice_approval,
    ).fetch_one(&pool).await.unwrap(), "the approval which installed a hat grant cannot reappear after revocation");
    assert_eq!(sqlx::query_scalar!("SELECT approval_kind FROM bear_web_fetches WHERE bear_id = $1 ORDER BY fetched_at DESC LIMIT 1", bear).fetch_one(&pool).await.unwrap(), "hat_host");
    for target in [
        "https://other.example.com/page",
        "http://example.com/page",
        "https://example.com:8443/page",
    ] {
        assert_eq!(
            fetcher.decide_fetch_approval(&ctx, target).await.unwrap().1,
            WebApproval::RequiresApproval,
            "{target}"
        );
    }
    let mut other_hat = ctx.clone();
    other_hat.conversation_id = "hat-fetch-other".into();
    other_hat.user_id = admin;
    assert_eq!(
        fetcher
            .decide_fetch_approval(&other_hat, url)
            .await
            .unwrap()
            .1,
        WebApproval::RequiresApproval
    );
    assert!(fetcher
        .decide_fetch_approval(&ctx_with_bear(&ctx, other_bear), url)
        .await
        .is_err());
    let mut impostor = ctx.clone();
    impostor.user_id = admin;
    assert!(fetcher.decide_fetch_approval(&impostor, url).await.is_err());
    access::revoke(&pool, BearId::new(bear), a.id, UserId::new(admin), host_id)
        .await
        .unwrap();
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::RequiresApproval
    );
    let approval_id = create_native_approval(
        &pool,
        bear,
        "hat-fetch-own",
        "hat-fetch-client",
        "hat-fetch-call",
        "web_fetch",
        &json!({"url": url}),
    )
    .await
    .unwrap();
    decide_native_approval(&pool, &approval_id, NativeApprovalDecision::Approve, None)
        .await
        .unwrap();
    let request_id = Uuid::new_v4();
    sqlx::query!(
        "UPDATE runtime_approvals SET execution_request_id = $1 WHERE approval_id = $2",
        request_id,
        approval_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    ctx.request_id = Some(request_id.to_string());
    assert_eq!(
        fetcher
            .decide_fetch_approval(&other_hat, url)
            .await
            .unwrap()
            .1,
        WebApproval::RequiresApproval,
        "an approval from another hat's conversation cannot be replayed",
    );
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::ApprovedOnce,
    );
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::RequiresApproval,
        "the same request may not run twice",
    );
    access::grant(
        &pool,
        BearId::new(bear),
        a.id,
        UserId::new(admin),
        &host,
        true,
    )
    .await
    .unwrap();
    sqlx::query!("UPDATE bear_web_sources SET policy = 'blocked' WHERE bear_id = $1 AND scope_value = 'example.com'", bear).execute(&pool).await.unwrap();
    sqlx::query!("INSERT INTO bear_web_sources (bear_id, scope_kind, scope_value, policy) VALUES ($1, 'url', $2, 'allowed')", bear, url).execute(&pool).await.unwrap();
    assert_eq!(
        fetcher.decide_fetch_approval(&ctx, url).await.unwrap().1,
        WebApproval::Blocked,
        "a specific URL allow cannot override an explicit host block or a hat grant",
    );
    db::revoke_membership(&pool, member, bear).await.unwrap();
    assert!(fetcher.decide_fetch_approval(&ctx, url).await.is_err());
}

#[sqlx::test]
async fn concurrent_first_hat_and_legacy_approval_leave_no_unscoped_access(pool: PgPool) {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats;

    let bear = db::create_bear(
        &pool,
        db::BearParams {
            slug: "racehatweb",
            name: "Race web approvals",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = sqlx::query_scalar!("INSERT INTO users (username, email) VALUES ('racehatadmin', 'racehatadmin@example.test') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    db::grant_membership(&pool, admin, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let (hat, approval) = tokio::join!(
        hats::create_hat(
            &pool,
            BearId::new(bear),
            UserId::new(admin),
            "Research",
            "Research"
        ),
        web_policy::record_web_approval(
            &pool,
            bear,
            "host",
            "example.com",
            Some(admin),
            "admin",
            None
        ),
    );
    hat.unwrap();
    // Either the approval won the lock and was revoked by hat creation, or it
    // lost the lock and was refused. Neither order leaves an active grant.
    assert!(approval.is_ok() || matches!(approval, Err(CustomError::ValidationError(_))));
    assert_eq!(
        sqlx::query_scalar!("SELECT count(*) AS \"count!: i64\" FROM bear_web_approvals WHERE bear_id = $1 AND revoked_at IS NULL", bear)
            .fetch_one(&pool).await.unwrap(),
        0,
    );
}

fn ctx_with_bear(ctx: &DenToolInvocationContext, bear: Uuid) -> DenToolInvocationContext {
    let mut other = ctx.clone();
    other.bear_id = bear;
    other
}
