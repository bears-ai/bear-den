use super::{
    assert_denied,
    fixture::{context, fixture},
    CountingUnavailable,
};
use crate::{
    bears::hats,
    connections,
    repository::{head_with_resolver, RepositoryHeadPolicy},
    work_surfaces,
};
use den_core::{tools::repository::RepositoryError, Governance, TurnExecutionOrigin};
use den_repository::RepositoryAuthorizer;
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};

#[sqlx::test(migrations = "../../migrations")]
async fn two_humans_hats_bears_and_inspection_cannot_borrow_credentials(pool: PgPool) {
    let f = fixture(&pool).await;
    assert_eq!(
        f.context.binding_id,
        hats::turn_binding::NativeTurnSource::Conversation(f.conversation).binding_id(f.bear)
    );
    let policy = RepositoryHeadPolicy {
        pool: &pool,
        context: &f.context,
        origin: TurnExecutionOrigin::ChannelConversation,
        governance: Governance::Interactive,
    };
    assert!(policy.authorize(f.surface).await.is_ok());
    let backend = CountingUnavailable(AtomicUsize::new(0));
    assert_eq!(
        head_with_resolver(
            &pool,
            &f.context,
            TurnExecutionOrigin::ChannelConversation,
            Governance::Interactive,
            f.surface,
            &backend
        )
        .await
        .err(),
        Some(RepositoryError::CredentialUnavailable)
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    let (other_owner, _) = context(&pool, f.bear, f.other, f.hat).await;
    assert_denied(
        &pool,
        &other_owner,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    let mut inspection = f.context.clone();
    inspection.user_id = f.other.get();
    inspection.membership_role = Some("admin".into());
    assert_denied(
        &pool,
        &inspection,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    let (other_hat, _) = context(&pool, f.bear, f.owner, f.other_hat).await;
    assert_denied(
        &pool,
        &other_hat,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    let second = fixture(&pool).await;
    let mut foreign = f.context.clone();
    foreign.bear_id = second.bear.as_uuid();
    assert_denied(
        &pool,
        &foreign,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert_denied(&pool, &f.context, f.surface, origin).await;
    }
    sqlx::query!(
        "DELETE FROM user_bear WHERE bear_id=$1 AND user_id=$2",
        f.bear.as_uuid(),
        f.owner.get()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_live_resource_and_grant_dimension_can_only_narrow_authority(pool: PgPool) {
    let f = fixture(&pool).await;
    let check = || RepositoryHeadPolicy {
        pool: &pool,
        context: &f.context,
        origin: TurnExecutionOrigin::ChannelConversation,
        governance: Governance::Interactive,
    };
    sqlx::query!(
        "UPDATE git_work_surface_details SET default_ref='other' WHERE id=$1",
        f.surface.0
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    sqlx::query!(
        "UPDATE git_work_surface_details SET default_ref='main' WHERE id=$1",
        f.surface.0
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(check().authorize(f.surface).await.is_ok());
    sqlx::query!(
        "UPDATE provider_connections SET external_secret_version=2 WHERE id=$1",
        f.connection.0
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    sqlx::query!(
        "UPDATE provider_connections SET external_secret_version=1 WHERE id=$1",
        f.connection.0
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE git_work_surface_details SET allowed_outbound_hosts=ARRAY[]::text[] WHERE id=$1",
        f.surface.0
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    sqlx::query!("UPDATE git_work_surface_details SET allowed_outbound_hosts=ARRAY['api.github.com']::text[] WHERE id=$1", f.surface.0).execute(&pool).await.unwrap();
    sqlx::query!("INSERT INTO bear_web_sources (bear_id,scope_kind,scope_value,policy) VALUES ($1,'host','api.github.com','blocked')", f.bear.as_uuid()).execute(&pool).await.unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    sqlx::query!(
        "DELETE FROM bear_web_sources WHERE bear_id=$1",
        f.bear.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    work_surfaces::unassign_bear(&pool, f.surface.0, f.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    work_surfaces::assign_bear(&pool, f.surface.0, f.bear.as_uuid(), f.owner.get())
        .await
        .unwrap();
    hats::manage::replace_surfaces(&pool, f.bear, f.hat, &[])
        .await
        .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    hats::manage::replace_surfaces(&pool, f.bear, f.hat, &[f.surface.0])
        .await
        .unwrap();
    sqlx::query!("UPDATE bear_hat_access_grants SET revoked_at=now() WHERE bear_id=$1 AND hat_id=$2 AND kind='network'", f.bear.as_uuid(), f.hat.as_uuid()).execute(&pool).await.unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    sqlx::query!("UPDATE bear_hat_access_grants SET revoked_at=NULL WHERE bear_id=$1 AND hat_id=$2 AND kind='network'", f.bear.as_uuid(), f.hat.as_uuid()).execute(&pool).await.unwrap();
    hats::access::revoke(&pool, f.bear, f.hat, f.owner, f.grant)
        .await
        .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn external_reconciliation_excludes_secret_and_locator_material_and_legacy_is_ineligible(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    let reference = crate::connections::external::for_owner(&pool, f.owner, f.surface)
        .await
        .unwrap()
        .credential
        .reference;
    let export = work_surfaces::build_managed_config(&pool, "unused-secret-key")
        .await
        .unwrap();
    let repository = work_surfaces::surface_by_id(&pool, f.surface.0)
        .await
        .unwrap()
        .unwrap();
    assert!(!export
        .surfaces
        .iter()
        .any(|surface| surface.name == repository.name));
    let export = serde_json::to_string(&export).unwrap();
    assert!(!export.contains(&reference.secret_id().to_string()));
    assert!(!export.contains(&reference.backend_binding_id().to_string()));
    assert!(connections::require_live_for_surface(&pool, f.surface.0)
        .await
        .is_err());
    let legacy = connections::create(
        &pool,
        f.owner,
        "Legacy token",
        connections::Material::HttpsToken("ghp_legacy_canary".into()),
        "a-test-encryption-key-long-enough",
    )
    .await
    .unwrap();
    connections::attach(&pool, f.owner, legacy, f.surface.0)
        .await
        .unwrap();
    assert_denied(
        &pool,
        &f.context,
        f.surface,
        TurnExecutionOrigin::ChannelConversation,
    )
    .await;
    let records = connections::list(&pool, f.owner).await.unwrap();
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("ghp_legacy_canary"));
}

struct RevokeDuringLookup {
    pool: PgPool,
    connection: connections::ConnectionId,
}
#[async_trait::async_trait]
impl den_repository::ExternalCredentialResolver for RevokeDuringLookup {
    async fn resolve(
        &self,
        request: &den_repository::CredentialRequest,
    ) -> Result<den_repository::CredentialLease, RepositoryError> {
        connections::revoke(
            &self.pool,
            request.owner,
            self.connection,
            request.connection_revision,
        )
        .await
        .unwrap();
        den_repository::CredentialLease::new(
            request.clone(),
            secrecy::SecretString::from("ghp_lookup_canary".to_owned()),
            std::time::Duration::from_secs(30),
        )
    }
    async fn validate(&self, _: &den_repository::CredentialLease) -> Result<(), RepositoryError> {
        Ok(())
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn revocation_during_secret_lookup_is_rechecked_before_dns_or_http(pool: PgPool) {
    let f = fixture(&pool).await;
    let result = head_with_resolver(
        &pool,
        &f.context,
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
        f.surface,
        &RevokeDuringLookup {
            pool: pool.clone(),
            connection: f.connection,
        },
    )
    .await;
    let error = result.err().unwrap();
    assert_eq!(error, RepositoryError::ConnectionUnavailable);
    assert!(!format!("{error:?} {error}").contains("ghp_lookup_canary"));
}
