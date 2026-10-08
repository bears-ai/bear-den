use super::*;
use crate::admin::usability_tests::assert_visible;

#[path = "action_tests.rs"]
mod action_tests;

#[tokio::test]
async fn postgres_resolution_preserves_canonical_history_and_next_turn_replay() {
    use den_service::conversation::{
        events::{
            canonical_persistence_context, persist_canonical_conversation_record,
            CanonicalConversationRecord,
        },
        persistence,
    };
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("pg-proposal-feedback-{}", Uuid::new_v4());
    let bear_id = bears_db::create_bear(
        &pool,
        bears_db::BearParams {
            slug: &slug,
            name: "Postgres proposal feedback",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let member_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let external = format!("conv-{}", Uuid::new_v4());
    let conversation = persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin_id),
        &external,
        None,
        None,
    )
    .await
    .unwrap();
    let context = canonical_persistence_context(
        pool.clone(),
        bear_id,
        Some(admin_id),
        external.clone(),
        None,
        None,
        "proposal-route-test".into(),
        false,
    );
    for record in [
        CanonicalConversationRecord::visible_user_message("Previous user message", json!({}), None),
        CanonicalConversationRecord::visible_assistant_message(
            "Previous assistant message",
            json!({}),
            None,
        ),
        CanonicalConversationRecord::workflow_event(
            "DIAGNOSTIC MUST NOT REPLAY",
            json!({"evidence": "PRIVATE RAW EVIDENCE"}),
            None,
        ),
    ] {
        persist_canonical_conversation_record(&context, &record)
            .await
            .unwrap();
    }
    let original = persistence::list_messages_page(&pool, conversation.id, None, 100)
        .await
        .unwrap();
    let snapshot = |rows: &[persistence::PersistedConversationMessage]| {
        rows.iter()
            .map(|row| {
                (
                    row.sequence_no,
                    row.message_type.clone(),
                    row.role.clone(),
                    row.visibility.clone(),
                    row.content_text.clone(),
                    row.content_json.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let original_snapshot = snapshot(&original);
    let proposal = memory_proposals::create(
        &pool,
        CreateMemoryProposal {
            bear_id,
            source_profile: RuntimeContextLabel::ArmatureConversation,
            source_agent_id: None,
            source_paths: Vec::new(),
            source_refs: json!({"conversation_id": external}),
            suggested_action: "unspecified",
            target_ref: None,
            title: "Reviewable preference",
            summary: "PRIVATE PROPOSAL SUMMARY",
            rationale: "PRIVATE RATIONALE",
            proposed_content: Some("PRIVATE PROPOSED CONTENT"),
            proposed_patch: None,
            refs: json!({}),
            sensitivity: "secret_risk",
            requires_human: true,
            project_to_conversation: false,
        },
    )
    .await
    .unwrap();
    let (app, _) = test_app(pool.clone(), &sqlite_dir).await;
    let admin = login_cookie(&app, admin_id).await;
    let member = login_cookie(&app, member_id).await;
    let uri = format!("/bear/{slug}/memory/proposals/{}", proposal.id);
    let post = |cookie: &str, resolution: &str| {
        Request::builder().method("POST").uri(&uri)
        .header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("status={resolution}&decision_summary=PRIVATE+DECISION+DRAFT&review_notes=PRIVATE+REVIEW+NOTES"))).unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(post(&member, "rejected"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let invalid = app.clone().oneshot(post(&admin, "invalid")).await.unwrap();
    assert_eq!(invalid.status(), StatusCode::OK);
    assert_eq!(
        snapshot(
            &persistence::list_messages_page(&pool, conversation.id, None, 100)
                .await
                .unwrap()
        ),
        original_snapshot
    );
    let response = app.clone().oneshot(post(&admin, "rejected")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let stored = memory_proposals::get_for_bear(&pool, bear_id, proposal.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "rejected");
    assert_eq!(stored.source_refs, json!({"conversation_id": external}));
    assert_eq!(stored.source_profile, proposal.source_profile);
    assert_eq!(stored.sensitivity, "secret_risk");
    assert!(stored.requires_human);
    assert_eq!(
        stored.proposed_content.as_deref(),
        Some("PRIVATE PROPOSED CONTENT")
    );
    assert_eq!(stored.summary, "PRIVATE PROPOSAL SUMMARY");
    assert_eq!(stored.rationale, "PRIVATE RATIONALE");
    assert_eq!(
        stored.decision_summary.as_deref(),
        Some("PRIVATE DECISION DRAFT")
    );
    assert_eq!(stored.review_notes.as_deref(), Some("PRIVATE REVIEW NOTES"));
    let rows = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let rows = persistence::list_messages_page(&pool, conversation.id, None, 100)
                .await
                .unwrap();
            if rows.len() >= original.len() + 2 {
                break rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("canonical resolution projection persisted");
    assert_eq!(rows.len(), original.len() + 2);
    let retained: Vec<_> = rows
        .iter()
        .filter(|row| row.sequence_no <= original.iter().map(|row| row.sequence_no).max().unwrap())
        .cloned()
        .collect();
    assert_eq!(
        snapshot(&retained),
        original_snapshot,
        "existing canonical rows must remain untouched"
    );
    let visible = persistence::list_projected_messages_page(
        &pool,
        conversation.id,
        None,
        100,
        persistence::ConversationHistoryProjection::UserHistory,
    )
    .await
    .unwrap();
    assert_eq!(visible.len(), 3);
    let replay = den_runtime::agent_loop::assemble_agent_messages(
        &pool,
        bear_id,
        &external,
        Some("System context"),
        Some("Next user message"),
        &[],
    )
    .await
    .unwrap();
    for text in [
        "Previous user message",
        "Previous assistant message",
        "Next user message",
        "Memory proposal 'Reviewable preference' was rejected.",
    ] {
        assert_eq!(
            replay
                .iter()
                .filter(|message| message.content.as_deref() == Some(text))
                .count(),
            1,
            "{text}"
        );
    }
    let serialized = serde_json::to_string(&replay).unwrap();
    for private in [
        "DIAGNOSTIC MUST NOT REPLAY",
        "PRIVATE RAW EVIDENCE",
        "PRIVATE PROPOSED CONTENT",
        "PRIVATE DECISION DRAFT",
        "PRIVATE REVIEW NOTES",
        "PRIVATE PROPOSAL SUMMARY",
        "PRIVATE RATIONALE",
    ] {
        assert!(
            !serialized.contains(private),
            "raw inspection content entered model replay: {private}"
        );
        assert!(!visible.iter().any(|row| row.content_text.contains(private)));
    }
    assert_eq!(
        snapshot(
            &persistence::list_messages_page(&pool, conversation.id, None, 100)
                .await
                .unwrap()
        ),
        snapshot(&rows)
    );
}

#[tokio::test]
async fn proposal_resolution_keeps_invalid_drafts_then_reports_a_persisted_save() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("proposal-feedback-{}", Uuid::new_v4());
    let bear_id = bears_db::create_bear(
        &pool,
        bears_db::BearParams {
            slug: &slug,
            name: "Proposal feedback",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let member_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let (app, manager) = test_app(pool.clone(), &sqlite_dir).await;
    let store = manager.store_for_bear(bear_id).await.unwrap();
    let proposal = den_memory::create_memory_proposal(
        &store,
        "unspecified",
        "private",
        true,
        &json!({ "title": "Feedback proposal", "proposed_content": "Private review content" }),
    )
    .await
    .expect("create proposal");
    let admin = login_cookie(&app, admin_id).await;
    let member = login_cookie(&app, member_id).await;
    let uri = format!("/bear/{slug}/memory/proposals/{}", proposal.proposal_id);
    let post = |cookie: &str, status: &str| {
        Request::builder()
            .method("POST")
            .uri(&uri)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "status={status}&decision_summary=Draft+summary&review_notes=Draft+notes"
            )))
            .unwrap()
    };
    let unauthorized = app.clone().oneshot(post(&member, "invalid")).await.unwrap();
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);
    let response = app.clone().oneshot(post(&admin, "invalid")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_visible(&html, "Resolution was not saved");
    assert_visible(&html, "Draft summary");
    assert_visible(&html, "Draft notes");
    assert!(html.contains("value=\"invalid\" selected"));
    let response = app.clone().oneshot(post(&admin, "deferred")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let stored = den_memory::get_memory_proposal(&store, &proposal.proposal_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "deferred");
    let (status, html) = get_page(&app, &admin, &location).await;
    assert_eq!(status, StatusCode::OK);
    assert_visible(&html, "Proposal resolution saved.");
}
