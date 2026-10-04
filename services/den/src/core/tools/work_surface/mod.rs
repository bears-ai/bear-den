//! Managed work-surface recognition from trusted session metadata.

#[cfg(test)]
pub(crate) use den_core::tools::work_surface::*;

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::{core::tools::session::DenToolInvocationContext, errors::DenError};

fn normalized_git_remote(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/').trim_end_matches(".git");
    let value = value
        .strip_prefix("git@")
        .and_then(|value| {
            value
                .split_once(':')
                .map(|(host, path)| format!("{host}/{path}"))
        })
        .or_else(|| {
            value.split_once("://").map(|(_, authority_and_path)| {
                authority_and_path
                    .strip_prefix("git@")
                    .unwrap_or(authority_and_path)
                    .to_string()
            })
        })
        .unwrap_or_else(|| value.to_string());
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn unique_git_remote_match_index<'a>(
    origins: &[String],
    upstreams: impl Iterator<Item = &'a str>,
) -> Option<usize> {
    let mut matches = upstreams.enumerate().filter_map(|(index, upstream)| {
        normalized_git_remote(upstream)
            .is_some_and(|upstream| origins.iter().any(|origin| origin == &upstream))
            .then_some(index)
    });
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn session_git_remote_origins(
    session: &den_service::client_sessions::ClientSessionRow,
) -> Vec<String> {
    session
        .adapter_environment
        .as_ref()
        .and_then(|environment| environment.get("git_remote_origins"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(normalized_git_remote)
        .collect()
}

pub(crate) async fn persist_recognized_session_surface(
    pool: &PgPool,
    context: &DenToolInvocationContext,
) -> Result<(), DenError> {
    let Some(client_session_id) = context.client_session_id.as_deref() else {
        return Ok(());
    };
    let Some(session) = den_service::client_sessions::find_for_user_bear_session_id(
        pool,
        context.user_id,
        context.bear_id,
        client_session_id,
    )
    .await?
    else {
        return Ok(());
    };
    if den_service::client_session_work_surface_resolutions::find(pool, session.id)
        .await?
        .is_some_and(|resolution| resolution.status == "confirmed")
    {
        return Ok(());
    }
    let assigned_surfaces =
        den_service::work_surfaces::list_surfaces_for_bears(pool, &[context.bear_id]).await?;
    let session_origins = session_git_remote_origins(&session);
    if let Some(match_index) = unique_git_remote_match_index(
        &session_origins,
        assigned_surfaces
            .iter()
            .map(|surface| surface.upstream_url.as_str()),
    ) {
        den_service::client_session_work_surface_resolutions::upsert(
            pool,
            session.id,
            assigned_surfaces[match_index].id,
            den_service::client_session_work_surface_resolutions::ClientSessionWorkSurfaceResolutionStatus::Resolved,
            json!({"kind": "git_remote_origin", "origins": session_origins}),
        )
        .await?;
        return Ok(());
    }
    den_service::client_session_work_surface_resolutions::clear_resolved(pool, session.id).await?;
    Ok(())
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod git_remote_tests {
    use super::{normalized_git_remote, unique_git_remote_match_index};

    #[test]
    fn normalizes_equivalent_git_remote_forms() {
        let expected = Some("github.com/bears-ai/bear-den".to_string());
        assert_eq!(
            normalized_git_remote("git@github.com:bears-ai/bear-den.git"),
            expected
        );
        assert_eq!(
            normalized_git_remote("https://github.com/bears-ai/bear-den.git/"),
            Some("github.com/bears-ai/bear-den".to_string())
        );
        assert_eq!(
            normalized_git_remote("ssh://git@github.com/bears-ai/bear-den.git"),
            Some("github.com/bears-ai/bear-den".to_string())
        );
    }

    #[test]
    fn matches_only_one_assigned_surface_for_session_origins() {
        let origins = vec!["github.com/bears-ai/bear-den".to_string()];
        assert_eq!(
            unique_git_remote_match_index(
                &origins,
                [
                    "ssh://git@github.com/bears-ai/bear-den.git",
                    "https://github.com/bears-ai/other.git",
                ]
                .into_iter(),
            ),
            Some(0)
        );
        assert_eq!(
            unique_git_remote_match_index(
                &origins,
                [
                    "https://github.com/bears-ai/bear-den.git",
                    "git@github.com:bears-ai/bear-den.git",
                ]
                .into_iter(),
            ),
            None
        );
        assert_eq!(
            unique_git_remote_match_index(
                &origins,
                ["https://github.com/bears-ai/other.git"].into_iter(),
            ),
            None
        );
    }
}
