use den_core::{
    ids::{BearId, HatId, ModelConfigurationId, UserId},
    DenError, ThinkingEffort,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::bears::{db, hats};

mod capability_backfill;
mod catalog;
mod compatibility;
mod conversation_pins;
mod migration;
mod persistence;
mod resolution;

fn bear_params<'a>(slug: &'a str, model: Option<&'a str>) -> db::BearParams<'a> {
    db::BearParams {
        slug,
        name: "Configuration Test Bear",
        description: "test",
        system_prompt: "test",
        default_model: model,
        tools_enabled: None,
        context_profile: None,
    }
}

async fn bear(pool: &PgPool, slug: &str) -> BearId {
    db::create_bear(pool, bear_params(slug, None))
        .await
        .unwrap()
        .into()
}

async fn hat(pool: &PgPool, bear_id: BearId) -> HatId {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('config@example.test', 'configurationuser') RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    hats::create_hat(
        pool,
        bear_id,
        UserId::new(user),
        "Engineering",
        "Engineering work",
    )
    .await
    .unwrap()
    .id
}

async fn reasoning_support(pool: &PgPool, support: Option<bool>) {
    sqlx::query!(
        r"UPDATE model_selection_options
          SET metadata_json = jsonb_set(metadata_json, '{supports_reasoning_effort}',
              COALESCE(to_jsonb($1::boolean), 'null'::jsonb))
          WHERE handle = 'openai/gpt-5'",
        support,
    )
    .execute(pool)
    .await
    .unwrap();
}
