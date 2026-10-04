//! Assertion-style contract checks — the Phase 0 exit gate of
//! `docs/roadmap/CABINET_IMPLEMENTATION_PLAN.md`, enforcing the rules in
//! `docs/architecture/cabinet-contract.md` independent of any provider.
//!
//! Immutability of a finalized version is enforced at compile time:
//! `ItemVersion` has private fields and no mutating API, so there is no
//! runtime mutation case to exercise here.

use den_cabinet::{
    validate_artifact_ref, validate_source_locator, ActorScope, CabinetAttachmentRef,
    CabinetCollectionRef, CabinetItem, CabinetItemRef, CabinetReviewRef, CabinetSourceRef,
    CabinetVersionRef, ContractViolation, CreateItemRequest, HistoryRequest, ItemVersion,
    MissionRef, ReadRequest, ReviewState, SearchRequest, SourceKind, UpdateItemRequest,
};
use den_core::execution_context::RuntimeContextLabel;
use den_core::ids::{BearId, UserId};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

fn user_scope() -> ActorScope {
    ActorScope::user(UserId(7))
}

fn bear_scope() -> ActorScope {
    ActorScope::bear(
        BearId::new(Uuid::new_v4()),
        RuntimeContextLabel::ChannelConversation,
    )
}

// --- refs: mint/parse round-trips, malformed and cross-kind rejection ---

macro_rules! assert_ref_contract {
    ($name:ident, $prefix:literal) => {
        let minted = $name::mint();
        assert!(minted.as_str().starts_with($prefix));
        let parsed = $name::parse(minted.as_str()).expect("round-trip parse");
        assert_eq!(parsed, minted);
        let json = serde_json::to_string(&minted).expect("serialize");
        let back: $name = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, minted);

        // Malformed: wrong prefix, short suffix, uppercase hex, non-hex.
        assert!($name::parse("nonsense").is_err());
        assert!($name::parse(&format!("{}{}", $prefix, "abc")).is_err());
        assert!($name::parse(&format!("{}{}", $prefix, "A".repeat(32))).is_err());
        assert!($name::parse(&format!("{}{}", $prefix, "z".repeat(32))).is_err());
    };
}

#[test]
fn refs_mint_parse_and_reject_malformed() {
    assert_ref_contract!(CabinetItemRef, "cabinet_item_");
    assert_ref_contract!(CabinetVersionRef, "cabinet_version_");
    assert_ref_contract!(CabinetCollectionRef, "cabinet_collection_");
    assert_ref_contract!(MissionRef, "mission_");
    assert_ref_contract!(CabinetSourceRef, "cabinet_source_");
    assert_ref_contract!(CabinetAttachmentRef, "cabinet_attachment_");
    assert_ref_contract!(CabinetReviewRef, "cabinet_review_");
}

#[test]
fn refs_reject_cross_kind_values() {
    let item = CabinetItemRef::mint();
    assert!(CabinetVersionRef::parse(item.as_str()).is_err());
    assert!(MissionRef::parse(item.as_str()).is_err());
    let version = CabinetVersionRef::mint();
    assert!(CabinetItemRef::parse(version.as_str()).is_err());
}

#[test]
fn artifact_ref_validation_matches_artifact_convention() {
    assert!(validate_artifact_ref(&format!("artifact_{}", "0".repeat(32))).is_ok());
    assert!(validate_artifact_ref("artifact_short").is_err());
    assert!(validate_artifact_ref(CabinetItemRef::mint().as_str()).is_err());
}

// --- operations: every input requires an explicit actor scope ---

#[test]
fn operation_inputs_require_actor_scope() {
    // Well-formed request with scope deserializes.
    let ok = json!({
        "scope": { "actor_kind": "user", "user_id": 7 },
        "query": "deploy runbook"
    });
    assert!(serde_json::from_value::<SearchRequest>(ok).is_ok());

    // The same request without scope fails for every operation type.
    let item = CabinetItemRef::mint();
    let version = CabinetVersionRef::mint();
    assert!(serde_json::from_value::<SearchRequest>(json!({ "query": "q" })).is_err());
    assert!(
        serde_json::from_value::<ReadRequest>(json!({ "cabinet_ref": item.as_str() })).is_err()
    );
    assert!(
        serde_json::from_value::<HistoryRequest>(json!({ "cabinet_ref": item.as_str() })).is_err()
    );
    assert!(serde_json::from_value::<CreateItemRequest>(json!({
        "kind": "document",
        "title": "t",
        "content": "c"
    }))
    .is_err());
    assert!(serde_json::from_value::<UpdateItemRequest>(json!({
        "cabinet_ref": item.as_str(),
        "content": "c",
        "base_version": version.as_str()
    }))
    .is_err());
}

#[test]
fn bear_actor_requires_stance() {
    let missing_stance = json!({
        "scope": { "actor_kind": "bear", "bear_id": Uuid::new_v4() },
        "query": "q"
    });
    assert!(serde_json::from_value::<SearchRequest>(missing_stance).is_err());
}

// --- records: required identity/provenance/scope fields ---

