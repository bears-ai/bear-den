use super::super::bundle_io::build_bear_bundle;
use super::*;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

fn fixture() -> Vec<u8> {
    build_bear_bundle("format: bear\nversion: 1\nbear:\n  slug: imported\n  name: Imported\n  description: Purpose\n  birthdate: '2020-01-01'\nprompts:\n  system_prompt: Identity\n", b"sqlite fixture").unwrap()
}
struct Directory(den_core::config::Config);
impl Directory {
    fn new() -> Self {
        let mut config = den_core::config::Config::test_stub();
        config.bear_sqlite_data_dir = std::env::temp_dir()
            .join(format!("den-import-stage-{}", Uuid::new_v4()))
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
fn stage(config: &den_core::config::Config, owner: i32, session: &str) -> PendingReview {
    let (mut staged, mut file) = StagedFile::reserve(config).unwrap();
    write_chunk(&mut file, &mut 0, &fixture()).unwrap();
    drop(file);
    let (_, hash) = staged.preview().unwrap();
    let pending = staged
        .finish(UserId::new(owner), session.into(), hash)
        .unwrap();
    staged.retain_pending();
    pending
}
fn display(pending: &PendingReview, config: &den_core::config::Config) {
    let (read, _) = pending.open_review(config).unwrap().verify().unwrap();
    read.mark_displayed().unwrap();
}
fn path(pending: &PendingReview, config: &den_core::config::Config, extension: &str) -> PathBuf {
    namespace::root(config)
        .join(pending.slot.to_string())
        .join(format!("{}.{extension}", pending.nonce.0))
}

#[test]
fn ownership_session_nonce_expiry_and_private_permissions_are_enforced() {
    let directory = Directory::new();
    let pending = stage(&directory.0, 1, "session-one");
    assert_eq!(
        fs::metadata(namespace::root(&directory.0))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(path(&pending, &directory.0, "bear"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(pending
        .authorize(UserId::new(1), "session-one", pending.nonce)
        .is_ok());
    assert!(pending
        .authorize(UserId::new(2), "session-one", pending.nonce)
        .is_err());
    assert!(pending
        .authorize(UserId::new(1), "session-two", pending.nonce)
        .is_err());
    assert!(pending
        .authorize(UserId::new(1), "session-one", ReviewNonce(Uuid::new_v4()))
        .is_err());
    assert!(pending
        .authorize_at(
            UserId::new(1),
            "session-one",
            pending.nonce,
            pending.expires_at
        )
        .is_err());
    assert!(pending
        .authorize_at(
            UserId::new(1),
            "session-one",
            pending.nonce,
            pending.expires_at - 1
        )
        .is_ok());
    let metadata = serde_json::to_string(&pending).unwrap();
    assert!(metadata.len() < 1024);
    assert!(!metadata.contains("Identity"));
    assert!(!metadata.contains(&directory.0.bear_sqlite_data_dir));
}

#[test]
fn receipt_is_nonce_hash_owner_and_session_bound_and_requires_successful_read() {
    let directory = Directory::new();
    let pending = stage(&directory.0, 1, "session");
    assert!(pending.claim(&directory.0).is_err());
    display(&pending, &directory.0);
    assert_eq!(
        fs::metadata(path(&pending, &directory.0, "reviewed"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    for changed in [
        PendingReview {
            owner: UserId::new(2),
            ..pending.clone()
        },
        PendingReview {
            session_id: "other-session".into(),
            ..pending.clone()
        },
        PendingReview {
            hash: [0; 32],
            ..pending.clone()
        },
    ] {
        assert!(changed.claim(&directory.0).is_err());
    }
    let claimed = pending.claim(&directory.0).unwrap();
    assert!(pending.read_claimed(&claimed).is_ok());
    drop(claimed);
    assert!(!path(&pending, &directory.0, "reviewed").exists());
    assert!(pending.claim(&directory.0).is_err());
}

#[test]
fn late_review_a_cannot_mark_or_authorize_replacement_b() {
    let directory = Directory::new();
    let a = stage(&directory.0, 1, "session");
    let (late_a, _) = a.open_review(&directory.0).unwrap().verify().unwrap();
    let b = stage(&directory.0, 1, "session");
    // This is the upload replacement path: A is retired while its GET/render
    // guard is still live, and only B remains the session pointer.
    a.discard(&directory.0).unwrap();
    assert!(late_a.mark_displayed().is_err());
    assert!(
        b.claim(&directory.0).is_err(),
        "A must not authorize B's unviewed bytes"
    );
    drop(late_a);
    assert!(!path(&a, &directory.0, "reviewed").exists());
    assert!(path(&b, &directory.0, "bear").exists());
    display(&b, &directory.0);
    drop(b.claim(&directory.0).unwrap());
}

#[test]
fn tamper_is_denied_and_cancel_and_replay_cannot_read_or_claim_again() {
    let directory = Directory::new();
    let pending = stage(&directory.0, 1, "session");
    display(&pending, &directory.0);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path(&pending, &directory.0, "bear"))
        .unwrap();
    file.write_all(b"changed bytes").unwrap();
    drop(file);
    assert!(pending.open_review(&directory.0).unwrap().verify().is_err());
    let claimed = pending.claim(&directory.0).unwrap();
    assert!(pending.read_claimed(&claimed).is_err());
    drop(claimed);
    assert!(pending.claim(&directory.0).is_err());
    let cancelled = stage(&directory.0, 1, "session");
    display(&cancelled, &directory.0);
    cancelled.discard(&directory.0).unwrap();
    assert!(!path(&cancelled, &directory.0, "reviewed").exists());
    assert!(cancelled.open_review(&directory.0).is_err());
}

#[test]
fn concurrent_claim_has_exactly_one_winner_and_preserves_other_files() {
    let directory = Directory::new();
    let pending = stage(&directory.0, 1, "session");
    display(&pending, &directory.0);
    let unrelated = FsPath::new(&directory.0.bear_sqlite_data_dir).join("other-bear.sqlite");
    fs::write(&unrelated, b"unrelated private memory").unwrap();
    std::thread::scope(|scope| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let barrier = barrier.clone();
            let pending = &pending;
            let config = &directory.0;
            threads.push(scope.spawn(move || {
                barrier.wait();
                pending.claim(config)
            }));
        }
        let claims: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(claims.iter().filter(|claim| claim.is_ok()).count(), 1);
        for claim in claims.into_iter().flatten() {
            assert!(pending.read_claimed(&claim).is_ok());
        }
    });
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated private memory");
}

#[test]
fn namespace_and_upload_are_bounded() {
    let directory = Directory::new();
    let pending: Vec<_> = (0..namespace::MAX_PENDING)
        .map(|_| stage(&directory.0, 1, "session"))
        .collect();
    assert!(StagedFile::reserve(&directory.0).is_err());
    pending[0].discard(&directory.0).unwrap();
    let (staged, mut file) = StagedFile::reserve(&directory.0).unwrap();
    let mut size = BEAR_BUNDLE_MAX_UPLOAD_BYTES;
    assert!(write_chunk(&mut file, &mut size, b"overflow").is_err());
    drop(file);
    drop(staged);
}
