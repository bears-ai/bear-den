use crate::{
    bears::{db, hats},
    connections::{self, ConnectionId, Material},
    conversation::persistence,
    work_surfaces,
};
use den_core::{
    ids::{BearId, HatId, UserId},
    tools::{context::DenToolInvocationContext, repository::RepositorySurfaceId},
    EffectivePolicy, Governance, TurnExecutionOrigin,
};
use den_repository::ExternalReference;
use sqlx::PgPool;
use uuid::Uuid;

pub(super) struct Fixture {
    pub bear: BearId,
    pub owner: UserId,
    pub other: UserId,
    pub hat: HatId,
    pub other_hat: HatId,
    pub conversation: Uuid,
    pub surface: RepositorySurfaceId,
    pub connection: ConnectionId,
    pub grant: Uuid,
    pub context: DenToolInvocationContext,
}

pub(super) async fn context(
    pool: &PgPool,
    bear: BearId,
    owner: UserId,
    hat: HatId,
) -> (DenToolInvocationContext, Uuid) {
    let external = format!("den-conv-{}", Uuid::new_v4());
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        bear.as_uuid(),
        Some(owner.get()),
        &external,
        None,
        None,
    )
    .await
    .unwrap();
    hats::bindings::bind_conversation_hat(pool, bear, conversation.id, hat)
        .await
        .unwrap();
    let context = serde_json::from_value(serde_json::json!({
        "bear_id":bear, "bear_slug":"repo-test", "binding_id":hats::turn_binding::NativeTurnSource::Conversation(conversation.id).binding_id(bear),
        "profile":EffectivePolicy::compile_for_origin(TurnExecutionOrigin::ChannelConversation, Governance::Interactive).context_label,
        "user_id":owner, "conversation_id":external, "session_id":format!("repo-client-{}", Uuid::new_v4())
    })).unwrap();
    (context, conversation.id)
}

pub(super) async fn fixture(pool: &PgPool) -> Fixture {
    let nonce = Uuid::new_v4();
    let compact = nonce.simple().to_string();
    let suffix = &compact[..16];
    let owner_name = format!("repoowner{suffix}");
    let other_name = format!("repoother{suffix}");
    for name in [&owner_name, &other_name] {
        assert!(name.len() <= 30 && name.bytes().all(|ch| ch.is_ascii_alphanumeric()));
    }
    let bear = BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug: &format!("repo{suffix}"),
                name: "Repository test",
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
    let owner = UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users (username,email) VALUES ($1,$2) RETURNING id",
            &owner_name,
            format!("{owner_name}@test.invalid")
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    let other = UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users (username,email) VALUES ($1,$2) RETURNING id",
            &other_name,
            format!("{other_name}@test.invalid")
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    );
    for user in [owner, other] {
        db::grant_membership(pool, user.get(), bear.as_uuid(), Some(db::BEAR_ROLE_ADMIN))
            .await
            .unwrap();
    }
    let hat = hats::create_hat(
        pool,
        bear,
        owner,
        "Repository reviewer",
        "Read bounded upstream evidence",
    )
    .await
    .unwrap()
    .id;
    let other_hat = hats::create_hat(
        pool,
        bear,
        owner,
        "Other responsibility",
        "No repository grant",
    )
    .await
    .unwrap()
    .id;
    let surface = RepositorySurfaceId(
        work_surfaces::create_surface(
            pool,
            owner.get(),
            work_surfaces::NewWorkSurface {
                name: format!("repo{suffix}"),
                description: None,
                upstream_url: "https://github.com/acme/widget.git".into(),
                default_ref: "main".into(),
                default_image: None,
                allowed_outbound_hosts: vec!["api.github.com".into()],
                credential: None,
            },
            "unused-secret-key",
        )
        .await
        .unwrap()
        .id,
    );
    work_surfaces::assign_bear(pool, surface.0, bear.as_uuid(), owner.get())
        .await
        .unwrap();
    hats::manage::replace_surfaces(pool, bear, hat, &[surface.0])
        .await
        .unwrap();
    let connection = connections::create(
        pool,
        owner,
        "External test reference",
        Material::ExternalReference(
            ExternalReference::new(Uuid::new_v4(), Uuid::new_v4(), 1).unwrap(),
        ),
        "unused",
    )
    .await
    .unwrap();
    connections::attach(pool, owner, connection, surface.0)
        .await
        .unwrap();
    hats::access::grant(
        pool,
        bear,
        hat,
        owner,
        &hats::access::HatAccessGrant::HttpsHost(
            hats::access::HttpsHost::parse("api.github.com").unwrap(),
        ),
        true,
    )
    .await
    .unwrap();
    let choice = super::super::grants::choices(pool, bear, hat, owner)
        .await
        .unwrap()
        .remove(0);
    let grant =
        super::super::grants::grant(pool, bear, hat, owner, surface, &choice.target_key, true)
            .await
            .unwrap();
    let (context, conversation) = context(pool, bear, owner, hat).await;
    Fixture {
        bear,
        owner,
        other,
        hat,
        other_hat,
        conversation,
        surface,
        connection,
        grant,
        context,
    }
}
