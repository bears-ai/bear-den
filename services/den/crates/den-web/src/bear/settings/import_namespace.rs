//! Bounded, nonce-owned staging leases. All allocation, claims, active guards and
//! expiry decisions share one process lock; cleanup never traverses Bear storage.
use super::import_staging::ReviewNonce;
use crate::errors::CustomError;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path as FsPath, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};
use uuid::Uuid;

pub(super) const MAX_PENDING: u8 = 8;
pub(super) const TTL_SECONDS: i64 = 15 * 60;
mod cleanup;
pub use cleanup::{cleanup_expired_import_reviews, start_import_staging_cleanup};
use cleanup::{reap_locked, remove_owned};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Phase {
    Upload,
    Pending,
    Claimed,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Lease {
    pub nonce: ReviewNonce,
    pub expires_at: i64,
    pub phase: Phase,
}

#[derive(Default)]
struct Namespace {
    active: HashMap<PathBuf, Active>,
}
struct Active {
    nonce: ReviewNonce,
    count: usize,
    retire: bool,
}
static NAMESPACE_LOCK: OnceLock<Mutex<Namespace>> = OnceLock::new();

fn lock() -> MutexGuard<'static, Namespace> {
    NAMESPACE_LOCK
        .get_or_init(|| Mutex::new(Namespace::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(super) fn root(config: &den_core::config::Config) -> PathBuf {
    FsPath::new(&config.bear_sqlite_data_dir).join(".import-reviews")
}
fn unavailable() -> CustomError {
    CustomError::ValidationError("This import review is expired, cancelled, already used, or belongs to another session. Upload the bundle again.".into())
}
fn private_directory(path: &FsPath) -> Result<(), CustomError> {
    let metadata = fs::symlink_metadata(path).map_err(staging_io_error)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(CustomError::System(
            "Import staging requires private directories (mode 0700).".into(),
        ));
    }
    Ok(())
}

pub(super) fn open_private(path: &FsPath) -> Result<fs::File, CustomError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(unavailable());
    }
    fs::File::open(path).map_err(staging_io_error)
}

fn staging_io_error(error: std::io::Error) -> CustomError {
    if error.kind() == std::io::ErrorKind::NotFound {
        unavailable()
    } else {
        error.into()
    }
}

fn lease_path(directory: &FsPath, nonce: ReviewNonce) -> PathBuf {
    directory.join(format!("{}.lease", nonce.0))
}
fn read_lease(directory: &FsPath, nonce: ReviewNonce) -> Result<Lease, CustomError> {
    let mut bytes = Vec::new();
    open_private(&lease_path(directory, nonce))?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(unavailable());
    }
    let lease: Lease = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if lease.nonce != nonce {
        return Err(unavailable());
    }
    Ok(lease)
}
fn write_lease(directory: &FsPath, lease: &Lease) -> Result<(), CustomError> {
    let temporary = directory.join(format!("{}.tmp", lease.nonce.0));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(lease).map_err(|_| unavailable())?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, lease_path(directory, lease.nonce))?;
    Ok(())
}

pub(super) struct Guard {
    directory: PathBuf,
    nonce: ReviewNonce,
    retire: bool,
}
impl Guard {
    fn acquire(
        namespace: &mut Namespace,
        directory: PathBuf,
        nonce: ReviewNonce,
        retire: bool,
    ) -> Result<Self, CustomError> {
        let active = namespace.active.entry(directory.clone()).or_insert(Active {
            nonce,
            count: 0,
            retire: false,
        });
        if active.nonce != nonce || active.retire {
            return Err(unavailable());
        }
        active.count += 1;
        Ok(Self {
            directory,
            nonce,
            retire,
        })
    }
    pub(super) fn path(&self, extension: &str) -> PathBuf {
        self.directory.join(format!("{}.{extension}", self.nonce.0))
    }
    pub(super) fn retain(&mut self) {
        self.retire = false;
    }
    pub(super) fn pending(&self, expires_at: i64) -> Result<(), CustomError> {
        let _namespace = lock();
        let lease = read_lease(&self.directory, self.nonce)?;
        if lease.phase != Phase::Upload {
            return Err(unavailable());
        }
        write_lease(
            &self.directory,
            &Lease {
                nonce: self.nonce,
                expires_at,
                phase: Phase::Pending,
            },
        )
    }
    pub(super) fn with_pending<T>(
        &self,
        work: impl FnOnce() -> Result<T, CustomError>,
    ) -> Result<T, CustomError> {
        let namespace = lock();
        let lease = read_lease(&self.directory, self.nonce)?;
        if lease.phase != Phase::Pending
            || lease.expires_at <= now()
            || !namespace
                .active
                .get(&self.directory)
                .is_some_and(|active| active.nonce == self.nonce && !active.retire)
            || !self.path("bear").is_file()
        {
            return Err(unavailable());
        }
        work()
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut namespace = lock();
        let Some(active) = namespace.active.get_mut(&self.directory) else {
            return;
        };
        if active.nonce != self.nonce {
            return;
        }
        active.retire |= self.retire;
        active.count = active.count.saturating_sub(1);
        if active.count == 0 {
            let retire = active.retire;
            namespace.active.remove(&self.directory);
            if retire && remove_owned(&self.directory, self.nonce).is_err() {
                tracing::warn!(nonce = %self.nonce.0, "Import staging removal deferred to cleanup");
            }
        }
    }
}

