use crate::{
    bears::{db, hats},
    personal_memory as service,
};
use den_core::{
    config::Config,
    ids::{BearId, HatId, UserId},
    DenError,
};
use den_memory::{
    append_memory_record, resolve_authenticated_user_entity, AuthenticatedUserEntity,
    BearMemoryStore, LogicalMemoryPath, MemorySource, MemoryStoreManager,
};
use serde_json::json;
use sqlx::PgPool;
use std::path::PathBuf;
use uuid::Uuid;

pub(super) struct Fixture {
    pub pool: PgPool,
    pub stores: MemoryStoreManager,
    pub bear: BearId,
    pub owner: UserId,
    pub other: UserId,
    pub outsider: UserId,
    pub hat: HatId,
    pub conversation: Uuid,
    pub source: Uuid,
    _directory: MemoryDirectory,
}

struct MemoryDirectory(PathBuf);

impl Drop for MemoryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) async fn new_bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        db::create_bear(
            pool,
            db::BearParams {
                slug,
                name: "Personal candidate test",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    )
}

async fn new_user(pool: &PgPool, username: &str) -> UserId {
    UserId::new(
        sqlx::query_scalar!(
            "INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id",
            username,
            format!("{username}@example.test"),
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

pub(super) async fn conversation(
    pool: &PgPool,
    bear: BearId,
    owner: Option<UserId>,
    hat: Option<HatId>,
) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO conversations (bear_id, created_by_user_id, hat_id) VALUES ($1, $2, $3) RETURNING id",
        bear.as_uuid(),
        owner.map(UserId::get),
        hat.map(HatId::as_uuid),
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

impl Fixture {
    pub async fn new(pool: PgPool) -> Self {
        let bear = new_bear(&pool, "personal-candidate").await;
        let owner = new_user(&pool, "personalowner").await;
        let other = new_user(&pool, "personalother").await;
        let outsider = new_user(&pool, "personaloutsider").await;
        for (user, role) in [(owner, db::BEAR_ROLE_MEMBER), (other, db::BEAR_ROLE_ADMIN)] {
            db::grant_membership(&pool, user.get(), bear.as_uuid(), Some(role))
                .await
                .unwrap();
        }
        let hat = hats::create_hat(&pool, bear, owner, "Personal test", "Organize context")
            .await
            .unwrap();
        assert!(
            !hat.auto_curate_enabled,
            "private capture needs no sharing opt-in"
        );
        let conversation = conversation(&pool, bear, Some(owner), Some(hat.id)).await;
        let directory = MemoryDirectory(
            std::env::temp_dir().join(format!("personal-service-{}", Uuid::new_v4())),
        );
        let mut config = Config::test_stub();
        config.bear_sqlite_data_dir = directory.0.to_string_lossy().into_owned();
        let stores = MemoryStoreManager::new(&config);
        let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
        let source = append_memory_record(
            &store,
            &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
            "note",
            "pair",
            None,
            "Private evidence stays in the source, not in the candidate payload.",
            &json!({}),
        )
        .await
        .unwrap();
        Self {
            pool,
            stores,
            bear,
            owner,
            other,
            outsider,
            hat: hat.id,
            conversation,
            source: Uuid::parse_str(&source.memory_id).unwrap(),
            _directory: directory,
        }
    }

    pub async fn store(&self) -> BearMemoryStore {
        self.stores
            .store_for_bear(self.bear.as_uuid())
            .await
            .unwrap()
    }

    pub async fn binding(&self, user: UserId) -> AuthenticatedUserEntity {
        resolve_authenticated_user_entity(&self.store().await, user)
            .await
            .unwrap()
    }

    pub async fn enable(&self, user: UserId) {
        let store = self.store().await;
        let binding = self.binding(user).await;
        den_memory::personal_candidates::set_capture_enabled(&store, &binding, true)
            .await
            .unwrap();
    }

    pub async fn stage_as(
        &self,
        user: UserId,
        conversation: Uuid,
        source: Uuid,
    ) -> Result<service::PersonalCandidate, DenError> {
        service::stage_preferred_name(
            &self.pool,
            &self.stores,
            self.bear,
            user,
            conversation,
            source,
            service::PreferredName::new("Alice").unwrap(),
        )
        .await
    }
}

pub(super) fn assert_denied<T>(result: Result<T, DenError>) {
    assert!(matches!(result, Err(DenError::Authorization(_))));
}
