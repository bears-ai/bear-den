//! Shared, typed Cabinet routing and surface ownership; no web/runtime dependencies.

use std::{error::Error, fmt};
use url::Url;

use super::Config;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WebSurface {
    #[default]
    Den,
    Cabinet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CabinetDestination {
    Local,
    Dedicated(Url),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CabinetOriginError;

impl fmt::Display for CabinetOriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CABINET_URL must be an absolute http(s) URL; an optional trailing slash is allowed, but credentials, subpaths, query, and fragment are not")
    }
}

impl Error for CabinetOriginError {}

pub fn validate_origin(origin: &Url) -> Result<(), CabinetOriginError> {
    if !matches!(origin.scheme(), "http" | "https")
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(CabinetOriginError);
    }
    Ok(())
}

pub fn parse_optional_origin(raw: Option<&str>) -> Result<Option<Url>, CabinetOriginError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    // Reject input paths even when URL normalization would erase them (e.g. /..).
    let (_, authority_and_path) = raw.split_once("://").ok_or(CabinetOriginError)?;
    let (authority, path) = authority_and_path
        .split_once('/')
        .unwrap_or((authority_and_path, ""));
    if authority.contains('@') || !path.is_empty() || raw.contains('\\') {
        return Err(CabinetOriginError);
    }
    let origin = Url::parse(raw).map_err(|_| CabinetOriginError)?;
    validate_origin(&origin)?;
    Ok(Some(origin))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCabinetPath;

impl fmt::Display for InvalidCabinetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected a local /cabinet path")
    }
}

impl Error for InvalidCabinetPath {}

impl CabinetDestination {
    pub fn home_url(&self) -> String {
        match self {
            Self::Local => "/cabinet".into(),
            Self::Dedicated(origin) => format!("{}/cabinet", origin.origin().ascii_serialization()),
        }
    }

    /// Resolve a legacy local link without allowing it to select another origin.
    /// Query strings are preserved for rollout redirects and filtered views.
    pub fn resolve_path(&self, path: &str) -> Result<String, InvalidCabinetPath> {
        if !(path == "/cabinet" || path.starts_with("/cabinet/") || path.starts_with("/cabinet?"))
            || path.contains(['\\', '#'])
            || path.chars().any(char::is_control)
        {
            return Err(InvalidCabinetPath);
        }
        let base = match self {
            Self::Local => "http://cabinet.invalid/".to_string(),
            Self::Dedicated(origin) => origin.to_string(),
        };
        let base = Url::parse(&base).map_err(|_| InvalidCabinetPath)?;
        let resolved = base.join(path).map_err(|_| InvalidCabinetPath)?;
        if resolved.origin() != base.origin()
            || !(resolved.path() == "/cabinet" || resolved.path().starts_with("/cabinet/"))
        {
            return Err(InvalidCabinetPath);
        }
        match self {
            Self::Local => Ok(resolved[url::Position::BeforePath..].to_string()),
            Self::Dedicated(_) => Ok(resolved.to_string()),
        }
    }
}

impl Config {
    pub fn cabinet_destination(&self) -> CabinetDestination {
        self.cabinet_web_origin
            .clone()
            .map_or(CabinetDestination::Local, CabinetDestination::Dedicated)
    }

    pub fn cabinet_home_url(&self, surface: WebSurface) -> String {
        match surface {
            WebSurface::Den => self.cabinet_destination().home_url(),
            WebSurface::Cabinet => "/cabinet".into(),
        }
    }

    pub fn surface_public_origin(&self, surface: WebSurface) -> String {
        match (surface, &self.cabinet_web_origin) {
            (WebSurface::Cabinet, Some(origin)) => origin.origin().ascii_serialization(),
            _ => self.web_public_origin(),
        }
    }

    /// Both deployments must receive the same Cabinet origin setting. A dedicated
    /// Cabinet owns recovery when configured; Den owns the local rollout fallback.
    pub fn owns_cabinet_recovery(&self, surface: WebSurface) -> bool {
        matches!(
            (self.cabinet_web_origin.is_some(), surface),
            (true, WebSurface::Cabinet) | (false, WebSurface::Den)
        )
    }
}

#[cfg(test)]
#[path = "cabinet_tests.rs"]
mod tests;
