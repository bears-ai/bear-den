//! Small session metadata plus guarded, private file staging. Review receipts
//! belong to a nonce, not the replaceable session pointer.
use super::import_namespace::{self as namespace, Guard};
use super::{bundle_io::read_bear_bundle, BearBundleManifest, BEAR_BUNDLE_MAX_UPLOAD_BYTES};
use crate::errors::CustomError;
use den_core::ids::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path as FsPath,
};
use uuid::Uuid;

pub(super) const REVIEW_KEY: &str = "bear_import_review";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ReviewNonce(pub Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PendingReview {
    pub nonce: ReviewNonce,
    owner: UserId,
    session_id: String,
    expires_at: i64,
    slot: u8,
    hash: [u8; 32],
}
fn unavailable() -> CustomError {
    CustomError::ValidationError("This import review is expired, cancelled, already used, or belongs to another session. Upload the bundle again.".into())
}

pub(super) struct StagedFile {
    guard: Guard,
    slot: u8,
    nonce: ReviewNonce,
}
impl StagedFile {
    pub(super) fn reserve(
        config: &den_core::config::Config,
    ) -> Result<(Self, fs::File), CustomError> {
        let (guard, file, slot, nonce) = namespace::reserve(config)?;
        Ok((Self { guard, slot, nonce }, file))
    }
    pub(super) fn preview(&self) -> Result<(BearBundleManifest, [u8; 32]), CustomError> {
        preview(&self.guard.path("bear"), None)
    }
    pub(super) fn finish(
        &mut self,
        owner: UserId,
        session_id: String,
        hash: [u8; 32],
    ) -> Result<PendingReview, CustomError> {
        let expires_at = namespace::now() + namespace::TTL_SECONDS;
        self.guard.pending(expires_at)?;
        Ok(PendingReview {
            nonce: self.nonce,
            owner,
            session_id,
            expires_at,
            slot: self.slot,
            hash,
        })
    }
    pub(super) fn retain_pending(&mut self) {
        self.guard.retain();
    }
}

fn hash_file(file: &mut fs::File) -> Result<[u8; 32], CustomError> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_usize;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size.checked_add(read).ok_or_else(unavailable)?;
        if size > BEAR_BUNDLE_MAX_UPLOAD_BYTES {
            return Err(unavailable());
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}
fn checked_file(path: &FsPath) -> Result<fs::File, CustomError> {
    let file = namespace::open_private(path)?;
    if file.metadata()?.len() > BEAR_BUNDLE_MAX_UPLOAD_BYTES as u64 {
        return Err(unavailable());
    }
    Ok(file)
}
fn check_hash(actual: [u8; 32], expected: [u8; 32]) -> Result<(), CustomError> {
    if actual != expected {
        return Err(CustomError::ValidationError(
            "The staged bundle changed. Nothing was imported; upload it again for a fresh review."
                .into(),
        ));
    }
    Ok(())
}
fn preview(
    path: &FsPath,
    expected: Option<[u8; 32]>,
) -> Result<(BearBundleManifest, [u8; 32]), CustomError> {
    let mut file = checked_file(path)?;
    let hash = hash_file(&mut file)?;
    if let Some(expected) = expected {
        check_hash(hash, expected)?;
    }
    file.seek(SeekFrom::Start(0))?;
    let manifest = super::bundle_io::preview_bear_bundle(&mut file)?;
    // Validate the same open file around manifest decoding, without retaining a
    // 256 MiB compressed copy or inflating SQLite on an ordinary preview GET.
    check_hash(hash_file(&mut file)?, hash)?;
    Ok((manifest, hash))
}

pub(super) struct ReviewRead {
    guard: Guard,
    pending: PendingReview,
}
impl ReviewRead {
    pub(super) fn verify(self) -> Result<(VerifiedReview, BearBundleManifest), CustomError> {
        let manifest = preview(&self.guard.path("bear"), Some(self.pending.hash))?.0;
        Ok((VerifiedReview { read: self }, manifest))
    }
}

pub(super) struct VerifiedReview {
    read: ReviewRead,
}
impl VerifiedReview {
    pub(super) fn mark_displayed(&self) -> Result<(), CustomError> {
        let read = &self.read;
        read.guard.with_pending(|| {
            read.pending.authorize_at(
                read.pending.owner,
                &read.pending.session_id,
                read.pending.nonce,
                namespace::now(),
            )?;
            let path = read.guard.path("reviewed");
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(&serde_json::to_vec(&read.pending).map_err(|_| unavailable())?)?;
                    file.sync_all()?;
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    read.pending.check_receipt(&path)
                }
                Err(error) => Err(error.into()),
            }
        })
    }
}