pub(super) fn reserve(
    config: &den_core::config::Config,
) -> Result<(Guard, fs::File, u8, ReviewNonce), CustomError> {
    let mut namespace = lock();
    fs::create_dir_all(&config.bear_sqlite_data_dir)?;
    let root = root(config);
    match fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private_directory(&root)?;
        }
        Err(error) => return Err(error.into()),
    }
    // Finish the complete reap pass before any allocation can return early.
    reap_locked(&namespace, &root, now())?;
    for slot in 0..MAX_PENDING {
        let directory = root.join(slot.to_string());
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
        let nonce = ReviewNonce(Uuid::new_v4());
        if let Err(error) = write_lease(
            &directory,
            &Lease {
                nonce,
                phase: Phase::Upload,
                expires_at: now() + TTL_SECONDS,
            },
        ) {
            let _ = remove_owned(&directory, nonce);
            return Err(error);
        }
        let guard = Guard::acquire(&mut namespace, directory, nonce, true)?;
        let opened = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(guard.path("bear"));
        drop(namespace);
        return Ok((guard, opened?, slot, nonce));
    }
    Err(CustomError::ValidationError(
        "Import staging is full. Cancel an outstanding review or retry after 15 minutes.".into(),
    ))
}

pub(super) fn read_guard(
    config: &den_core::config::Config,
    slot: u8,
    nonce: ReviewNonce,
) -> Result<Guard, CustomError> {
    let mut namespace = lock();
    let directory = directory(config, slot)?;
    let lease = read_lease(&directory, nonce)?;
    if lease.phase != Phase::Pending || lease.expires_at <= now() {
        return Err(unavailable());
    }
    Guard::acquire(&mut namespace, directory, nonce, false)
}
fn directory(config: &den_core::config::Config, slot: u8) -> Result<PathBuf, CustomError> {
    if slot >= MAX_PENDING {
        return Err(unavailable());
    }
    let root = root(config);
    private_directory(&root)?;
    let directory = root.join(slot.to_string());
    private_directory(&directory)?;
    Ok(directory)
}

pub(super) fn claim(
    config: &den_core::config::Config,
    slot: u8,
    nonce: ReviewNonce,
    check: impl FnOnce(&FsPath) -> Result<(), CustomError>,
) -> Result<Guard, CustomError> {
    let mut namespace = lock();
    let directory = directory(config, slot)?;
    let lease = read_lease(&directory, nonce)?;
    if lease.phase != Phase::Pending || lease.expires_at <= now() {
        return Err(unavailable());
    }
    check(&directory)?;
    let guard = Guard::acquire(&mut namespace, directory.clone(), nonce, true)?;
    let result = fs::rename(guard.path("bear"), guard.path("claimed"))
        .map_err(|_| unavailable())
        .and_then(|()| {
            write_lease(
                &directory,
                &Lease {
                    phase: Phase::Claimed,
                    ..lease
                },
            )
        });
    drop(namespace);
    result?;
    Ok(guard)
}

#[cfg(test)]
#[path = "tests/import_namespace.rs"]
mod tests;
