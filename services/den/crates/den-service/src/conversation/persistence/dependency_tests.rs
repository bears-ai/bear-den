use super::*;
use crate::bears::db::{create_bear, BearParams};
use crate::conversation::{
    dependencies::ContextDependencies,
    events::{
        CanonicalConversationRecord, CanonicalToolRequestRecord, CanonicalToolResultRecord,
        ConversationEventProvenance,
    },
};
use den_core::ids::BearId;
use den_memory::personal_facts::PersonalFactId;
use serde_json::{json, Value};
use time::OffsetDateTime;

fn dependencies() -> ContextDependencies {
    ContextDependencies::personal_fact(
        BearId::new(Uuid::from_u128(1)),
        PersonalFactId(Uuid::from_u128(2)),
    )
}

fn persisted(write: ConversationMessageWrite) -> PersistedConversationMessage {
    PersistedConversationMessage {
        sequence_no: 7,
        message_type: write.message_type.as_str().to_string(),
        role: write.role.map(|role| role.as_str().to_string()),
        visibility: write.visibility.as_str().to_string(),
        content_text: write.content_text,
        content_json: write.content_json,
        provider_message_id: write.provider_message_id,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn assistant(visibility: ConversationMessageVisibility) -> CanonicalConversationRecord {
    let provenance = ConversationEventProvenance::client_session("dependency-scope");
    match visibility {
        ConversationMessageVisibility::Default => CanonicalConversationRecord::assistant_output(
            "retained evidence",
            &provenance,
            Some("provider".into()),
            Some("request".into()),
        ),
        ConversationMessageVisibility::HiddenFromUser => {
            CanonicalConversationRecord::model_visible_hidden_assistant_message(
                "retained evidence",
                provenance.as_content_json("assistant_output"),
                Some("provider".into()),
            )
        }
        _ => CanonicalConversationRecord::structured_event(
            ConversationMessageType::Assistant,
            Some(ConversationMessageRole::Assistant),
            visibility,
            "retained evidence",
            provenance.as_content_json("assistant_output"),
            Some("provider".into()),
        ),
    }
}

fn tool_records() -> [CanonicalConversationRecord; 2] {
    let provenance = ConversationEventProvenance::client_session("dependency-scope");
    [
        CanonicalConversationRecord::model_tool_request(
            CanonicalToolRequestRecord::new(
                "memory_read",
                "call",
                "request",
                Some("approval".into()),
                json!({"path": "note"}),
                true,
                Some("policy".into()),
                "native_runtime",
            ),
            &provenance,
        ),
        CanonicalConversationRecord::model_tool_result(
            CanonicalToolResultRecord::new(
                Some("memory_read".into()),
                "call",
                Some("approval".into()),
                ToolResultStatus::Ok,
                Some("retained tool evidence".into()),
                json!({"content": "retained structured evidence"}),
                json!({"component": "dependency_test"}),
                Some("request".into()),
            ),
            &provenance,
        ),
    ]
}

fn assert_attachment_preserves_record(record: CanonicalConversationRecord) {
    let source_event_id = Some("stable-source-event".into());
    let original = record.to_write(source_event_id.clone());
    let attached = record
        .with_context_dependencies(&dependencies())
        .unwrap()
        .to_write(source_event_id);
    assert_eq!(attached.message_type, original.message_type);
    assert_eq!(attached.role, original.role);
    assert_eq!(attached.visibility, original.visibility);
    assert_eq!(attached.content_text, original.content_text);
    assert_eq!(attached.provider_message_id, original.provider_message_id);
    assert_eq!(attached.source_event_id, original.source_event_id);
    assert_eq!(attached.created_at, original.created_at);
    let mut metadata = attached.content_json.clone();
    assert!(metadata
        .as_object_mut()
        .unwrap()
        .remove("den_context_dependencies")
        .is_some());
    assert_eq!(metadata, original.content_json);

    let original = persisted(original);
    let attached = persisted(attached);
    assert!(original.require_unrestricted_model_reuse().is_ok());
    assert!(attached.require_unrestricted_model_reuse().is_err());
    assert!(attached.to_model_transcript_message().is_none());
    assert!(attached.to_model_transcript_record().is_none());
    assert!(attached.to_model_history_record().is_none());
    assert!(!attached.is_model_transcript_visible());
    assert!(!attached.is_transcript_visible());
    assert_eq!(
        attached.to_user_history_transcript_message(),
        original.to_user_history_transcript_message()
    );
    assert_eq!(
        attached.to_user_history_record(),
        original.to_user_history_record()
    );
    assert_eq!(
        attached.is_user_history_visible(),
        original.is_user_history_visible()
    );
}

#[test]
fn canonical_assistant_attachment_blocks_model_reuse_without_changing_human_history() {
    for visibility in ConversationMessageVisibility::ALL {
        assert_attachment_preserves_record(assistant(visibility));
    }
    let visible = persisted(
        assistant(ConversationMessageVisibility::Default)
            .with_context_dependencies(&dependencies())
            .unwrap()
            .to_write(None),
    );
    assert_eq!(
        visible
            .to_user_history_transcript_message()
            .unwrap()
            .content,
        "retained evidence"
    );
    assert!(visible.to_user_history_record().is_some());
}

#[test]
fn canonical_tool_attachment_preserves_payloads_visibility_and_human_tool_history() {
    for record in tool_records() {
        for visibility in ConversationMessageVisibility::ALL {
            let mut record = record.clone();
            let CanonicalConversationRecord::StructuredEvent {
                visibility: stored, ..
            } = &mut record
            else {
                panic!("canonical model tool record must be structured");
            };
            *stored = visibility;
            assert_attachment_preserves_record(record);
        }
    }
}

#[test]
fn canonical_attachment_cannot_clear_existing_protection() {
    let protected = assistant(ConversationMessageVisibility::Default)
        .with_context_dependencies(&dependencies())
        .unwrap();
    let original_metadata = protected.to_write(None).content_json;
    let attached = protected
        .with_context_dependencies(&ContextDependencies::default())
        .unwrap()
        .to_write(None);
    assert_eq!(attached.content_json, original_metadata);
    assert!(persisted(attached)
        .require_unrestricted_model_reuse()
        .is_err());
}

#[test]
fn canonical_attachment_promotes_null_metadata_but_rejects_non_maps_and_invalid_envelopes() {
    let null_record =
        CanonicalConversationRecord::visible_assistant_message("evidence", Value::Null, None);
    let attached = null_record
        .with_context_dependencies(&dependencies())
        .unwrap()
        .to_write(None);
    assert_eq!(
        ContextDependencies::from_metadata(&attached.content_json).unwrap(),
        dependencies()
    );
    for metadata in [
        json!([]),
        json!("opaque"),
        json!({"den_context_dependencies": null}),
        json!({"den_context_dependencies": {"version": 2, "personal_facts": []}}),
    ] {
        let record =
            CanonicalConversationRecord::visible_assistant_message("evidence", metadata, None);
        assert!(record
            .with_context_dependencies(&ContextDependencies::default())
            .is_err());
    }
}

#[test]
fn invalid_metadata_blocks_every_model_projection_without_affecting_human_evidence() {
    for mut row in [
        persisted(assistant(ConversationMessageVisibility::Default).to_write(None)),
        persisted(tool_records()[0].to_write(None)),
        persisted(tool_records()[1].to_write(None)),
    ] {
        row.visibility = ConversationMessageVisibility::Default.as_str().into();
        let human_message = row.to_user_history_transcript_message();
        let human_record = row.to_user_history_record();
        for envelope in [
            Value::Null,
            json!([1, []]),
            json!({"version": 99, "personal_facts": []}),
        ] {
            row.content_json["den_context_dependencies"] = envelope;
            assert!(row.require_unrestricted_model_reuse().is_err());
            assert!(row.to_model_transcript_message().is_none());
            assert!(row.to_model_transcript_record().is_none());
            assert!(row.to_model_history_record().is_none());
            assert!(!row.is_model_transcript_visible());
            assert_eq!(row.to_user_history_transcript_message(), human_message);
            assert_eq!(row.to_user_history_record(), human_record);
        }
    }
}

#[test]
fn aggregate_model_guard_checks_entire_raw_page_before_filtering_or_truncation() {
    for visibility in ConversationMessageVisibility::ALL {
        for invalid in [false, true] {
            let clean = persisted(assistant(ConversationMessageVisibility::Default).to_write(None));
            let mut rejected = persisted(
                assistant(visibility)
                    .with_context_dependencies(&dependencies())
                    .unwrap()
                    .to_write(None),
            );
            if invalid {
                rejected.content_json["den_context_dependencies"] = Value::Null;
            }
            assert!(project_message_page(
                vec![clean.clone(), rejected.clone()],
                ConversationHistoryProjection::ModelTranscript,
                1,
            )
            .is_err());
            assert!(project_message_page(
                vec![rejected.clone(), clean.clone()],
                ConversationHistoryProjection::ModelTranscript,
                1,
            )
            .is_err());
            assert!(project_message_page(
                vec![rejected],
                ConversationHistoryProjection::ModelTranscript,
                0,
            )
            .is_err());
        }
    }
}

#[test]
fn aggregate_user_history_does_not_apply_model_dependency_policy() {
    for visibility in ConversationMessageVisibility::ALL {
        let row = persisted(
            assistant(visibility)
                .with_context_dependencies(&dependencies())
                .unwrap()
                .to_write(None),
        );
        let projected =
            project_message_page(vec![row], ConversationHistoryProjection::UserHistory, 1).unwrap();
        assert_eq!(
            projected.len(),
            usize::from(visibility.is_user_history_visible())
        );
    }
}

#[test]
fn ordinary_model_projection_and_legacy_null_metadata_keep_their_existing_semantics() {
    let mut visible = persisted(assistant(ConversationMessageVisibility::Default).to_write(None));
    visible.content_json = Value::Null;
    assert!(visible.to_model_transcript_message().is_some());
    assert!(visible.to_model_transcript_record().is_some());
    assert!(visible.to_model_history_record().is_some());
    let hidden = persisted(assistant(ConversationMessageVisibility::HiddenFromUser).to_write(None));
    let diagnostic =
        persisted(assistant(ConversationMessageVisibility::DiagnosticOnly).to_write(None));
    let projected = project_message_page(
        vec![diagnostic, hidden, visible],
        ConversationHistoryProjection::ModelTranscript,
        1,
    )
    .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].visibility,
        ConversationMessageVisibility::HiddenFromUser.as_str()
    );
    for record in tool_records() {
        let row = persisted(record.to_write(None));
        assert!(row.to_model_transcript_record().is_some());
        assert!(row.to_model_history_record().is_some());
        assert!(row.to_user_history_record().is_none());
    }
}

