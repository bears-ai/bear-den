use std::path::Path as FsPath;

use crate::errors::CustomError;

pub(super) fn is_absolute_local_path(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    FsPath::new(path).is_absolute()
        || path.starts_with("\\\\")
        || (path.len() >= 3
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'))
}

pub(super) fn optional_absolute_cwd_filter(raw: Option<&str>) -> Result<Option<&str>, CustomError> {
    let Some(cwd) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if is_absolute_local_path(cwd) {
        Ok(Some(cwd))
    } else {
        tracing::warn!(cwd = %cwd, "rejecting non-absolute ACP sessions cwd filter");
        Err(CustomError::ValidationError(
            "cwd filter must be an absolute local path".to_string(),
        ))
    }
}

pub(super) fn require_absolute_cwd(raw: Option<&str>) -> Result<String, CustomError> {
    let Some(cwd) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        tracing::warn!("rejecting ACP prompt with missing cwd");
        return Err(CustomError::ValidationError(
            "ACP client_context.cwd must be an absolute local path".to_string(),
        ));
    };
    if is_absolute_local_path(cwd) {
        Ok(cwd.to_string())
    } else {
        tracing::warn!(cwd = %cwd, "rejecting ACP prompt with non-absolute cwd");
        Err(CustomError::ValidationError(
            "ACP client_context.cwd must be an absolute local path".to_string(),
        ))
    }
}