impl PendingReview {
    pub(super) fn authorize(
        &self,
        owner: UserId,
        session_id: &str,
        nonce: ReviewNonce,
    ) -> Result<(), CustomError> {
        self.authorize_at(owner, session_id, nonce, namespace::now())
    }
    fn authorize_at(
        &self,
        owner: UserId,
        session_id: &str,
        nonce: ReviewNonce,
        now: i64,
    ) -> Result<(), CustomError> {
        if self.owner != owner
            || self.session_id != session_id
            || self.nonce != nonce
            || now >= self.expires_at
        {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn open_review(
        &self,
        config: &den_core::config::Config,
    ) -> Result<ReviewRead, CustomError> {
        let guard = namespace::read_guard(config, self.slot, self.nonce)?;
        Ok(ReviewRead {
            guard,
            pending: self.clone(),
        })
    }
    fn check_receipt(&self, path: &FsPath) -> Result<(), CustomError> {
        let mut bytes = Vec::new();
        namespace::open_private(path)?
            .take(4097)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(unavailable());
        }
        let receipt: Self = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if &receipt != self {
            return Err(unavailable());
        }
        self.authorize_at(self.owner, &self.session_id, self.nonce, namespace::now())
    }
    pub(super) fn claim(
        &self,
        config: &den_core::config::Config,
    ) -> Result<StagedFile, CustomError> {
        let guard = namespace::claim(config, self.slot, self.nonce, |directory| {
            self.check_receipt(&directory.join(format!("{}.reviewed", self.nonce.0)))
                .map_err(|_| CustomError::ValidationError("Open the current import review page before confirming. Nothing was imported.".into()))
        })?;
        Ok(StagedFile {
            guard,
            slot: self.slot,
            nonce: self.nonce,
        })
    }
    pub(super) fn discard(&self, config: &den_core::config::Config) -> Result<(), CustomError> {
        let guard = namespace::claim(config, self.slot, self.nonce, |_| Ok(()))?;
        drop(guard);
        Ok(())
    }
    pub(super) fn read_claimed(
        &self,
        staged: &StagedFile,
    ) -> Result<(BearBundleManifest, Vec<u8>), CustomError> {
        if staged.nonce != self.nonce || staged.slot != self.slot {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        checked_file(&staged.guard.path("claimed"))?
            .take(BEAR_BUNDLE_MAX_UPLOAD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > BEAR_BUNDLE_MAX_UPLOAD_BYTES {
            return Err(unavailable());
        }
        check_hash(Sha256::digest(&bytes).into(), self.hash)?;
        // Commit reads and verifies one immutable byte buffer, so later filesystem
        // changes cannot substitute different manifest or SQLite bytes for creation.
        read_bear_bundle(&bytes)
    }
}

pub(super) fn write_chunk(
    file: &mut fs::File,
    size: &mut usize,
    bytes: &[u8],
) -> Result<(), CustomError> {
    *size = size.checked_add(bytes.len()).ok_or_else(unavailable)?;
    if *size > BEAR_BUNDLE_MAX_UPLOAD_BYTES {
        return Err(CustomError::ValidationError(
            ".bear bundle exceeds the 256 MiB upload limit".into(),
        ));
    }
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/import_staging.rs"]
mod tests;