async fn projection_conversation(pool: &PgPool) -> (BearId, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let slug = format!("dependency-page-{suffix}");
    let bear_id = create_bear(
        pool,
        BearParams {
            slug: &slug,
            name: "Dependency pagination test",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    // Ownerless deliberately: this exercises storage projection, not inference admission.
    let conversation = ensure_conversation_for_external_id(
        pool,
        bear_id,
        None,
        &format!("den-conv-dependency-page-{suffix}"),
        None,
        None,
    )
    .await
    .unwrap();
    (BearId::new(bear_id), conversation.id)
}

fn dependency_page_write(
    bear_id: BearId,
    visibility: ConversationMessageVisibility,
    malformed: bool,
) -> ConversationMessageWrite {
    let dependencies = ContextDependencies::personal_fact(bear_id, PersonalFactId(Uuid::new_v4()));
    let mut write = assistant(visibility)
        .with_context_dependencies(&dependencies)
        .unwrap()
        .to_write(None);
    if malformed {
        write.content_json["den_context_dependencies"] = Value::Null;
    }
    write
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_page_rejects_later_raw_page_dependencies_after_accumulating_clean_rows(
    pool: PgPool,
) {
    for malformed in [false, true] {
        let (bear_id, conversation_id) = projection_conversation(&pool).await;
        let older_write =
            ConversationMessageWrite::assistant_turn("older ordinary evidence", json!({}));
        let older = append_message(&pool, conversation_id, &older_write)
            .await
            .unwrap();
        let rejected_write =
            dependency_page_write(bear_id, ConversationMessageVisibility::Default, malformed);
        let rejected = append_message(&pool, conversation_id, &rejected_write)
            .await
            .unwrap();
        let hidden_write = assistant(ConversationMessageVisibility::HiddenFromUser).to_write(None);
        let hidden = append_message(&pool, conversation_id, &hidden_write)
            .await
            .unwrap();
        for _ in 0..99 {
            append_message(
                &pool,
                conversation_id,
                &ConversationMessageWrite::workflow_diagnostic("diagnostic evidence", json!({})),
            )
            .await
            .unwrap();
        }
        let clean_write =
            ConversationMessageWrite::assistant_turn("newest ordinary evidence", json!({}));
        let clean = append_message(&pool, conversation_id, &clean_write)
            .await
            .unwrap();

        let first_raw = list_messages_page(&pool, conversation_id, None, 100)
            .await
            .unwrap();
        assert_eq!(first_raw.len(), 100);
        assert_eq!(first_raw[0].sequence_no, clean.sequence_no);
        assert_eq!(
            first_raw
                .iter()
                .filter(|row| row.is_model_transcript_visible())
                .count(),
            1
        );
        assert!(first_raw
            .iter()
            .all(|row| row.require_unrestricted_model_reuse().is_ok()));
        assert!(first_raw[1..]
            .iter()
            .all(|row| row.storage_visibility().unwrap()
                == ConversationMessageVisibility::DiagnosticOnly));
        let second_raw = list_messages_page(
            &pool,
            conversation_id,
            Some(first_raw.last().unwrap().sequence_no),
            100,
        )
        .await
        .unwrap();
        assert_eq!(
            second_raw
                .iter()
                .map(|row| row.sequence_no)
                .collect::<Vec<_>>(),
            vec![hidden.sequence_no, rejected.sequence_no, older.sequence_no]
        );
        assert_eq!(
            second_raw[0].storage_visibility().unwrap(),
            ConversationMessageVisibility::HiddenFromUser
        );

        let first_model = list_projected_messages_page(
            &pool,
            conversation_id,
            None,
            1,
            ConversationHistoryProjection::ModelTranscript,
        )
        .await
        .unwrap();
        assert_eq!(first_model.len(), 1);
        assert_eq!(first_model[0].sequence_no, clean.sequence_no);
        assert!(matches!(
            list_projected_messages_page(
                &pool,
                conversation_id,
                None,
                2,
                ConversationHistoryProjection::ModelTranscript,
            )
            .await,
            Err(DenError::Authorization(_))
        ));

        let human = list_projected_messages_page(
            &pool,
            conversation_id,
            None,
            2,
            ConversationHistoryProjection::UserHistory,
        )
        .await
        .unwrap();
        assert_eq!(
            human.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
            vec![clean.sequence_no, rejected.sequence_no]
        );
        assert_eq!(human[0].content_text, clean_write.content_text);
        assert_eq!(human[1].content_text, rejected_write.content_text);
        assert_eq!(human[1].content_json, rejected_write.content_json);
        let evidence = human[1].to_user_history_transcript_message().unwrap();
        assert_eq!(evidence.sequence_no, rejected.sequence_no);
        assert_eq!(evidence.content, rejected_write.content_text);
        assert!(
            human
                .iter()
                .all(|row| row.storage_visibility().unwrap()
                    == ConversationMessageVisibility::Default)
        );
        let next_human = list_projected_messages_page(
            &pool,
            conversation_id,
            Some(human.last().unwrap().sequence_no),
            2,
            ConversationHistoryProjection::UserHistory,
        )
        .await
        .unwrap();
        assert_eq!(next_human.len(), 1);
        assert_eq!(next_human[0].sequence_no, older.sequence_no);
        assert_eq!(next_human[0].content_text, older_write.content_text);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_page_rejects_dependencies_beyond_requested_limit_in_same_raw_page(pool: PgPool) {
    for malformed in [false, true] {
        let (bear_id, conversation_id) = projection_conversation(&pool).await;
        for _ in 0..100 {
            append_message(
                &pool,
                conversation_id,
                &ConversationMessageWrite::workflow_diagnostic(
                    "older diagnostic evidence",
                    json!({}),
                ),
            )
            .await
            .unwrap();
        }
        let rejected_write = dependency_page_write(
            bear_id,
            ConversationMessageVisibility::DiagnosticOnly,
            malformed,
        );
        let rejected = append_message(&pool, conversation_id, &rejected_write)
            .await
            .unwrap();
        let hidden_write = assistant(ConversationMessageVisibility::HiddenFromUser).to_write(None);
        let hidden = append_message(&pool, conversation_id, &hidden_write)
            .await
            .unwrap();
        let clean_write =
            ConversationMessageWrite::assistant_turn("newest ordinary evidence", json!({}));
        let clean = append_message(&pool, conversation_id, &clean_write)
            .await
            .unwrap();

        let raw = list_messages_page(&pool, conversation_id, None, 100)
            .await
            .unwrap();
        assert_eq!(raw.len(), 100);
        assert_eq!(raw[0].sequence_no, clean.sequence_no);
        assert!(raw[0].to_model_transcript_message().is_some());
        assert_eq!(raw[1].sequence_no, hidden.sequence_no);
        assert_eq!(
            raw[1].storage_visibility().unwrap(),
            ConversationMessageVisibility::HiddenFromUser
        );
        assert_eq!(raw[2].sequence_no, rejected.sequence_no);
        assert_eq!(
            raw[2].storage_visibility().unwrap(),
            ConversationMessageVisibility::DiagnosticOnly
        );
        assert_eq!(raw[2].content_text, rejected_write.content_text);
        assert_eq!(raw[2].content_json, rejected_write.content_json);
        assert!(raw[2].require_unrestricted_model_reuse().is_err());
        let remainder = list_messages_page(
            &pool,
            conversation_id,
            Some(raw.last().unwrap().sequence_no),
            100,
        )
        .await
        .unwrap();
        assert_eq!(remainder.len(), 3);
        assert!(matches!(
            list_projected_messages_page(
                &pool,
                conversation_id,
                None,
                1,
                ConversationHistoryProjection::ModelTranscript,
            )
            .await,
            Err(DenError::Authorization(_))
        ));

        let human = list_projected_messages_page(
            &pool,
            conversation_id,
            None,
            1,
            ConversationHistoryProjection::UserHistory,
        )
        .await
        .unwrap();
        assert_eq!(human.len(), 1);
        assert_eq!(human[0].sequence_no, clean.sequence_no);
        assert_eq!(human[0].content_text, clean_write.content_text);
        assert_eq!(
            human[0].storage_visibility().unwrap(),
            ConversationMessageVisibility::Default
        );
        let next_human = list_projected_messages_page(
            &pool,
            conversation_id,
            Some(clean.sequence_no),
            2,
            ConversationHistoryProjection::UserHistory,
        )
        .await
        .unwrap();
        assert!(
            next_human.is_empty(),
            "hidden and diagnostic evidence must remain hidden from human history"
        );
    }
}
