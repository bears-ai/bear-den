use super::*;
use crate::{conversation::persistence, model_selection::apply_conversation_model_selection};

async fn conversation(pool: &PgPool, owner: BearId, external_id: &str) -> Uuid {
    persistence::ensure_conversation_for_external_id(
        pool,
        owner.as_uuid(),
        None,
        external_id,
        None,
        None,
    )
    .await
    .unwrap()
    .id
}

async fn state_snapshot(pool: &PgPool, conversation_id: Uuid) -> serde_json::Value {
    // Include timestamps and diagnostic fields as well as the selected model:
    // rejection must not mutate any part of the existing canonical state row.
    sqlx::query_scalar!(
        r#"SELECT to_jsonb(state) AS "snapshot!: serde_json::Value"
           FROM conversation_model_state state WHERE conversation_id = $1"#,
        conversation_id,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn empty_catalog_cannot_authorize_a_static_registry_pin(pool: PgPool) {
    let owner = bear(&pool, "pin-empty-catalog").await;
    let conversation_id = conversation(&pool, owner, "pin-empty-catalog").await;
    sqlx::query!("DELETE FROM model_selection_options")
        .execute(&pool)
        .await
        .unwrap();
    assert!(den_llm::model_registry::entry_for_handle("gpt-5").is_some());
    for model in ["gpt-5", "openai/gpt-5"] {
        assert!(matches!(
            apply_conversation_model_selection(
                &pool,
                conversation_id,
                "explicit",
                Some(model),
                "pin",
                "inherit",
            )
            .await,
            Err(DenError::ValidationError(_))
        ));
        assert!(
            persistence::get_conversation_model_state(&pool, conversation_id)
                .await
                .unwrap()
                .is_none(),
            "rejection must not create model state"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn auto_clears_revoked_pins_with_no_selectable_or_empty_catalog(pool: PgPool) {
    let owner = bear(&pool, "pin-clear-revoked").await;
    let revoked = conversation(&pool, owner, "pin-revoked").await;
    let removed = conversation(&pool, owner, "pin-removed").await;
    for id in [revoked, removed] {
        apply_conversation_model_selection(&pool, id, "explicit", Some("gpt-5"), "pin", "inherit")
            .await
            .unwrap();
    }
    sqlx::query!("UPDATE model_selection_options SET selectable = false")
        .execute(&pool)
        .await
        .unwrap();
    let before = state_snapshot(&pool, revoked).await;
    assert!(matches!(
        apply_conversation_model_selection(
            &pool,
            revoked,
            "explicit",
            Some("gpt-5"),
            "pin again",
            "inherit",
        )
        .await,
        Err(DenError::ValidationError(_))
    ));
    assert_eq!(state_snapshot(&pool, revoked).await, before);
    for id in [revoked, removed] {
        if id == removed {
            sqlx::query!("DELETE FROM model_selection_options")
                .execute(&pool)
                .await
                .unwrap();
        }
        // A stale form model must be ignored when the human chooses inheritance.
        let cleared = apply_conversation_model_selection(
            &pool,
            id,
            "auto",
            Some("missing/revoked"),
            "unused",
            "inherit",
        )
        .await
        .unwrap();
        assert_eq!(cleared.selection_mode, "auto");
        assert_eq!(cleared.requested_model, None);
        assert_eq!(cleared.selected_model, None);
        assert_eq!(cleared.selected_reason.as_deref(), Some("inherit"));
        let persisted = persistence::get_conversation_model_state(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(persisted).unwrap(),
            serde_json::to_value(cleared).unwrap()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn explicit_pin_aliases_normalize_before_canonical_persistence(pool: PgPool) {
    let owner = bear(&pool, "pin-aliases").await;
    let conversation_id = conversation(&pool, owner, "pin-aliases").await;
    for alias in [" gpt-5 ", "openai:gpt-5", "openai/gpt-5"] {
        let selected = apply_conversation_model_selection(
            &pool,
            conversation_id,
            " explicit ",
            Some(alias),
            "human pin",
            "unused",
        )
        .await
        .unwrap();
        assert_eq!(selected.selection_mode, "explicit");
        assert_eq!(selected.requested_model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(selected.selected_model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(selected.selected_reason.as_deref(), Some("human pin"));
        let persisted = persistence::get_conversation_model_state(&pool, conversation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(persisted).unwrap(),
            serde_json::to_value(selected).unwrap()
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_selection_modes_neither_replace_nor_clear_existing_pin(pool: PgPool) {
    let owner = bear(&pool, "pin-invalid-mode").await;
    let conversation_id = conversation(&pool, owner, "pin-invalid-mode").await;
    apply_conversation_model_selection(
        &pool,
        conversation_id,
        "explicit",
        Some("gpt-5"),
        "original pin",
        "inherit",
    )
    .await
    .unwrap();
    let before = state_snapshot(&pool, conversation_id).await;
    for mode in [
        "",
        "automatic",
        "inherit",
        "EXPLICIT",
        "explcit",
        "auto extra",
    ] {
        for requested in [Some("openai/gpt-4.1"), None] {
            assert!(matches!(
                apply_conversation_model_selection(
                    &pool,
                    conversation_id,
                    mode,
                    requested,
                    "replacement",
                    "clear",
                )
                .await,
                Err(DenError::ValidationError(_))
            ));
            assert_eq!(state_snapshot(&pool, conversation_id).await, before);
        }
    }
}
