use super::*;

pub(super) const ALICE: UserId = UserId::new(11);
pub(super) const BOB: UserId = UserId::new(22);
pub(super) const ACTOR: UserId = UserId::new(99);

#[derive(Clone, Copy)]
pub(super) struct Note {
    pub conversation: Uuid,
    pub memory: Uuid,
}

impl Note {
    pub fn proof(self, binding: &AuthenticatedUserEntity) -> VerifiedPersonalSource {
        VerifiedPersonalSource::for_authorized_conversation(binding, self.conversation, self.memory)
    }
}

pub(super) async fn person(store: &BearMemoryStore, user: UserId) -> AuthenticatedUserEntity {
    resolve_authenticated_user_entity(store, user)
        .await
        .unwrap()
}

pub(super) async fn opted_in(store: &BearMemoryStore, user: UserId) -> AuthenticatedUserEntity {
    let binding = person(store, user).await;
    set_capture_enabled(store, &binding, true).await.unwrap();
    binding
}

pub(super) async fn next_sequence(store: &BearMemoryStore) -> i64 {
    sqlx::query_scalar!(r#"SELECT next_sequence AS "value!: i64" FROM bear_sequence WHERE id = 1"#)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

pub(super) async fn candidate_sequence(store: &BearMemoryStore, id: PersonalCandidateId) -> i64 {
    let id = id.to_string();
    sqlx::query_scalar!(
        r#"SELECT sequence_no AS "value!: i64" FROM personal_memory_candidates WHERE candidate_id = ?"#,
        id,
    ).fetch_one(store.pool()).await.unwrap()
}

pub(super) async fn insert_note(
    store: &BearMemoryStore,
    logical: &LogicalMemoryPath,
    metadata: &str,
    visibility: &str,
) -> Uuid {
    let memory = Uuid::new_v4();
    let memory_id = memory.to_string();
    let bear_id = store.bear_id().to_string();
    let scope = logical.scope_type.as_str();
    let source_kind = logical.source.map(MemorySource::kind);
    let source_id = logical.source.map(|source| source.id().to_string());
    let hat_id = logical.hat_id.map(|hat| hat.to_string());
    let path = logical.to_logical_path();
    let mut tx = store.pool().begin().await.unwrap();
    let sequence = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
           RETURNING next_sequence - 1 AS "value!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query!(
        r"INSERT INTO memory_records (
            memory_id, bear_id, sequence_no, scope_type, scope_profile,
            scope_source_kind, scope_source_id, scope_hat_id, kind, author_profile,
            created_at, content_text, metadata_json, visibility, logical_path
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'note', 'pair',
                  '2026-01-01T00:00:00Z', 'Source text is not a personal candidate or identity grant.', ?, ?, ?)",
        memory_id, bear_id, sequence, scope, logical.scope_profile,
        source_kind, source_id, hat_id, metadata, visibility, path,
    ).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    memory
}

pub(super) async fn note_in(store: &BearMemoryStore, conversation: Uuid) -> Note {
    let memory = insert_note(
        store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(conversation), "note"),
        "{}",
        "normal",
    )
    .await;
    Note {
        conversation,
        memory,
    }
}

pub(super) async fn note(store: &BearMemoryStore) -> Note {
    note_in(store, Uuid::new_v4()).await
}

pub(super) async fn stage(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    note: Note,
    name: &str,
) -> PersonalCandidate {
    stage_preferred_name(
        store,
        &note.proof(binding),
        PreferredName::new(name).unwrap(),
    )
    .await
    .unwrap()
}

pub(super) fn assert_denied<T: std::fmt::Debug>(result: Result<T, DenError>) {
    assert!(
        matches!(result, Err(DenError::Authorization(_))),
        "{result:?}"
    );
}

pub(super) fn assert_database_error<T: std::fmt::Debug>(result: Result<T, DenError>) {
    assert!(matches!(result, Err(DenError::Database(_))), "{result:?}");
}

pub(super) async fn assert_all_denied(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    note: Note,
    id: PersonalCandidateId,
) {
    assert_denied(capture_enabled(store, binding).await);
    assert_denied(set_capture_enabled(store, binding, true).await);
    assert_denied(set_capture_enabled(store, binding, false).await);
    assert_denied(get_candidate(store, binding, id).await);
    assert_denied(list_candidates(store, binding, 100).await);
    assert_denied(withdraw_candidate(store, binding, id).await);
    assert_denied(
        stage_preferred_name(
            store,
            &note.proof(binding),
            PreferredName::new("Must not persist").unwrap(),
        )
        .await,
    );
}

pub(super) async fn current_candidate(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
) -> PersonalCandidate {
    get_candidate(store, binding, id).await.unwrap().unwrap()
}

pub(super) async fn state(store: &BearMemoryStore, id: PersonalCandidateId) -> CandidateState {
    let id = id.to_string();
    let raw = sqlx::query_scalar!(
        "SELECT state FROM personal_memory_candidates WHERE candidate_id = ?",
        id
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    match raw.as_str() {
        "pending" => CandidateState::Pending,
        "quarantined" => CandidateState::Quarantined,
        "withdrawn" => CandidateState::Withdrawn,
        _ => panic!("unexpected stored candidate state"),
    }
}

pub(super) async fn quarantine_candidate(store: &BearMemoryStore, id: PersonalCandidateId) {
    let id = id.to_string();
    assert_eq!(
        sqlx::query!(
            "UPDATE personal_memory_candidates SET state = 'quarantined' WHERE candidate_id = ?",
            id,
        )
        .execute(store.pool())
        .await
        .unwrap()
        .rows_affected(),
        1
    );
}

pub(super) async fn revoke(store: &BearMemoryStore, binding: &AuthenticatedUserEntity) {
    let review = review_revocation(store, ACTOR, binding.entity_id())
        .await
        .unwrap();
    commit_review(store, &review, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
}

pub(super) async fn reconnect(
    store: &BearMemoryStore,
    entity_id: &str,
    user: UserId,
    consent: ReplacementConsent,
) -> AuthenticatedUserEntity {
    let review = review_reconnection(store, ACTOR, entity_id, user)
        .await
        .unwrap();
    commit_review(store, &review, consent).await.unwrap();
    let binding = person(store, user).await;
    assert_eq!(binding.entity_id(), entity_id);
    binding
}
