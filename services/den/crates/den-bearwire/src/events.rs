use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use bearwire_protocol::wire::bearwire_event_to_json_rpc_notification;
use den_core::ids::{BearId, UserId};
use den_http::errors::CustomError;
use den_runtime::bearwire_events;
use den_service::{
    client_sessions,
    conversation::{persistence, viewer::ConversationViewer},
    DenState,
};
use serde_json::{json, Value};

pub(crate) use bearwire_protocol::methods::EventPageQuery;

use crate::auth::authenticate_for_bear_slug;

const DEFAULT_EVENT_PAGE_LIMIT: i64 = 100;
const MAX_EVENT_PAGE_LIMIT: i64 = 500;

async fn require_session_conversation_access(
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
) -> Result<(), CustomError> {
    let viewer = ConversationViewer::resolve(
        &state.sqlx_pool,
        BearId::new(session.bear_id),
        UserId::new(session.user_id),
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("conversation not found".to_string()))?;
    for external_id in std::iter::once(session.conversation_id.as_str())
        .chain(session.resolved_conversation_id.as_deref())
    {
        if let Some(conversation) = persistence::get_conversation_for_external_id(
            &state.sqlx_pool,
            session.bear_id,
            external_id,
        )
        .await?
        {
            if !viewer
                .may_access_id(&state.sqlx_pool, conversation.id)
                .await?
            {
                return Err(CustomError::NotFound("conversation not found".to_string()));
            }
        }
    }
    Ok(())
}

pub(crate) fn events_page_body(
    session_id: &str,
    after: Option<i64>,
    mut events: Vec<bearwire_events::BearWireEventRow>,
    requested_limit: Option<i64>,
) -> Result<Value, CustomError> {
    let limit = requested_limit
        .unwrap_or(DEFAULT_EVENT_PAGE_LIMIT)
        .clamp(1, MAX_EVENT_PAGE_LIMIT) as usize;
    let has_more = events.len() > limit;
    if has_more {
        events.truncate(limit);
    }
    let next_after = events.last().map(|event| event.sequence_no).or(after);
    let events = events
        .into_iter()
        .map(|row| {
            let notification = bearwire_event_to_json_rpc_notification(row.event);
            Ok(json!({
                "sequence": row.sequence_no,
                "event": notification.params,
            }))
        })
        .collect::<Result<Vec<_>, CustomError>>()?;

    Ok(json!({
        "ok": true,
        "session_id": session_id,
        "events": events,
        "next_after": next_after,
        "has_more": has_more,
    }))
}

