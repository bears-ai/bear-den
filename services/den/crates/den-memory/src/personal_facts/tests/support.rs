use super::*;

pub(super) const ALICE: UserId = UserId::new(11);
pub(super) const BOB: UserId = UserId::new(22);

#[derive(Clone, Copy)]
pub(super) struct Note {
    pub conversation: Uuid,
    pub memory: Uuid,
}

impl Note {
    pub fn proof(self, owner: &AuthenticatedUserEntity) -> VerifiedPersonalSource {
        VerifiedPersonalSource::for_authorized_conversation(owner, self.conversation, self.memory)
    }

    pub fn path(self) -> LogicalMemoryPath {
        LogicalMemoryPath::source_local(MemorySource::Conversation(self.conversation), "note")
    }
}

pub(super) async fn person(store: &BearMemoryStore, user: UserId) -> AuthenticatedUserEntity {
    resolve_authenticated_user_entity(store, user)
        .await
        .unwrap()
}

pub(super) async fn opted_in(store: &BearMemoryStore, user: UserId) -> AuthenticatedUserEntity {
    let owner = person(store, user).await;
    set_capture_enabled(store, &owner, true).await.unwrap();
    owner
}

pub(super) async fn note(store: &BearMemoryStore) -> Note {
    let conversation = Uuid::new_v4();
    let row = store
        .append_record(
            &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
            "note",
            "pair",
            None,
            "Canonical source, not an identity grant or a copy of the preferred name.",
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    Note {
        conversation,
        memory: Uuid::parse_str(&row.memory_id).unwrap(),
    }
}

pub(super) async fn stage(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    source: Note,
    value: &str,
) -> PersonalCandidate {
    stage_preferred_name(
        store,
        &source.proof(owner),
        PreferredName::new(value).unwrap(),
    )
    .await
    .unwrap()
}

pub(super) async fn publish(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    source: Note,
    value: &str,
    expected: Option<PersonalFactId>,
) -> PublicationReceipt {
    let candidate = stage(store, owner, source, value).await;
    publish_preferred_name(store, &source.proof(owner), candidate.id(), expected)
        .await
        .unwrap()
}

pub(super) async fn assert_head(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    receipt: PublicationReceipt,
    state: PersonalFactState,
    value: Option<&str>,
) {
    let head = current_preferred_name(store, owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, receipt.fact_id);
    assert_eq!(head.candidate_id, receipt.candidate_id);
    assert_eq!(head.state, state);
    assert_eq!(
        head.preferred_name.as_ref().map(PreferredName::as_str),
        value
    );
}

pub(super) fn denied<T: std::fmt::Debug>(result: Result<T, DenError>) {
    assert!(
        matches!(result, Err(DenError::Authorization(_))),
        "{result:?}"
    );
}

pub(super) fn database_error<T: std::fmt::Debug>(result: Result<T, DenError>) {
    assert!(matches!(result, Err(DenError::Database(_))), "{result:?}");
}

pub(super) async fn all_state(store: &BearMemoryStore) -> (Snapshot, GenericSnapshot) {
    (snapshot(store).await, generic_snapshot(store).await)
}

pub(super) fn table(snapshot_json: &str, name: &str) -> Vec<Value> {
    let value: Value = serde_json::from_str(snapshot_json).unwrap();
    value[name].as_array().unwrap().clone()
}

pub(super) fn row(snapshot_json: &str, name: &str, id: impl ToString) -> Value {
    let id = id.to_string();
    table(snapshot_json, name)
        .into_iter()
        .find(|row| row[0] == id)
        .unwrap()
}

pub(super) async fn candidate_state(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
) -> CandidateState {
    get_candidate(store, owner, id)
        .await
        .unwrap()
        .unwrap()
        .state()
}

pub(super) async fn reconnect(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
) -> AuthenticatedUserEntity {
    use crate::identity_reconnection::{commit_review, review_reconnection, ReplacementConsent};
    let review = review_reconnection(store, UserId::new(99), owner.entity_id(), owner.user_id())
        .await
        .unwrap();
    commit_review(store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    person(store, owner.user_id()).await
}