#[test]
fn item_deserialization_rejects_missing_provenance_and_empty_title() {
    let base = json!({
        "cabinet_ref": CabinetItemRef::mint().as_str(),
        "kind": "document",
        "title": "Deploy runbook",
        "created_by": { "actor_kind": "user", "user_id": 7 },
        "created_at": "2026-08-31T12:00:00Z",
        "lifecycle": "active"
    });
    assert!(serde_json::from_value::<CabinetItem>(base.clone()).is_ok());

    let mut missing_creator = base.clone();
    missing_creator
        .as_object_mut()
        .unwrap()
        .remove("created_by");
    assert!(serde_json::from_value::<CabinetItem>(missing_creator).is_err());

    let mut empty_title = base;
    empty_title["title"] = json!("   ");
    assert!(serde_json::from_value::<CabinetItem>(empty_title).is_err());
}

#[test]
fn source_locators_must_match_their_kind() {
    assert!(validate_source_locator(SourceKind::Url, "https://example.com/a").is_ok());
    assert!(validate_source_locator(SourceKind::Url, "book://isbn/9780262046305").is_err());
    assert!(validate_source_locator(SourceKind::Offline, "book://isbn/9780262046305").is_ok());
    assert!(validate_source_locator(SourceKind::Offline, "https://example.com/a").is_err());
    assert!(validate_source_locator(SourceKind::Offline, "no-scheme").is_err());
    assert!(validate_source_locator(
        SourceKind::Artifact,
        &format!("artifact_{}", "a".repeat(32))
    )
    .is_ok());
    assert!(validate_source_locator(SourceKind::Artifact, "artifact_nope").is_err());
    assert!(validate_source_locator(SourceKind::Conversation, "").is_err());
}

// --- versions: revision rules, base_version, hash integrity, review gate ---

#[test]
fn version_rules_enforced_at_construction() {
    let now = OffsetDateTime::now_utc();
    let item = CabinetItemRef::mint();

    let first = ItemVersion::first(
        CabinetVersionRef::mint(),
        item.clone(),
        "hello".to_string(),
        user_scope(),
        now,
    )
    .expect("revision 1");
    assert_eq!(first.revision(), 1);
    assert!(first.base_version().is_none());
    assert_eq!(first.review(), ReviewState::None);
    assert_eq!(first.content_sha256().len(), 64);
    first
        .ensure_phase1_direct_edit()
        .expect("direct edit publishes");

    // Revision 1 must not carry a base version.
    let err = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        item.clone(),
        1,
        "hello".to_string(),
        user_scope(),
        now,
        Some(CabinetVersionRef::mint()),
        ReviewState::None,
    )
    .unwrap_err();
    assert_eq!(err, ContractViolation::UnexpectedBaseVersion);

    // Revision > 1 requires a base version.
    let err = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        item.clone(),
        2,
        "hello".to_string(),
        bear_scope(),
        now,
        None,
        ReviewState::None,
    )
    .unwrap_err();
    assert_eq!(err, ContractViolation::MissingBaseVersion { revision: 2 });

    // Revision 0 is out of range.
    let err = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        item.clone(),
        0,
        "hello".to_string(),
        user_scope(),
        now,
        None,
        ReviewState::None,
    )
    .unwrap_err();
    assert_eq!(err, ContractViolation::RevisionOutOfRange { revision: 0 });

    // Direct-edit follow-up revisions carry their base.
    let second = ItemVersion::direct_edit(
        CabinetVersionRef::mint(),
        item,
        2,
        "hello again".to_string(),
        bear_scope(),
        now,
        first.version_ref().clone(),
    )
    .expect("revision 2");
    assert_eq!(second.base_version(), Some(first.version_ref()));
}

#[test]
fn version_deserialization_rejects_missing_base_and_tampered_content() {
    let now = OffsetDateTime::now_utc();
    let first = ItemVersion::first(
        CabinetVersionRef::mint(),
        CabinetItemRef::mint(),
        "hello".to_string(),
        user_scope(),
        now,
    )
    .expect("revision 1");
    let mut value = serde_json::to_value(&first).expect("serialize version");

    // Round-trip is fine.
    assert!(serde_json::from_value::<ItemVersion>(value.clone()).is_ok());

    // Tampered content no longer matches the declared hash.
    let mut tampered = value.clone();
    tampered["content"] = json!("goodbye");
    assert!(serde_json::from_value::<ItemVersion>(tampered).is_err());

    // A revision-2 record without a base_version is rejected.
    value["revision"] = json!(2);
    assert!(serde_json::from_value::<ItemVersion>(value).is_err());
}

#[test]
fn phase1_rejects_pending_review_state() {
    let now = OffsetDateTime::now_utc();
    let pending = ItemVersion::with_review_state(
        CabinetVersionRef::mint(),
        CabinetItemRef::mint(),
        1,
        "draft".to_string(),
        bear_scope(),
        now,
        None,
        ReviewState::Pending,
    )
    .expect("phase 2 shape is representable");
    let err = pending.ensure_phase1_direct_edit().unwrap_err();
    assert_eq!(
        err,
        ContractViolation::ReviewStateNotAvailable { state: "pending" }
    );
}

// --- provenance: actor scope survives serialization verbatim ---

#[test]
fn actor_scope_round_trips_verbatim() {
    let mut scope = bear_scope();
    scope.run_id = Some("run_123".to_string());
    let json = serde_json::to_value(&scope).expect("serialize scope");
    let back: ActorScope = serde_json::from_value(json).expect("deserialize scope");
    assert_eq!(back, scope);
}
