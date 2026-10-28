use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Query, State,
    },
    http::HeaderMap,
    Json,
};
use den_core::ids::{BearId, HatId};
use den_oauth::oauth::OAuthScope;
use den_service::{
    bears::{db, hats},
    conversation::persistence,
    DenState,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{access, error::Error};

#[derive(Debug, Serialize)]
pub(super) struct BearPublic {
    pub bear_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub is_bear_admin: bool,
}

pub(super) async fn bears(
    State(state): State<DenState>,
    headers: HeaderMap,
) -> Result<Json<Vec<BearPublic>>, Error> {
    let principal = access::authenticate(&state, &headers, OAuthScope::DataRead).await?;
    let rows = db::list_bears_for_user(&state.sqlx_pool, principal.user_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| BearPublic {
                bear_id: row.bear.id,
                slug: row.bear.slug,
                name: row.bear.name,
                description: row.bear.description,
                is_bear_admin: db::role_is_bear_admin(row.membership_role.as_deref()),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    pub bear_id: Uuid,
}
#[derive(Debug, Serialize)]
pub(super) struct Conversation {
    pub id: String,
    pub title: String,
    pub hat_id: Option<Uuid>,
    pub can_send: bool,
    pub last_message_at: String,
    pub session_uri: String,
    pub chat_uri: String,
}
#[derive(Debug, Serialize)]
pub(super) struct Hat {
    pub id: Uuid,
    pub name: String,
    pub short_summary: Option<String>,
}
#[derive(Serialize)]
pub(super) struct Inventory {
    pub conversations: Vec<Conversation>,
    pub hats: Vec<Hat>,
}

pub(super) async fn inventory(state: &DenState, user: i32, bear: Uuid) -> Result<Inventory, Error> {
    let viewer = access::viewer(state, bear, user).await?;
    let rows = viewer.list_visible(&state.sqlx_pool, 100).await?;
    let mut conversations = Vec::new();
    for row in rows {
        let Some(external) = row.external_conversation_id.clone() else {
            continue;
        };
        let hat =
            hats::bindings::conversation_hat(&state.sqlx_pool, BearId::new(bear), row.id).await?;
        conversations.push(Conversation {
            id: external,
            title: row
                .current_title
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Conversation".into()),
            hat_id: hat.map(|hat| hat.as_uuid()),
            can_send: access::can_send(state, user, &row).await?,
            last_message_at: timestamp(row.updated_at),
            session_uri: format!("ahp-session:/{}", row.id),
            chat_uri: format!("ahp-chat:/{}", row.id),
        });
    }
    let hats = hats::list_hats(&state.sqlx_pool, BearId::new(bear))
        .await?
        .into_iter()
        .map(|hat| Hat {
            id: hat.id.as_uuid(),
            name: hat.name,
            short_summary: hat.short_summary,
        })
        .collect();
    Ok(Inventory {
        conversations,
        hats,
    })
}

pub(super) async fn list(
    State(state): State<DenState>,
    headers: HeaderMap,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Inventory>, Error> {
    let principal = access::authenticate(&state, &headers, OAuthScope::DataRead).await?;
    let Query(query) = query.map_err(|_| Error::invalid("bear_id must be a UUID."))?;
    Ok(Json(
        inventory(&state, principal.user_id, query.bear_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    pub bear_id: Uuid,
    pub hat_id: Uuid,
    pub request_id: Uuid,
}
#[derive(Debug, Serialize)]
pub(super) struct Created {
    pub id: String,
    pub title: String,
    pub hat_id: Uuid,
    pub session_uri: String,
    pub chat_uri: String,
}

pub(super) fn external_id(user: i32, request: &Create) -> String {
    let key = format!(
        "den-native-conversation:{}:{user}:{}",
        request.bear_id, request.request_id
    );
    format!(
        "conv-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes()).simple()
    )
}

pub(super) async fn create(
    State(state): State<DenState>,
    headers: HeaderMap,
    body: Result<Json<Create>, JsonRejection>,
) -> Result<Json<Created>, Error> {
    let principal = access::authenticate(&state, &headers, OAuthScope::DataWrite).await?;
    let Json(request) =
        body.map_err(|_| Error::invalid("Expected bear_id, hat_id and request_id UUIDs."))?;
    if request.request_id.is_nil() || request.hat_id.is_nil() {
        return Err(Error::invalid(
            "Non-nil request and hat UUIDs are required.",
        ));
    }
    Ok(Json(create_for(&state, principal.user_id, &request).await?))
}

pub(super) async fn create_for(
    state: &DenState,
    user: i32,
    request: &Create,
) -> Result<Created, Error> {
    access::membership(state, user, request.bear_id).await?;
    hats::manage::get_hat(
        &state.sqlx_pool,
        BearId::new(request.bear_id),
        HatId::new(request.hat_id),
    )
    .await?;
    let external = external_id(user, request);
    let mut tx = state.sqlx_pool.begin().await?;
    // The request UUID scopes one owner-bound command. A changed hat is a conflict,
    // never a request to rebind an already published source.
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            external
        )
        .execute(&mut *tx),
    )
    .await
    .map_err(|_| {
        Error::conflict(
            "Conversation creation is already in progress. Retry the same request UUID.",
        )
    })??;
    if let Some(existing) =
        persistence::get_conversation_for_external_id(&state.sqlx_pool, request.bear_id, &external)
            .await?
    {
        let viewer = access::viewer(state, request.bear_id, user).await?;
        let hat = hats::bindings::conversation_hat(
            &state.sqlx_pool,
            BearId::new(request.bear_id),
            existing.id,
        )
        .await?;
        if !viewer
            .may_read_own_source(&state.sqlx_pool, existing.id)
            .await?
            || hat != Some(HatId::new(request.hat_id))
        {
            return Err(Error::conflict(
                "Request UUID already names an unavailable conversation or another hat.",
            ));
        }
        tx.commit().await?;
        return Ok(created(&external, request.hat_id, &existing));
    }
    let source_session = format!(
        "native-api:{}:{user}:{}",
        request.bear_id, request.request_id
    );
    let hat = Some(request.hat_id);
    let work: Option<Uuid> = None;
    // Exact existing cached source-publication SQL; only its ordinary owner/hat
    // branch is used. Publication is atomic, without an unbound intermediate row.
    sqlx::query_scalar!(
        r#"INSERT INTO conversations (bear_id, created_by_user_id, external_conversation_id, source_client_session_id, hat_id)
           SELECT $1, $2, $3, $4, $5 FROM user_bear ub
           WHERE ub.bear_id = $1 AND ub.user_id = $2
             AND NOT EXISTS (SELECT 1 FROM conversations c
                 WHERE c.bear_id = $1 AND c.external_conversation_id = $6)
             AND NOT EXISTS (SELECT 1 FROM archived_conversations a
                 WHERE a.bear_id = $1 AND a.conversation_id = $6)
             AND (
                 ($5::uuid IS NOT NULL AND $7::uuid IS NULL
                  AND EXISTS (SELECT 1 FROM bear_hats h WHERE h.id = $5 AND h.bear_id = $1)
                  AND NOT EXISTS (SELECT 1 FROM turn_runs r WHERE r.session_id = $4)
                  AND NOT EXISTS (SELECT 1 FROM bear_work_runs r
                      WHERE r.bearwire_session_id = $4 OR r.attached_client_session_id = $4))
                 OR ($5::uuid IS NULL AND $7::uuid IS NOT NULL
                     AND EXISTS (SELECT 1 FROM bear_work_runs r JOIN bear_jobs j ON j.id = r.job_id
                         WHERE r.id = $7 AND r.bear_id = $1 AND j.bear_id = $1
                           AND j.created_by_user_id = $2 AND r.bearwire_session_id = $4
                           AND NOT r.cancel_requested
                           AND r.state IN ('claimed', 'provisioning', 'running', 'reporting')))
             )
           RETURNING id"#,
        request.bear_id,user,external,source_session,hat,external,work,
    ).fetch_optional(&mut *tx).await?.ok_or_else(Error::unavailable)?;
    tx.commit().await?;
    let record = access::selected(state, user, request.bear_id, &external).await?;
    Ok(created(&external, request.hat_id, &record))
}

fn created(external: &str, hat: Uuid, row: &persistence::ConversationRecord) -> Created {
    Created {
        id: external.into(),
        title: row
            .current_title
            .clone()
            .unwrap_or_else(|| "New chat".into()),
        hat_id: hat,
        session_uri: format!("ahp-session:/{}", row.id),
        chat_uri: format!("ahp-chat:/{}", row.id),
    }
}
pub(super) fn timestamp(value: time::OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
