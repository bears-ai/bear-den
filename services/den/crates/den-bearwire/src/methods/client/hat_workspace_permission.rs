//! Only the winning local-tool permission result can persist a read-only
//! workspace grant. This boundary parses the stored obligation and currently
//! authenticated editor session; the grant writer rechecks the admin-owned hat
//! under the same transaction as result insertion.

use std::path::{Component, Path, PathBuf};

use den_core::ids::{BearId, UserId};
use den_http::errors::CustomError;
use den_runtime::turn_runs::WorkspacePermissionGrant;
use den_service::{
    bears::hats::access::{ReadOnlyWorkspaceAction, WorkspaceRoot},
    client_sessions::ClientSessionRow,
    conversation::persistence,
};
use serde_json::Value;
use sqlx::PgPool;

fn bounded_target(
    raw_path: &str,
    root: &WorkspaceRoot,
    cwd: Option<&str>,
) -> Result<(), CustomError> {
    if raw_path.trim().is_empty()
        || raw_path.contains("://")
        || raw_path.chars().any(char::is_control)
    {
        return Err(CustomError::ValidationError(
            "persistent workspace approval requires a normal filesystem path".into(),
        ));
    }
    let requested = Path::new(raw_path);
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        let cwd = cwd.ok_or_else(|| {
            CustomError::ValidationError("relative tool target has no trusted editor cwd".into())
        })?;
        Path::new(cwd).join(requested)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) => {
                return Err(CustomError::ValidationError(
                    "workspace grant does not support this tool path format".into(),
                ));
            }
        }
    }
    if !normalized.starts_with(root.as_str()) {
        return Err(CustomError::Authorization(
            "tool target is outside the exact workspace root".into(),
        ));
    }
    Ok(())
}

pub(super) async fn validated_grant(
    pool: &PgPool,
    bear_id: BearId,
    actor: UserId,
    conversation_external_id: &str,
    session: &ClientSessionRow,
    obligation_payload: &Value,
    raw_root: &str,
) -> Result<WorkspacePermissionGrant, CustomError> {
    if session.closed_at.is_some() || session.archived_at.is_some() {
        return Err(CustomError::Authorization(
            "closed editor sessions cannot create hat permissions".into(),
        ));
    }
    let action = ReadOnlyWorkspaceAction::from_provider_name(
        obligation_payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let root = WorkspaceRoot::parse(raw_root)?;
    let workspace = session.trusted_workspace_context();
    if !workspace
        .roots
        .iter()
        .filter_map(|candidate| WorkspaceRoot::parse(candidate).ok())
        .any(|candidate| candidate == root)
    {
        return Err(CustomError::Authorization(
            "workspace root is not assigned to this IDE session".into(),
        ));
    }
    let raw_path = obligation_payload
        .pointer("/arguments/path")
        .or_else(|| obligation_payload.pointer("/arguments/root"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CustomError::ValidationError(
                "persistent workspace approval requires an exact tool path".into(),
            )
        })?;
    bounded_target(raw_path, &root, workspace.cwd.as_deref())?;
    let conversation = persistence::get_conversation_for_external_id(
        pool,
        bear_id.as_uuid(),
        conversation_external_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("permission has no canonical conversation".into()))?;
    Ok(WorkspacePermissionGrant {
        bear_id,
        conversation_id: conversation.id,
        actor,
        action,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_root_cannot_absorb_outside_or_unresolved_targets() {
        let root = WorkspaceRoot::parse("/workspace/project").unwrap();
        assert!(bounded_target("src/main.rs", &root, Some("/workspace/project")).is_ok());
        assert!(bounded_target("/workspace/project/src/main.rs", &root, None).is_ok());
        for (path, cwd) in [
            ("../other/file", Some("/workspace/project")),
            ("/workspace/project/../../secrets", None),
            ("/workspace/project-sibling/file", None),
            ("src/main.rs", None),
        ] {
            assert!(bounded_target(path, &root, cwd).is_err(), "{path}");
        }
    }
}