pub(crate) async fn events_page(
    State(state): State<DenState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<EventPageQuery>,
) -> Result<Json<Value>, CustomError> {
    let user_id = authenticate_for_bear_slug(&state, &headers, &query.bear_slug).await?;
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &query.bear_slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("BearWire session not found".to_string()))?;
    require_session_conversation_access(&state, &session).await?;
    let requested_limit = query
        .limit
        .unwrap_or(DEFAULT_EVENT_PAGE_LIMIT)
        .clamp(1, MAX_EVENT_PAGE_LIMIT);
    let events = bearwire_events::list_bearwire_events_after_for_user(
        &state.sqlx_pool,
        session.bear_id,
        user_id,
        &session.client_session_id,
        query.after,
        requested_limit + 1,
    )
    .await?;
    Ok(Json(events_page_body(
        &session.client_session_id,
        query.after,
        events,
        Some(requested_limit),
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path, Query, State},
        http::header,
    };
    use bearwire_protocol::wire::BearWireEvent;
    use den_http::armature_tokens;
    use den_service::{
        bears::db::{self as bears_db, BearParams, BEAR_ROLE_MEMBER},
        conversation::persistence::ensure_conversation_for_external_id,
    };
    use sqlx::types::time::OffsetDateTime;
    use uuid::Uuid;

    fn test_state(pool: sqlx::PgPool) -> DenState {
        let config = std::sync::Arc::new(den_core::config::Config::test_stub());
        DenState::new(
            pool,
            config.clone(),
            std::sync::Arc::new(den_service::bifrost::BifrostClient::new(config.as_ref())),
            den_memory::MemoryStoreManager::new(config.as_ref()),
        )
    }

    async fn test_user(pool: &sqlx::PgPool) -> i32 {
        let suffix = Uuid::new_v4().simple().to_string();
        sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
            format!("{suffix}@example.test"),
            format!("u{}", &suffix[..16]),
            "Event page user",
            "unused",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn test_bear(pool: &sqlx::PgPool) -> (Uuid, String) {
        let slug = format!("events-{}", Uuid::new_v4().simple());
        let bear_id = bears_db::create_bear(
            pool,
            BearParams {
                slug: &slug,
                name: "Event page Bear",
                description: "test",
                system_prompt: "test",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap();
        (bear_id, slug)
    }

    async fn test_token(pool: &sqlx::PgPool, user_id: i32, bear_id: Uuid) -> String {
        bears_db::grant_membership(pool, user_id, bear_id, Some(BEAR_ROLE_MEMBER))
            .await
            .unwrap();
        armature_tokens::create_for_bear(pool, user_id, bear_id, "event page test")
            .await
            .unwrap()
            .raw_token
    }

    async fn test_session(
        pool: &sqlx::PgPool,
        user_id: i32,
        bear_id: Uuid,
        slug: &str,
        session_id: &str,
    ) {
        let conversation_id = format!("conv-{}", Uuid::new_v4().simple());
        ensure_conversation_for_external_id(
            pool,
            bear_id,
            Some(user_id),
            &conversation_id,
            None,
            None,
        )
        .await
        .unwrap();
        client_sessions::upsert_session(
            pool,
            client_sessions::UpsertClientSession {
                user_id,
                bear_id,
                bear_slug: slug.to_string(),
                client_session_id: session_id.to_string(),
                runtime_session_id: format!("runtime-{user_id}-{bear_id}-{session_id}"),
                conversation_id,
                resolved_conversation_id: None,
                client: "event-page-test".to_string(),
                cwd: None,
                current_mode: None,
            },
        )
        .await
        .unwrap();
    }

    async fn page(
        pool: &sqlx::PgPool,
        token: &str,
        slug: &str,
        session_id: &str,
        after: Option<i64>,
        limit: Option<i64>,
    ) -> Result<Value, CustomError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        Ok(events_page(
            State(test_state(pool.clone())),
            headers,
            Path(session_id.to_string()),
            Query(EventPageQuery {
                bear_slug: slug.to_string(),
                after,
                limit,
            }),
        )
        .await?
        .0)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn events_page_only_replays_owned_events_for_matching_bear_and_member(
        pool: sqlx::PgPool,
    ) {
        let owner = test_user(&pool).await;
        let other = test_user(&pool).await;
        let (bear_id, slug) = test_bear(&pool).await;
        let (other_bear_id, other_slug) = test_bear(&pool).await;
        let owner_token = test_token(&pool, owner, bear_id).await;
        let other_token = test_token(&pool, other, bear_id).await;
        let other_bear_token = test_token(&pool, owner, other_bear_id).await;
        let session_id = format!("session-{}", Uuid::new_v4().simple());
        test_session(&pool, owner, bear_id, &slug, &session_id).await;
        test_session(&pool, other, bear_id, &slug, &session_id).await;
        test_session(&pool, owner, other_bear_id, &other_slug, &session_id).await;

        async fn append(
            pool: &sqlx::PgPool,
            session: &str,
            bear: Uuid,
            user: Option<i32>,
            kind: &str,
        ) -> i64 {
            bearwire_events::append_ephemeral_bearwire_event(
                pool,
                session,
                Some(bear),
                user,
                kind,
                json!({"private": kind}),
            )
            .await
            .unwrap()
            .sequence_no
        }
        append(&pool, &session_id, bear_id, Some(other), "other.private").await;
        append(
            &pool,
            &session_id,
            other_bear_id,
            Some(owner),
            "other_bear.private",
        )
        .await;
        append(&pool, &session_id, bear_id, None, "legacy.private").await;
        let first_sequence = append(&pool, &session_id, bear_id, Some(owner), "owner.first").await;
        append(&pool, &session_id, bear_id, Some(other), "other.second").await;
        let second_sequence =
            append(&pool, &session_id, bear_id, Some(owner), "owner.second").await;

        let first = page(&pool, &owner_token, &slug, &session_id, None, Some(1))
            .await
            .unwrap();
        assert_eq!(first["events"].as_array().unwrap().len(), 1);
        assert_eq!(first["events"][0]["event"]["type"], "owner.first");
        assert_eq!(first["next_after"], first_sequence);
        assert_eq!(first["has_more"], true);

        let second = page(
            &pool,
            &owner_token,
            &slug,
            &session_id,
            Some(first_sequence),
            Some(1),
        )
        .await
        .unwrap();
        assert_eq!(second["events"].as_array().unwrap().len(), 1);
        assert_eq!(second["events"][0]["event"]["type"], "owner.second");
        assert_eq!(second["next_after"], second_sequence);
        assert_eq!(second["has_more"], false);
        let empty = page(
            &pool,
            &owner_token,
            &slug,
            &session_id,
            Some(second_sequence),
            Some(1),
        )
        .await
        .unwrap();
        assert!(empty["events"].as_array().unwrap().is_empty());
        assert_eq!(empty["next_after"], second_sequence);

        let other_page = page(&pool, &other_token, &slug, &session_id, None, None)
            .await
            .unwrap();
        assert_eq!(other_page["events"].as_array().unwrap().len(), 2);
        assert_eq!(other_page["events"][0]["event"]["type"], "other.private");
        assert_eq!(other_page["events"][1]["event"]["type"], "other.second");
        let different_bear = page(
            &pool,
            &other_bear_token,
            &other_slug,
            &session_id,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(different_bear["events"].as_array().unwrap().len(), 1);
        assert_eq!(
            different_bear["events"][0]["event"]["type"],
            "other_bear.private"
        );

        bears_db::revoke_membership(&pool, other, bear_id)
            .await
            .unwrap();
        assert!(matches!(
            page(&pool, &other_token, &slug, &session_id, None, None).await,
            Err(CustomError::Authorization(_))
        ));
        let owner_page = page(&pool, &owner_token, &slug, &session_id, None, None)
            .await
            .unwrap();
        assert_eq!(owner_page["events"].as_array().unwrap().len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn events_page_checks_session_conversation_before_replay(pool: sqlx::PgPool) {
        let owner = test_user(&pool).await;
        let other = test_user(&pool).await;
        let (bear_id, slug) = test_bear(&pool).await;
        let token = test_token(&pool, other, bear_id).await;
        test_token(&pool, owner, bear_id).await;
        let session_id = format!("session-{}", Uuid::new_v4().simple());
        let conversation_id = format!("conv-{}", Uuid::new_v4().simple());
        ensure_conversation_for_external_id(
            &pool,
            bear_id,
            Some(owner),
            &conversation_id,
            None,
            None,
        )
        .await
        .unwrap();
        client_sessions::upsert_session(
            &pool,
            client_sessions::UpsertClientSession {
                user_id: other,
                bear_id,
                bear_slug: slug.clone(),
                client_session_id: session_id.clone(),
                runtime_session_id: format!("runtime-{session_id}"),
                conversation_id: conversation_id.clone(),
                resolved_conversation_id: None,
                client: "event-page-test".to_string(),
                cwd: None,
                current_mode: None,
            },
        )
        .await
        .unwrap();
        append_bearwire_event_for_test(&pool, &session_id, bear_id, other).await;
        assert!(matches!(
            page(&pool, &token, &slug, &session_id, None, None).await,
            Err(CustomError::NotFound(_))
        ));

        let own_conversation_id = format!("conv-{}", Uuid::new_v4().simple());
        ensure_conversation_for_external_id(
            &pool,
            bear_id,
            Some(other),
            &own_conversation_id,
            None,
            None,
        )
        .await
        .unwrap();
        client_sessions::upsert_session(
            &pool,
            client_sessions::UpsertClientSession {
                user_id: other,
                bear_id,
                bear_slug: slug.clone(),
                client_session_id: session_id.clone(),
                runtime_session_id: format!("runtime-{session_id}"),
                conversation_id: own_conversation_id,
                resolved_conversation_id: Some(conversation_id),
                client: "event-page-test".to_string(),
                cwd: None,
                current_mode: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            page(&pool, &token, &slug, &session_id, None, None).await,
            Err(CustomError::NotFound(_))
        ));
    }

    async fn append_bearwire_event_for_test(
        pool: &sqlx::PgPool,
        session: &str,
        bear: Uuid,
        user: i32,
    ) {
        bearwire_events::append_ephemeral_bearwire_event(
            pool,
            session,
            Some(bear),
            Some(user),
            "private",
            json!({"private": true}),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn events_page_endpoint_requires_bearer_token_for_bear_session() {
        let err = events_page(
            State(test_state(
                sqlx::PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/noop").unwrap(),
            )),
            HeaderMap::new(),
            Path("session-test".to_string()),
            Query(EventPageQuery {
                bear_slug: "meta".to_string(),
                after: None,
                limit: None,
            }),
        )
        .await
        .expect_err("missing auth should error");
        assert!(err.to_string().contains("missing Authorization"));
    }

    fn event_row(sequence_no: i64, event_type: &str) -> bearwire_events::BearWireEventRow {
        bearwire_events::BearWireEventRow {
            id: Uuid::new_v4(),
            sequence_no,
            session_id: "session-test".to_string(),
            event_type: event_type.to_string(),
            event: BearWireEvent::ephemeral(
                event_type,
                json!({
                    "sequence_no": sequence_no,
                }),
            ),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn events_page_body_uses_server_owned_next_after() {
        let body = events_page_body(
            "session-test",
            Some(41),
            vec![
                event_row(42, "run.progress"),
                event_row(43, "message.delta"),
            ],
            Some(100),
        )
        .unwrap();

        assert_eq!(body["ok"], true);
        assert_eq!(body["next_after"], 43);
        assert_eq!(body["has_more"], false);
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
        assert_eq!(body["events"][0]["sequence"], 42);
        assert_eq!(body["events"][0]["event"]["type"], "run.progress");
    }

    #[test]
    fn events_page_body_does_not_advance_empty_page_cursor() {
        let body = events_page_body("session-test", Some(41), Vec::new(), Some(100)).unwrap();

        assert_eq!(body["next_after"], 41);
        assert_eq!(body["has_more"], false);
        assert!(body["events"].as_array().unwrap().is_empty());
    }

    #[test]
    fn events_page_body_reports_has_more_without_advancing_past_returned_events() {
        let body = events_page_body(
            "session-test",
            Some(41),
            vec![
                event_row(42, "run.progress"),
                event_row(43, "message.delta"),
                event_row(44, "message.delta"),
            ],
            Some(2),
        )
        .unwrap();

        assert_eq!(body["has_more"], true);
        assert_eq!(body["next_after"], 43);
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
    }
}
