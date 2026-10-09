use crate::config::Config;
use den_core::{
    ids::{BearId, UserId},
    tools::repository::RepositorySurfaceId,
    AgentLoopControlLevel,
};
use den_repository::{CredentialRequest, ExternalReference};
use den_service::{
    bears::{db, hats, model_configurations},
    connections,
    conversation::persistence,
    repository::grants,
    work_surfaces, DenState,
};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub(super) struct Fixture {
    pub state: DenState,
    pub bear: BearId,
    pub slug: String,
    pub actor: UserId,
    pub canonical: Uuid,
    pub conversation: String,
    pub session: String,
    pub binding: String,
    pub surface: RepositorySurfaceId,
    pub credential: CredentialRequest,
}

pub(super) async fn fixture(pool: &PgPool, model_url: &str) -> Fixture {
    let nonce = Uuid::new_v4();
    let compact = nonce.simple().to_string();
    let suffix = &compact[..16];
    let actor_name = format!("reponative{suffix}");
    assert!(actor_name.len() <= 30 && actor_name.bytes().all(|ch| ch.is_ascii_alphanumeric()));
    let slug = format!("reponative{suffix}");
    let bear = BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug: &slug,
                name: "Repository native test",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    );
    let actor = UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users (username,email) VALUES ($1,$2) RETURNING id",
            &actor_name,
            format!("{actor_name}@test.invalid")
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    db::grant_membership(pool, actor.get(), bear.as_uuid(), Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let hat = hats::create_hat(
        pool,
        bear,
        actor,
        "Upstream review",
        "Read one authorized repository head",
    )
    .await
    .unwrap();
    let model = model_configurations::create(pool, bear, "Mock primary", "gpt-4.1", None)
        .await
        .unwrap();
    model_configurations::set_default(pool, bear, Some(model.id))
        .await
        .unwrap();
    db::set_bear_agent_loop_control_setting(
        pool,
        bear.as_uuid(),
        Some(AgentLoopControlLevel::Light),
    )
    .await
    .unwrap();
    let conversation = format!("den-conv-{nonce}");
    let session = format!("native-repo-session-{nonce}");
    let canonical = persistence::ensure_conversation_for_external_id(
        pool,
        bear.as_uuid(),
        Some(actor.get()),
        &conversation,
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(pool, bear, canonical.id, hat.id)
        .await
        .unwrap();
    let binding = hats::turn_binding::NativeTurnSource::Conversation(canonical.id).binding_id(bear);
    let surface = RepositorySurfaceId(
        work_surfaces::create_surface(
            pool,
            actor.get(),
            work_surfaces::NewWorkSurface {
                name: format!("reponative{suffix}"),
                description: None,
                upstream_url: "https://github.com/acme/widget.git".into(),
                default_ref: "main".into(),
                default_image: None,
                allowed_outbound_hosts: vec!["api.github.com".into()],
                credential: None,
            },
            "unused",
        )
        .await
        .unwrap()
        .id,
    );
    work_surfaces::assign_bear(pool, surface.0, bear.as_uuid(), actor.get())
        .await
        .unwrap();
    hats::manage::replace_surfaces(pool, bear, hat.id, &[surface.0])
        .await
        .unwrap();
    let connection = connections::create(
        pool,
        actor,
        "External native test reference",
        connections::Material::ExternalReference(
            ExternalReference::new(Uuid::new_v4(), Uuid::new_v4(), 1).unwrap(),
        ),
        "unused",
    )
    .await
    .unwrap();
    connections::attach(pool, actor, connection, surface.0)
        .await
        .unwrap();
    hats::access::grant(
        pool,
        bear,
        hat.id,
        actor,
        &hats::access::HatAccessGrant::HttpsHost(
            hats::access::HttpsHost::parse("api.github.com").unwrap(),
        ),
        true,
    )
    .await
    .unwrap();
    let target = grants::choices(pool, bear, hat.id, actor)
        .await
        .unwrap()
        .remove(0);
    grants::grant(pool, bear, hat.id, actor, surface, &target.target_key, true)
        .await
        .unwrap();
    let credential = connections::external::for_owner(pool, actor, surface)
        .await
        .unwrap()
        .credential;
    let mut config = Config::test_stub();
    config.llm_api_url = model_url.into();
    config.default_llm_model = "openai/gpt-4.1".into();
    config.den_secret_encryption_key = "native-repository-test-encryption-key".into();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("native-repository-{nonce}"))
        .display()
        .to_string();
    db::set_bear_bifrost_virtual_key(
        pool,
        bear.as_uuid(),
        Some("vk-native-repository"),
        Some("native-test"),
        Some("sk-bf-native-repository-test"),
        &config.den_secret_encryption_key,
    )
    .await
    .unwrap();
    let record = db::get_bear(pool, bear.as_uuid()).await.unwrap().unwrap();
    den_service::bears::compile_and_store_managed_config_for_bear(pool, &record)
        .await
        .unwrap();
    let config = Arc::new(config);
    let stores = den_memory::MemoryStoreManager::new(&config);
    let state = DenState::new(
        pool.clone(),
        config.clone(),
        Arc::new(den_service::bifrost::BifrostClient::new(&config)),
        stores,
    );
    Fixture {
        state,
        bear,
        slug,
        actor,
        canonical: canonical.id,
        conversation,
        session,
        binding,
        surface,
        credential,
    }
}
