//! New-IA bear management areas: identity, skills, tools, connections,
//! portability — plus redirects from retired paths. Member-gated like the
//! rest of `/bear/{slug}/…` (read for members, write for bear admins).
//!
//! When changing routes, update `src/web/ROUTES.md`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::routing::RouterExt;
use den_core::{
    client_tools::ClientToolName,
    ids::{BearId, HatId},
    tools::descriptor::builtin_den_tool_descriptors,
    BearProfile,
};
use den_service::bears::hats;
use minijinja::context;
use serde::Serialize;

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

use super::settings::{bear_nav_context, load_session_bear};

#[derive(Serialize)]
struct HatIdentityRow {
    id: HatId,
    name: String,
    short_summary: Option<String>,
}

/// One tool, one row: identity and (future) configuration are per-tool;
/// stances only gate availability, expressed as the boolean columns.
#[derive(Serialize)]
struct ToolMatrixRow {
    name: &'static str,
    origin: &'static str,
    note: &'static str,
    stances: Vec<bool>,
}

/// Build the tool matrix for the Tools management page: unique tools as
/// rows, stance availability as columns.
///
/// Source: `den_core::tools::descriptor::builtin_den_tool_descriptors()` for
/// Den-hosted tools (each descriptor carries `allowed_roles`, so per-stance
/// availability is exact), plus `den_core::client_tools::ClientToolName::all()`
/// for armature-local tools, which are only ever exposed on the pair stance.
fn tool_matrix_context() -> Vec<ToolMatrixRow> {
    let mut rows: Vec<ToolMatrixRow> = builtin_den_tool_descriptors()
        .iter()
        .map(|descriptor| ToolMatrixRow {
            name: descriptor.name,
            origin: "built-in",
            note: descriptor.label,
            stances: BearProfile::ALL
                .iter()
                .map(|stance| descriptor.allowed_roles.contains(&stance.as_str()))
                .collect(),
        })
        .collect();
    rows.extend(ClientToolName::all().iter().map(|tool| {
        let descriptor = tool.descriptor();
        ToolMatrixRow {
            name: descriptor.provider_name,
            origin: "local (armature)",
            note: descriptor.title,
            stances: BearProfile::ALL
                .iter()
                .map(|stance| *stance == BearProfile::Pair)
                .collect(),
        }
    }));
    rows
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/bear/{slug}/identity", get(identity_view))
        .route_with_tsr("/bear/{slug}/skills", get(skills_view))
        .route_with_tsr("/bear/{slug}/tools", get(tools_view))
        .route_with_tsr("/bear/{slug}/connections", get(connections_view))
        .route_with_tsr("/bear/{slug}/portability", get(portability_view))
        // Retired paths from the previous IA.
        .route_with_tsr("/bear/{slug}/access", get(redirect_people))
        .route_with_tsr("/bear/{slug}/policy", get(redirect_resources))
}

async fn redirect_people(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/bear/{slug}/people"))
}

async fn redirect_resources(Path(slug): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/bear/{slug}/resources"))
}

async fn identity_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) =
        match super::settings::load_session_bear(&state, &auth_session, &slug).await? {
            Ok(v) => v,
            Err(r) => return Ok(r.into_response()),
        };
    let hats = hats::list_hats(state.sqlx_pool(), BearId::new(bear.id))
        .await?
        .into_iter()
        .map(|hat| HatIdentityRow {
            id: hat.id,
            name: hat.name,
            short_summary: hat.short_summary,
        })
        .collect::<Vec<_>>();
    web::render_template(
        &state,
        "bear/manage/identity.html",
        auth_session,
        context! {
            can_manage_bear,
            hats,
            manage_title => "Identity & charter",
            ..bear_nav_context(&bear, "identity"),
        },
    )
    .await
}

async fn skills_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    web::render_template(
        &state,
        "bear/manage/skills.html",
        auth_session,
        context! {
            can_manage_bear,
            manage_title => "Skills",
            ..bear_nav_context(&bear, "skills"),
        },
    )
    .await
}

async fn tools_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    let tools = tool_matrix_context();
    web::render_template(
        &state,
        "bear/manage/tools.html",
        auth_session,
        context! {
            can_manage_bear,
            manage_title => "Tools",
            tools,
            ..bear_nav_context(&bear, "tools"),
        },
    )
    .await
}

async fn connections_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    web::render_template(
        &state,
        "bear/manage/connections.html",
        auth_session,
        context! {
            can_manage_bear,
            manage_title => "Connections",
            ..bear_nav_context(&bear, "connections"),
        },
    )
    .await
}

async fn portability_view(
    Path(slug): Path<String>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let (bear, can_manage_bear) = match load_session_bear(&state, &auth_session, &slug).await? {
        Ok(v) => v,
        Err(r) => return Ok(r.into_response()),
    };
    web::render_template(
        &state,
        "bear/manage/portability.html",
        auth_session,
        context! {
            can_manage_bear,
            manage_title => "Portability",
            ..bear_nav_context(&bear, "portability"),
        },
    )
    .await
}
