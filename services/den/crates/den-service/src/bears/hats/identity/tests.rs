use super::*;
use crate::bears::{
    db::{self, BearParams},
    hats, managed_blocks,
};
use den_core::ids::UserId;
use serde_json::json;
use sqlx::{types::Json, PgPool};

#[sqlx::test(migrations = "../../migrations")]
async fn bound_identity_is_selected_by_hat_and_not_by_interaction_mode(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('identityhat@example.test', 'identityhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "identityhatbear",
            name: "Lumen",
            description: "",
            system_prompt: "Bear-wide steering only",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let bear = db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let security = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Security",
        "Review risks",
    )
    .await
    .unwrap();
    let support = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Support",
        "Help customers",
    )
    .await
    .unwrap();
    let pair = bound_prompt_text(&pool, &bear, BearProfile::Pair, security.id)
        .await
        .unwrap();
    let work = bound_prompt_text(&pool, &bear, BearProfile::Work, security.id)
        .await
        .unwrap();
    let other = bound_prompt_text(&pool, &bear, BearProfile::Chat, support.id)
        .await
        .unwrap();
    for selected in [&pair, &work] {
        assert!(selected.contains("Bear-wide steering only"));
        assert!(selected.contains("Lumen, wearing the Security hat"));
        assert!(selected.contains("Review risks"));
        assert!(!selected.contains("Help customers"));
        assert!(!selected.contains("Collaboration Space"));
        assert!(!selected.contains("Execution Space"));
    }
    assert!(pair.contains("Interactive collaboration mode"));
    assert!(!pair.contains("Authorized Work mode"));
    assert!(work.contains("Authorized Work mode"));
    assert!(!work.contains("Interactive collaboration mode"));
    assert!(other.contains("Help customers"));
    assert!(!other.contains("Review risks"));
    assert!(matches!(
        bound_prompt_text(&pool, &bear, BearProfile::Curate, security.id).await,
        Err(DenError::Authorization(_))
    ));
    assert!(bound_prompt_text(
        &pool,
        &bear,
        BearProfile::Pair,
        HatId::new(uuid::Uuid::new_v4())
    )
    .await
    .is_err());
    hats::manage::update_hat(
        &pool,
        BearId::new(bear_id),
        security.id,
        "Security",
        "Review risks",
        "Inspect new risks {{ untrusted }}",
        false,
    )
    .await
    .unwrap();
    let refreshed = bound_prompt_text(&pool, &bear, BearProfile::Pair, security.id)
        .await
        .unwrap();
    assert!(refreshed.contains("Inspect new risks {{ untrusted }}"));
    assert!(!refreshed.contains("Help customers"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn managed_bound_base_compiles_without_stance_identity_or_role_contract(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('identitymanaged@example.test', 'identitymanaged') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let context_profile = json!({
        "composition_version": 1,
        "role_contracts": {
            "chat": "OLD CHAT IDENTITY", "pair": "OLD PAIR IDENTITY",
            "curate": "CURATE INTERNAL", "work": "OLD WORK IDENTITY", "watch": "WATCH INTERNAL"
        },
        "user_steering": "Keep answers concise", "bear_context": "Shared charter"
    });
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "identitymanagedbear",
            name: "Lumen",
            description: "",
            system_prompt: "legacy fallback",
            default_model: None,
            tools_enabled: None,
            context_profile: Some(Json(context_profile)),
        },
    )
    .await
    .unwrap();
    let bear = db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Security",
        "Review code",
    )
    .await
    .unwrap();
    let compiled = managed_blocks::compile_and_store_managed_config_for_bear(&pool, &bear)
        .await
        .unwrap();
    assert!(compiled.rendered_prompts["bound_base"]
        .as_str()
        .unwrap()
        .contains("Shared charter"));
    assert!(!compiled.rendered_prompts["bound_base"]
        .as_str()
        .unwrap()
        .contains("OLD PAIR IDENTITY"));
    assert!(compiled.rendered_prompt_hashes["bound_base"].is_string());
    assert!(compiled.rendered_prompt_hashes["bound_work_mode"].is_string());
    assert!(compiled.rendered_prompts["bound_source_version"].is_string());
    let internal = crate::bears::provision::profile_prompt_text(&pool, &bear, BearProfile::Curate)
        .await
        .unwrap();
    assert_eq!(
        internal,
        compiled.rendered_prompts["curate"].as_str().unwrap(),
        "internal curation must keep its independently compiled role prompt"
    );
    assert!(!internal.contains("wearing the Security hat"));
    let pair = bound_prompt_text(&pool, &bear, BearProfile::Pair, hat.id)
        .await
        .unwrap();
    let work = bound_prompt_text(&pool, &bear, BearProfile::Work, hat.id)
        .await
        .unwrap();
    for selected in [pair, work] {
        assert!(selected.contains("Keep answers concise"));
        assert!(selected.contains("Shared charter"));
        assert!(selected.contains("Review code"));
        assert!(!selected.contains("OLD PAIR IDENTITY"));
        assert!(!selected.contains("OLD WORK IDENTITY"));
    }
}
