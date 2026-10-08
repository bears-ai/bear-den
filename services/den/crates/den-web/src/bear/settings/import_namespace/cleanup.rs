//! Fixed-slot expiry sweeps and nonce-scoped physical removal.
use super::{
    lease_path, lock, now, private_directory, read_lease, root, unavailable, Namespace,
    ReviewNonce, MAX_PENDING, TTL_SECONDS,
};
use crate::errors::CustomError;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path as FsPath, PathBuf},
};
use uuid::Uuid;

const MAX_SLOT_FILES: usize = 6;
const EXTENSIONS: [&str; 6] = ["bear", "claimed", "lease", "reviewed", "tmp", "ready"];

// Unknown entries, symlinks and mixed ownership fail closed, rather than becoming
// a generic directory garbage collector. Enumeration itself is bounded.
fn owned_files(directory: &FsPath) -> Result<Option<(ReviewNonce, Vec<PathBuf>)>, CustomError> {
    private_directory(directory)?;
    let mut owner = None;
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)?.take(MAX_SLOT_FILES + 1) {
        let path = entry?.path();
        if files.len() == MAX_SLOT_FILES {
            return Err(unavailable());
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(unavailable());
        }
        let extension = path
            .extension()
            .and_then(|text| text.to_str())
            .ok_or_else(unavailable)?;
        if !EXTENSIONS.contains(&extension) {
            return Err(unavailable());
        }
        let nonce = ReviewNonce(
            path.file_stem()
                .and_then(|text| text.to_str())
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or_else(unavailable)?,
        );
        if owner.is_some_and(|owner| owner != nonce) {
            return Err(unavailable());
        }
        owner = Some(nonce);
        files.push(path);
    }
    Ok(owner.map(|owner| (owner, files)))
}
pub(super) fn remove_owned(directory: &FsPath, nonce: ReviewNonce) -> Result<(), CustomError> {
    if let Some((owner, files)) = owned_files(directory)? {
        if owner != nonce {
            return Err(unavailable());
        }
        for path in files {
            fs::remove_file(path)?;
        }
    }
    fs::remove_dir(directory)?;
    Ok(())
}

pub(super) fn reap_locked(
    namespace: &Namespace,
    root: &FsPath,
    at: i64,
) -> Result<usize, CustomError> {
    let mut removed = 0;
    let mut first_error = None;
    for slot in 0..MAX_PENDING {
        let directory = root.join(slot.to_string());
        if namespace.active.contains_key(&directory) {
            continue;
        }
        match reap_slot(&directory, at) {
            Ok(true) => removed += 1,
            Ok(false) => (),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(removed)
}

fn reap_slot(directory: &FsPath, at: i64) -> Result<bool, CustomError> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let Some((nonce, _)) = owned_files(directory)? else {
        fs::remove_dir(directory)?;
        return Ok(true);
    };
    let expires_at = if lease_path(directory, nonce).exists() {
        read_lease(directory, nonce)?.expires_at
    } else {
        // Compatibility for the earlier staging format and crashes before the
        // first lease rename. Never remove entries belonging to another nonce.
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_secs();
        i64::try_from(modified)
            .map_err(|_| unavailable())?
            .saturating_add(TTL_SECONDS)
    };
    if at < expires_at {
        return Ok(false);
    }
    remove_owned(directory, nonce)?;
    Ok(true)
}

/// One bounded pass over the eight import slots. Safe at startup and periodically;
/// active operations survive TTL, and crash-left pending/claimed leases expire.
pub fn cleanup_expired_import_reviews(
    config: &den_core::config::Config,
) -> Result<usize, CustomError> {
    cleanup_at(config, now())
}
pub(super) fn cleanup_at(config: &den_core::config::Config, at: i64) -> Result<usize, CustomError> {
    let namespace = lock();
    let root = root(config);
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
        Ok(_) => private_directory(&root)?,
    }
    reap_locked(&namespace, &root, at)
}

/// Parent startup hook: immediately sweep, then sweep every minute, without
/// requiring an upload request. Keep the JoinHandle if shutdown should abort it.
pub fn start_import_staging_cleanup(
    config: std::sync::Arc<den_core::config::Config>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let config = config.clone();
            let result =
                tokio::task::spawn_blocking(move || cleanup_expired_import_reviews(&config)).await;
            if !matches!(result, Ok(Ok(_))) {
                tracing::warn!("Import staging cleanup could not finish; check private staging permissions and storage availability");
            }
        }
    })
}
