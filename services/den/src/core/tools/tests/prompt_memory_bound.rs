use den_core::{
    ids::{BearId, UserId},
    tools::{
        context::DenToolInvocationContext,
        prompt_memory::{
            prompt_memory_list, prompt_memory_patch, prompt_memory_upsert, PromptMemoryBlockScope,
            PromptMemoryBlockState, PromptMemoryBlockType, PromptMemoryBlockWrite,
        },
    },
    BearProfile, DenError,
};
use den_service::{
    bears::{
        db::{create_bear, BearParams},
        hats::{bindings::bind_conversation_hat, create_hat},
    },
    conversation::persistence::ensure_conversation_for_external_id,
    prompt_memory_block_store::upsert_prompt_memory_block,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::tools::prompt_memory::DenPromptMemoryStore;

fn prompt_block(
    bear_id: Uuid,
    user_id: i32,
    id: &str,
    scope: PromptMemoryBlockScope,
    session: Option<&str>,
) -> PromptMemoryBlockWrite {
    PromptMemoryBlockWrite {
        block_id: id.into(),
        bear_id: Some(bear_id),
        profile_slug: Some("pair".into()),
        scope,
        block_type: PromptMemoryBlockType::UserInstruction,
        state: PromptMemoryBlockState::Active,
        work_surface: None,
        session_id: session.map(str::to_string),
        title: id.into(),
        body: id.into(),
        priority: 1,
        created_by_user_id: Some(user_id),
        supersedes_block_id: None,
        metadata: json!({}),
    }
}

#[sqlx::test]
async fn bound_prompt_tools_only_read_and_mutate_own_session_blocks(pool: PgPool) {
    let nonce = Uuid::new_v4().simple().to_string();
    let bear = create_bear(
        &pool,
        BearParams {
            slug: &format!("bound-prompts-{}", &nonce[..12]),
            name: "Bound prompt test",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create Bear");
    let other_bear = create_bear(
        &pool,
        BearParams {
            slug: &format!("bound-prompts-other-{}", &nonce[..12]),
            name: "Other Bear",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .expect("create other Bear");
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, 'Test', 'x') RETURNING id",
        format!("memory-{nonce}@example.invalid"), format!("memory{}", &nonce[..12]),
    ).fetch_one(&pool).await.expect("create user");
    let conversation =
        ensure_conversation_for_external_id(&pool, bear, Some(user), "bound-prompts-a", None, None)
            .await
            .expect("create conversation");
    let hat = create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Review",
        "Prompt boundary",
    )
    .await
    .expect("create hat");
    bind_conversation_hat(&pool, BearId::new(bear), conversation.id, hat.id)
        .await
        .expect("bind hat");
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": bear, "bear_slug": "bound-prompts", "binding_id": "test-binding",
        "profile": "pair", "user_id": user, "username": null, "membership_role": null,
        "conversation_id": "bound-prompts-a", "session_id": "sess-a",
        "request_id": null, "channel": {}
    }))
    .expect("tool context");
    let store = DenPromptMemoryStore::new(&pool);
    for (id, scope, session) in [
        ("shared", PromptMemoryBlockScope::BearWide, None),
        ("legacy-role", PromptMemoryBlockScope::RoleLocal, None),
        ("session-b", PromptMemoryBlockScope::Session, Some("sess-b")),
    ] {
        upsert_prompt_memory_block(&pool, &prompt_block(bear, user, id, scope, session))
            .await
            .expect("seed block");
    }
    upsert_prompt_memory_block(
        &pool,
        &prompt_block(
            other_bear,
            user,
            "other-bear",
            PromptMemoryBlockScope::Session,
            Some("sess-a"),
        ),
    )
    .await
    .expect("seed foreign block");

    let own = prompt_memory_upsert(
        &store,
        &context,
        BearProfile::Pair,
        json!({
            "block_id": "session-a", "scope": "session", "block_type": "user_instruction",
            "session_id": "sess-a", "title": "Own session", "body": "Own note"
        }),
    )
    .await
    .expect("write own block");
    assert_eq!(own["status"], "ok");
    let listed = prompt_memory_list(
        &store,
        &context,
        BearProfile::Pair,
        json!({"include_archived": true}),
    )
    .await
    .expect("list bound blocks");
    let ids = listed["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"shared"));
    assert!(ids.contains(&"session-a"));
    assert!(!ids.contains(&"legacy-role"));
    assert!(!ids.contains(&"session-b"));
    assert!(!ids.contains(&"other-bear"));

    for args in [
        json!({"block_id":"cannot-promote","scope":"profile_local","block_type":"user_instruction","title":"No","body":"No"}),
        json!({"block_id":"wrong-session","scope":"session","block_type":"user_instruction","session_id":"sess-b","title":"No","body":"No"}),
        json!({"block_id":"shared","scope":"session","block_type":"user_instruction","session_id":"sess-a","title":"No","body":"No"}),
        json!({"block_id":"other-bear","scope":"session","block_type":"user_instruction","session_id":"sess-a","title":"No","body":"No"}),
        json!({"block_id":"supersession","scope":"session","block_type":"user_instruction","session_id":"sess-a","supersedes_block_id":"legacy-role","title":"No","body":"No"}),
    ] {
        let result = prompt_memory_upsert(&store, &context, BearProfile::Pair, args.clone()).await;
        assert!(
            matches!(result, Err(DenError::Authorization(_))),
            "{args}: {result:?}"
        );
    }
    let foreign =
        den_service::prompt_memory_block_store::list_prompt_memory_blocks_for_bear_profile(
            &pool, other_bear, "pair",
        )
        .await
        .unwrap();
    assert_eq!(foreign[0].body, "other-bear");
    assert!(matches!(
        prompt_memory_patch(
            &store,
            &context,
            BearProfile::Pair,
            json!({
                "block_id": "legacy-role", "title": "No", "body": "No"
            })
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    prompt_memory_patch(
        &store,
        &context,
        BearProfile::Pair,
        json!({
            "block_id": "session-a", "title": "Updated", "body": "Updated own session"
        }),
    )
    .await
    .expect("patch own block");
    let listed = prompt_memory_list(
        &store,
        &context,
        BearProfile::Pair,
        json!({"scope":"session"}),
    )
    .await
    .expect("list own session");
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["blocks"][0]["body"], "Updated own session");
    let mut missing_conversation = context.clone();
    missing_conversation.conversation_id = "unknown-conversation".into();
    assert!(matches!(
        prompt_memory_list(&store, &missing_conversation, BearProfile::Pair, json!({})).await,
        Err(DenError::NotFound(_))
    ));
}
