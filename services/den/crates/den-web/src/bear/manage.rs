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
    ArmatureAvailability, BearCapability, EffectivePolicy, Governance, TurnExecutionOrigin,
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

/// A static description of where a tool may be offered; it does not grant
/// access to any particular person, session, Job, or work surface.
#[derive(Serialize)]
struct ToolMatrixRow {
    name: &'static str,
    origin: &'static str,
    note: &'static str,
    contexts: Vec<bool>,
}

const TOOL_CONTEXTS: [TurnExecutionOrigin; 3] = [
    TurnExecutionOrigin::ChannelConversation,
    TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
];

fn tool_matrix_context() -> Vec<ToolMatrixRow> {
    let mut rows: Vec<ToolMatrixRow> = builtin_den_tool_descriptors()
        .iter()
        .filter(|descriptor| !descriptor.allowed_origins.is_empty())
        .map(|descriptor| ToolMatrixRow {
            name: descriptor.name,
            origin: "built-in",
            note: descriptor.label,
            contexts: TOOL_CONTEXTS
                .iter()
                .map(|origin| descriptor.allows_origin(*origin))
                .collect(),
        })
        .collect();
    rows.extend(ClientToolName::all().iter().map(|tool| {
        let descriptor = tool.descriptor();
        ToolMatrixRow {
            name: descriptor.provider_name,
            origin: "local (armature)",
            note: descriptor.title,
            contexts: TOOL_CONTEXTS
                .iter()
                .map(|origin| {
                    EffectivePolicy::compile_for_origin(*origin, Governance::Interactive)
                        .capabilities
                        .contains(BearCapability::UseArmatureTools)
                })
                .collect(),
        }
    }));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_matrix_uses_execution_contexts_for_local_tool_availability() {
        let rows = tool_matrix_context();
        let read = rows
            .iter()
            .find(|row| row.name == ClientToolName::ReadTextFile.descriptor().provider_name)
            .expect("local read tool is listed");
        assert_eq!(read.contexts, [false, true, true]);
    }
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
            manage_title => "Purpose",
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
            manage_title => "Backup & move",
            ..bear_nav_context(&bear, "portability"),
        },
    )
    .await
}
