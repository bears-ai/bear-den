use super::*;
use den_core::config::Config;

#[test]
fn generation_exhaustion_fails_closed_without_wrapping_or_allowing_old_publication() {
    let client = BifrostClient::new(&Config::test_stub());
    let bear_id = Uuid::nil();
    let mut old = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve old refresh"));
    {
        let mut states = client.bear_catalogs.write().unwrap();
        states.get_mut(&bear_id).unwrap().generation = u64::MAX;
    }
    old.generation = u64::MAX;
    assert!(client.begin_catalog_refresh(bear_id).is_err());
    assert!(matches!(
        client.publish_catalog(
            old,
            CredentialFingerprint::for_key("sk-bf-test"),
            BifrostCatalogSnapshot::from_available_models(vec![]),
        ),
        Err(RefreshFailure::Superseded)
    ));
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_none());
}

#[test]
fn generation_tombstones_prevent_aba_and_fence_old_publication_and_invalidation() {
    let client = BifrostClient::new(&Config::test_stub());
    let bear_id = Uuid::nil();
    let credential = CredentialFingerprint::for_key("sk-bf-test");
    let old = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve old refresh"));
    let rejected = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve newer refresh"));
    client
        .invalidate_catalog(rejected)
        .unwrap_or_else(|_| panic!("invalidate current refresh"));
    let latest = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve latest refresh"));
    assert!(latest.generation > rejected.generation);
    assert!(rejected.generation > old.generation);
    client
        .publish_catalog(
            latest,
            credential,
            BifrostCatalogSnapshot::from_available_models(vec![]),
        )
        .unwrap_or_else(|_| panic!("publish latest catalog"));
    assert!(matches!(
        client.publish_catalog(old, credential, BifrostCatalogSnapshot::default()),
        Err(RefreshFailure::Superseded)
    ));
    assert!(matches!(
        client.invalidate_catalog(old),
        Err(RefreshFailure::Superseded)
    ));
    assert!(
        matches!(
            client.finish_catalog_outage(old, credential),
            Err(RefreshFailure::Superseded)
        ),
        "supersession is not a genuine outage"
    );
    assert!(!client.cached_bear_catalog_snapshot(bear_id).unwrap().stale);
}

#[test]
fn a_generation_reserved_before_credential_lookup_fences_that_lookups_late_invalidation() {
    let client = BifrostClient::new(&Config::test_stub());
    let bear_id = Uuid::nil();
    let before_lookup = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve before lookup"));
    let newer = client
        .begin_catalog_refresh(bear_id)
        .unwrap_or_else(|_| panic!("reserve newer lookup"));
    client
        .publish_catalog(
            newer,
            CredentialFingerprint::for_key("sk-bf-new"),
            BifrostCatalogSnapshot::from_available_models(vec![]),
        )
        .unwrap_or_else(|_| panic!("publish newer catalog"));
    assert!(matches!(
        client.invalidate_catalog(before_lookup),
        Err(RefreshFailure::Superseded)
    ));
    assert!(client.cached_bear_catalog_snapshot(bear_id).is_some());
}
