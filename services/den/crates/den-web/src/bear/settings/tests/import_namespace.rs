use super::*;

struct Directory(den_core::config::Config);
impl Directory {
    fn new() -> Self {
        let mut config = den_core::config::Config::test_stub();
        config.bear_sqlite_data_dir = std::env::temp_dir()
            .join(format!("den-import-lease-{}", Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        Self(config)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.bear_sqlite_data_dir);
    }
}
fn expire(guard: &Guard) {
    let _namespace = lock();
    let lease = read_lease(&guard.directory, guard.nonce).unwrap();
    write_lease(
        &guard.directory,
        &Lease {
            expires_at: now() - 1,
            ..lease
        },
    )
    .unwrap();
}
fn pending(config: &den_core::config::Config) -> (u8, ReviewNonce) {
    let (mut guard, file, slot, nonce) = reserve(config).unwrap();
    drop(file);
    guard.pending(now() + TTL_SECONDS).unwrap();
    guard.retain();
    drop(guard);
    (slot, nonce)
}

#[test]
fn active_upload_read_and_claim_survive_expiry_and_other_allocations() {
    let directory = Directory::new();
    let (upload, file, _, _) = reserve(&directory.0).unwrap();
    expire(&upload);
    let (slot, nonce) = pending(&directory.0);
    let read = read_guard(&directory.0, slot, nonce).unwrap();
    expire(&read);
    let (slot, nonce) = pending(&directory.0);
    let claimed = claim(&directory.0, slot, nonce, |_| Ok(())).unwrap();
    expire(&claimed);
    assert_eq!(cleanup_expired_import_reviews(&directory.0).unwrap(), 0);
    let (other, other_file, _, _) = reserve(&directory.0).unwrap();
    assert!(upload.path("bear").exists());
    assert!(read.path("bear").exists());
    assert!(claimed.path("claimed").exists());
    drop(other_file);
    drop(other);
    drop(file);
    drop(upload);
    drop(claimed);
    assert!(read.path("bear").exists());
    drop(read);
    assert_eq!(cleanup_expired_import_reviews(&directory.0).unwrap(), 1);
}

#[test]
fn retired_review_paths_are_validation_rejections_not_filesystem_server_errors() {
    let directory = Directory::new();
    let (slot, nonce) = pending(&directory.0);
    drop(claim(&directory.0, slot, nonce, |_| Ok(())).unwrap());
    assert!(matches!(
        read_guard(&directory.0, slot, nonce),
        Err(CustomError::ValidationError(_))
    ));
    assert!(matches!(
        claim(&directory.0, slot, nonce, |_| Ok(())),
        Err(CustomError::ValidationError(_))
    ));
}

#[test]
fn allocation_reaps_every_slot_before_returning_the_first_free_slot() {
    let directory = Directory::new();
    let mut owners = Vec::new();
    for _ in 0..MAX_PENDING {
        owners.push(pending(&directory.0));
    }
    let root = root(&directory.0);
    let (slot, nonce) = owners[0];
    drop(claim(&directory.0, slot, nonce, |_| Ok(())).unwrap());
    for &(slot, nonce) in &owners[1..] {
        let _namespace = lock();
        let directory = root.join(slot.to_string());
        let lease = read_lease(&directory, nonce).unwrap();
        write_lease(
            &directory,
            &Lease {
                expires_at: now() - 1,
                ..lease
            },
        )
        .unwrap();
    }
    let (guard, file, slot, _) = reserve(&directory.0).unwrap();
    assert_eq!(slot, 0);
    for slot in 1..MAX_PENDING {
        assert!(!root.join(slot.to_string()).exists());
    }
    drop(file);
    drop(guard);
}

#[tokio::test]
async fn startup_sweep_removes_crash_left_pending_and_claimed_without_any_upload() {
    let directory = Directory::new();
    let (claimed_slot, claimed_nonce) = pending(&directory.0);
    let (slot, nonce) = pending(&directory.0);
    let pending_directory = root(&directory.0).join(slot.to_string());
    let mut claimed = claim(&directory.0, claimed_slot, claimed_nonce, |_| Ok(())).unwrap();
    expire(&claimed);
    let claimed_directory = claimed.directory.clone();
    // Model restart: retain the files but discard the old process's active map.
    claimed.retain();
    drop(claimed);
    {
        let _namespace = lock();
        let lease = read_lease(&pending_directory, nonce).unwrap();
        write_lease(
            &pending_directory,
            &Lease {
                expires_at: now() - 1,
                ..lease
            },
        )
        .unwrap();
    }
    let unrelated = FsPath::new(&directory.0.bear_sqlite_data_dir).join("surviving-bear.sqlite");
    fs::write(&unrelated, b"private memory").unwrap();
    let handle = start_import_staging_cleanup(std::sync::Arc::new(directory.0.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while claimed_directory.exists() || pending_directory.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    handle.abort();
    assert_eq!(fs::read(unrelated).unwrap(), b"private memory");
}

#[test]
fn stale_nonce_drop_cannot_remove_a_replacement_slot_owner() {
    let directory = Directory::new();
    let (mut guard, file, slot, nonce) = reserve(&directory.0).unwrap();
    drop(file);
    let old_directory = guard.directory.clone();
    // Simulate an obsolete guard reaching Drop after an ownership change. The
    // guard must not remove another nonce, even under a broken/stale caller.
    {
        let mut namespace = lock();
        remove_owned(&old_directory, nonce).unwrap();
        namespace.active.remove(&old_directory);
    }
    let (replacement, file, replacement_slot, _) = reserve(&directory.0).unwrap();
    assert_eq!(slot, replacement_slot);
    guard.retire = true;
    drop(guard);
    assert!(replacement.path("bear").exists());
    drop(file);
    drop(replacement);
}
