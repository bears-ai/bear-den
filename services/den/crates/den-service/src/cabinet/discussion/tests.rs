//! SQLx owns a fresh migrated database per test; no live Den database fallback.

use super::*;
use crate::cabinet::{self, pages};
use den_cabinet::{CabinetPolicy, CreateItemRequest, ItemKind};
use den_core::{execution_context::RuntimeContextLabel, ids::UserId};
use uuid::Uuid;

mod access;
mod audit;
mod concurrency;
mod content;
mod fence;
mod migrations;

async fn user(pool: &PgPool) -> ActorScope {
    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("discussion{}", &suffix[..12]);
    let id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash)
         VALUES ($1, $2, $2, 'x') RETURNING id",
        format!("{name}@example.invalid"),
        name,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    ActorScope::user(UserId(id))
}

async fn bear(pool: &PgPool, enabled: bool) -> ActorScope {
    let id = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name, cabinet_enabled) VALUES ($1, 'Discussion Bear', $2) RETURNING id",
        format!("discussion-{}", Uuid::new_v4().simple()),
        enabled,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    ActorScope::bear(
        den_core::ids::BearId::new(id),
        RuntimeContextLabel::ChannelConversation,
    )
}

fn user_id(scope: &ActorScope) -> i32 {
    let Actor::User { user_id } = scope.actor else {
        panic!("fixture requires a human");
    };
    user_id.0
}

fn request(scope: &ActorScope) -> CreateItemRequest {
    CreateItemRequest {
        scope: scope.clone(),
        kind: ItemKind::Document,
        title: "Discussion page".into(),
        content: "Published content".into(),
        collection_ref: None,
        mission_ref: None,
        source_links: Vec::new(),
    }
}

async fn fixture(pool: &PgPool) -> (ActorScope, CabinetItemRef) {
    let owner = user(pool).await;
    let page = cabinet::create_item(pool, request(&owner)).await.unwrap();
    (owner, page.item.cabinet_ref)
}

async fn entry_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM cabinet_discussion_entries"#)
        .fetch_one(pool)
        .await
        .unwrap()
}
