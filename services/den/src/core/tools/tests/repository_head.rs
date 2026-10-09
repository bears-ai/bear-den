//! Routed default-backend and persistence/model-projection boundary regressions.
mod native;
mod native_fixture;
mod native_provider;
use crate::{
    config::Config,
    core::tools::session::{invoke_den_tool_for_origin, DenToolInvocationContext},
};
use den_core::{
    ids::{BearId, UserId},
    tools::{
        constants::DEN_REPOSITORY_HEAD, repository::RepositorySurfaceId,
        result_compaction::ToolResultStatus,
    },
    Governance, TurnExecutionOrigin,
};
use den_service::{
    bears::{db, hats},
    connections,
    conversation::{
        events::{
            self, CanonicalConversationRecord, CanonicalToolRequestRecord,
            CanonicalToolResultRecord, ConversationEventProvenance, ConversationPersistenceContext,
        },
        persistence,
    },
    repository::grants,
    work_surfaces,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn routed_operation_persists_only_closed_outputs_and_next_request_has_no_credential(
    pool: PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    const CANARY: &str = "ghp_runtime_canary_NEVER_PROJECT";
    let bear = BearId::new(
        db::create_bear(
            &pool,
            db::BearParams {
                slug: "repository-projection",
                name: "Repository projection",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await?,
    );
    let actor = UserId::new(sqlx::query_scalar!("INSERT INTO users (username,email) VALUES ('repositoryprojection','repositoryprojection@test.invalid') RETURNING id").fetch_one(&pool).await?);
    db::grant_membership(
        &pool,
        actor.get(),
        bear.as_uuid(),
        Some(db::BEAR_ROLE_ADMIN),
    )
    .await?;
    let conversation = persistence::ensure_conversation_for_external_id(
        &pool,
        bear.as_uuid(),
        Some(actor.get()),
        "den-conv-repository-projection",
        None,
        None,
    )
    .await?;
    let hat = hats::create_hat(
        &pool,
        bear,
        actor,
        "Repository",
        "Read bounded upstream evidence",
    )
    .await?;
    hats::bindings::bind_conversation_hat(&pool, bear, conversation.id, hat.id).await?;
    let surface = work_surfaces::create_surface(
        &pool,
        actor.get(),
        work_surfaces::NewWorkSurface {
            name: "repository-projection".into(),
            description: None,
            upstream_url: "https://github.com/acme/widget".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec!["api.github.com".into()],
            credential: None,
        },
        "unused",
    )
    .await?;
    work_surfaces::assign_bear(&pool, surface.id, bear.as_uuid(), actor.get()).await?;
    hats::manage::replace_surfaces(&pool, bear, hat.id, &[surface.id]).await?;
    let reference =
        den_service::repository::ExternalReference::new(Uuid::new_v4(), Uuid::new_v4(), 1)?;
    let backend_id = reference.backend_binding_id();
    let secret_id = reference.secret_id();
    let connection = connections::create(
        &pool,
        actor,
        "External reference",
        connections::Material::ExternalReference(reference),
        "unused",
    )
    .await?;
    connections::attach(&pool, actor, connection, surface.id).await?;
    hats::access::grant(
        &pool,
        bear,
        hat.id,
        actor,
        &hats::access::HatAccessGrant::HttpsHost(hats::access::HttpsHost::parse("api.github.com")?),
        true,
    )
    .await?;
    let choice = grants::choices(&pool, bear, hat.id, actor).await?.remove(0);
    grants::grant(
        &pool,
        bear,
        hat.id,
        actor,
        RepositorySurfaceId(surface.id),
        &choice.target_key,
        true,
    )
    .await?;
    let context: DenToolInvocationContext = serde_json::from_value(
        json!({"bear_id":bear,"bear_slug":"repository-projection","binding_id":hats::turn_binding::NativeTurnSource::Conversation(conversation.id).binding_id(bear),"profile":den_core::RuntimeContextLabel::ChannelConversation,"user_id":actor,"conversation_id":"den-conv-repository-projection","session_id":"repository-projection-session"}),
    )?;
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("repository-projection-{}", Uuid::new_v4()))
        .display()
        .to_string();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let args = json!({"work_surface_id":surface.id});
    let result = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        "repository_head",
        args.clone(),
        context.clone(),
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
    )
    .await?;
    assert_eq!(result, json!({"error":{"code":"credential_unavailable"}}));
    let bad_args = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        DEN_REPOSITORY_HEAD,
        json!({"work_surface_id":surface.id,"url":"https://evil.test"}),
        context.clone(),
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
    )
    .await?;
    assert_eq!(bad_args, json!({"error":{"code":"invalid_arguments"}}));
    let persisted = ConversationPersistenceContext {
        pool: pool.clone(),
        bear_id: bear.as_uuid(),
        user_id: Some(actor.get()),
        external_conversation_id: context.conversation_id.clone(),
        source_session_id: Some(context.session_id.clone()),
        request_id: Some("repository-request".into()),
        persistence_scope_id: "repository-test".into(),
        skip_persistence: false,
    };
    let provenance = ConversationEventProvenance::client_session(&context.session_id);
    events::persist_canonical_conversation_record(
        &persisted,
        &CanonicalConversationRecord::tool_request(
            CanonicalToolRequestRecord::new(
                "repository_head",
                "repository-call",
                "repository-request",
                None,
                args,
                false,
                None,
                "den",
            ),
            &provenance,
        ),
    )
    .await?;
    events::persist_canonical_conversation_record(
        &persisted,
        &CanonicalConversationRecord::tool_result(
            CanonicalToolResultRecord::new(
                Some("repository_head".into()),
                "repository-call",
                None,
                ToolResultStatus::Error,
                Some(result.to_string()),
                result,
                json!({}),
                Some("repository-request".into()),
            ),
            &provenance,
        ),
    )
    .await?;
    let rows = persistence::list_messages_page(&pool, conversation.id, None, 100).await?;
    assert!(rows.iter().any(|row| row
        .content_json
        .to_string()
        .contains("credential_unavailable")));
    for projection in [
        persistence::ConversationHistoryProjection::UserHistory,
        persistence::ConversationHistoryProjection::ModelTranscript,
    ] {
        let projected = persistence::list_projected_messages_page(
            &pool,
            conversation.id,
            None,
            100,
            projection,
        )
        .await?;
        for row in projected {
            assert!(!row.content_json.to_string().contains(CANARY));
        }
    }
    let messages = den_runtime::agent_loop::assemble_agent_messages(
        &pool,
        bear.as_uuid(),
        &context.conversation_id,
        Some("Continue the owned conversation"),
        Some("What was the outcome?"),
        &[],
    )
    .await?;
    let request = den_runtime::llm::ChatCompletionRequest {
        model: "openai/test".into(),
        messages,
        tools: vec![],
        stream: false,
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        thinking_effort: None,
        telemetry: None,
    };
    for body in [request.to_body(), request.to_responses_body()] {
        let body = body.to_string();
        for forbidden in [CANARY, &backend_id.to_string(), &secret_id.to_string()] {
            assert!(!body.contains(forbidden));
        }
    }
    let memories = invoke_den_tool_for_origin(
        &pool,
        &config,
        &stores,
        "memory_search",
        json!({"query":CANARY}),
        context,
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
    )
    .await?;
    // Search arguments themselves can be echoed; only actual entry bodies matter.
    assert!(memories
        .get("results")
        .is_none_or(|results| results.as_array().is_some_and(Vec::is_empty)));
    Ok(())
}
