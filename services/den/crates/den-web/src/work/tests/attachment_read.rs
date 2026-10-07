//! Bear-visible file reads, deliberately separate from the acting human's files.

use super::{seed_member, test_pool, TEST_DB_LOCK};
use den_cabinet::{
    ActorScope, AttachmentRole, CabinetAttachmentRef, CabinetItemRef, CabinetPolicy,
};
use den_core::{
    ids::{BearId, UserId},
    DenError, RuntimeContextLabel,
};
use den_service::{
    artifacts::{
        self,
        bytes::{ArtifactByteReader, ArtifactReadFuture},
        ArtifactContentLocation, ArtifactRef, ArtifactVisibility,
    },
    bears::db as bears_db,
    cabinet::{
        self,
        attachment_read::{self, TextRange},
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

fn scope(bear: Uuid) -> ActorScope {
    ActorScope::bear(BearId::new(bear), RuntimeContextLabel::ChannelConversation)
}

enum Effect {
    None,
    Restrict {
        pool: sqlx::PgPool,
        owner: i32,
        page: CabinetItemRef,
    },
    Detach {
        pool: sqlx::PgPool,
        owner: i32,
        page: CabinetItemRef,
        attachment: CabinetAttachmentRef,
    },
}
struct Reader {
    bytes: Vec<u8>,
    calls: AtomicUsize,
    effect: Effect,
}
impl Reader {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.into(),
            calls: AtomicUsize::new(0),
            effect: Effect::None,
        }
    }
}
impl ArtifactByteReader for Reader {
    fn read<'a>(&'a self, _location: &'a ArtifactContentLocation) -> ArtifactReadFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match &self.effect {
                Effect::None => {}
                Effect::Restrict { pool, owner, page } => cabinet::pages::configure(
                    pool,
                    &ActorScope::user(UserId::new(*owner)),
                    page,
                    CabinetPolicy::default(),
                    &[*owner],
                    &[],
                    &[],
                )
                .await
                .map_err(DenError::from)?,
                Effect::Detach {
                    pool,
                    owner,
                    page,
                    attachment,
                } => cabinet::attachments::unlink(
                    pool,
                    &ActorScope::user(UserId::new(*owner)),
                    page,
                    attachment,
                )
                .await
                .map_err(DenError::from)?,
            }
            Ok(self.bytes.clone())
        })
    }
}

async fn file(
    pool: &sqlx::PgPool,
    user: i32,
    bear: Uuid,
    page: &CabinetItemRef,
    bytes: &[u8],
    content_type: &str,
    shared: bool,
) -> CabinetAttachmentRef {
    let owner = ActorScope::user(UserId::new(user));
    let pending = cabinet::uploads::prepare(
        pool,
        &owner,
        page,
        cabinet::uploads::UploadInput {
            bear_id: BearId::new(bear),
            title: "SHARED TEXT TITLE".into(),
            content_type: content_type.into(),
            bytes,
            role: AttachmentRole::Data,
            audience: if shared {
                cabinet::uploads::UploadAudience::BearAndMembers
            } else {
                cabinet::uploads::UploadAudience::Private
            },
        },
    )
    .await
    .unwrap();
    // A byte-store substitute supplies the exact receipt bytes; no server config.
    artifacts::verify_content_bytes(pending.location(), bytes).unwrap();
    cabinet::uploads::publish(pool, &owner, &pending)
        .await
        .unwrap()
}

#[tokio::test]
async fn bears_discover_only_explicitly_shared_same_bear_files_and_can_read_db_json() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let (other, other_bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "File page", "Source").await;
    let private =
        super::knowledge::document(&pool, bear, owner, ArtifactVisibility::SameUser).await;
    let shared =
        super::knowledge::document(&pool, bear, owner, ArtifactVisibility::BearVisible).await;
    let foreign =
        super::knowledge::document(&pool, other_bear, other, ArtifactVisibility::BearVisible).await;
    let human = ActorScope::user(UserId::new(owner));
    let private_link = cabinet::attachments::link(
        &pool,
        &human,
        &page,
        &ArtifactRef::parse(&private.artifact_ref).unwrap(),
        AttachmentRole::Data,
    )
    .await
    .unwrap();
    let shared_link = cabinet::attachments::link(
        &pool,
        &human,
        &page,
        &ArtifactRef::parse(&shared.artifact_ref).unwrap(),
        AttachmentRole::Data,
    )
    .await
    .unwrap();
    cabinet::attachments::link(
        &pool,
        &ActorScope::user(UserId::new(other)),
        &page,
        &ArtifactRef::parse(&foreign.artifact_ref).unwrap(),
        AttachmentRole::Data,
    )
    .await
    .unwrap();
    let visible = attachment_read::list(&pool, &scope(bear), &page, false)
        .await
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].attachment_ref, shared_link);
    assert!(
        visible[0].text_readable,
        "database JSON needs no object-store config"
    );
    let encoded = serde_json::to_string(&visible).unwrap();
    for hidden in [
        private.artifact_ref.as_str(),
        foreign.artifact_ref.as_str(),
        "storage_key",
        "content_sha256",
        "provenance",
        "metadata",
    ] {
        assert!(!encoded.contains(hidden));
    }
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &private_link,
        TextRange::new(0, 100).unwrap(),
        None
    )
    .await
    .is_err());
    let text = attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &shared_link,
        TextRange::new(0, 100).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(text.text.contains("private document bytes"));
    assert!(text.next_offset_chars.is_none());
    let another_page = super::knowledge::page(&pool, owner, "Other file page", "Source").await;
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &another_page,
        &shared_link,
        TextRange::new(0, 100).unwrap(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn shared_utf8_ranges_are_character_safe_and_all_served_bytes_are_verified() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "UTF8 file page", "Source").await;
    let bytes = "A€🦊Z".as_bytes();
    let attached = file(
        &pool,
        owner,
        bear,
        &page,
        bytes,
        "text/plain; charset=utf-8",
        true,
    )
    .await;
    let reader = Reader::new(bytes);
    let text = attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &attached,
        TextRange::new(1, 2).unwrap(),
        Some(&reader),
    )
    .await
    .unwrap();
    assert_eq!(text.text, "€🦊");
    assert_eq!(text.total_chars, 4);
    assert_eq!(text.next_offset_chars, Some(3));
    let end = attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &attached,
        TextRange::new(4, 1).unwrap(),
        Some(&reader),
    )
    .await
    .unwrap();
    assert_eq!(end.text, "");
    assert_eq!(end.next_offset_chars, None);
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &attached,
        TextRange::new(5, 1).unwrap(),
        Some(&reader)
    )
    .await
    .is_err());
    assert!(TextRange::new(0, 0).is_err());
    assert!(TextRange::new(0, 24001).is_err());
    let corrupt = Reader::new(b"wrong bytes");
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &attached,
        TextRange::new(0, 100).unwrap(),
        Some(&corrupt)
    )
    .await
    .is_err());
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &attached,
        TextRange::new(0, 100).unwrap(),
        None
    )
    .await
    .is_err());
    assert!(
        !attachment_read::list(&pool, &scope(bear), &page, false)
            .await
            .unwrap()[0]
            .text_readable
    );
}

#[tokio::test]
async fn binary_and_unsupported_encodings_never_reach_the_text_reader() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, bear, _, _) = seed_member(&pool).await;
    let page = super::knowledge::page(&pool, owner, "Unsupported files", "Source").await;
    let reader = Reader::new(b"fixture bytes");
    for content_type in [
        "application/pdf",
        "image/png",
        "application/octet-stream",
        "text/plain; charset=utf-16",
    ] {
        let attached = file(
            &pool,
            owner,
            bear,
            &page,
            b"fixture bytes",
            content_type,
            true,
        )
        .await;
        assert!(attachment_read::read_text(
            &pool,
            &scope(bear),
            &page,
            &attached,
            TextRange::new(0, 100).unwrap(),
            Some(&reader)
        )
        .await
        .is_err());
    }
    assert_eq!(reader.calls.load(Ordering::Relaxed), 0);
    assert!(attachment_read::list(&pool, &scope(bear), &page, true)
        .await
        .unwrap()
        .iter()
        .all(|entry| !entry.text_readable));
    let invalid = file(&pool, owner, bear, &page, b"\xff\xfe", "text/plain", true).await;
    assert!(attachment_read::read_text(
        &pool,
        &scope(bear),
        &page,
        &invalid,
        TextRange::new(0, 100).unwrap(),
        Some(&Reader::new(b"\xff\xfe"))
    )
    .await
    .is_err());
}

#[tokio::test]
async fn page_revocation_or_detach_during_io_prevents_returning_file_text() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    for detach in [false, true] {
        let (owner, bear, _, _) = seed_member(&pool).await;
        let page = super::knowledge::page(&pool, owner, "Revocable file", "Source").await;
        let attached = file(
            &pool,
            owner,
            bear,
            &page,
            b"secret bytes",
            "text/plain",
            true,
        )
        .await;
        let effect = if detach {
            Effect::Detach {
                pool: pool.clone(),
                owner,
                page: page.clone(),
                attachment: attached.clone(),
            }
        } else {
            Effect::Restrict {
                pool: pool.clone(),
                owner,
                page: page.clone(),
            }
        };
        let reader = Reader {
            effect,
            ..Reader::new(b"secret bytes")
        };
        assert!(attachment_read::read_text(
            &pool,
            &scope(bear),
            &page,
            &attached,
            TextRange::new(0, 100).unwrap(),
            Some(&reader)
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn disabled_bears_cannot_borrow_attachment_access_from_the_human_creator() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner, _, _, _) = seed_member(&pool).await;
    let disabled = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name, cabinet_enabled) VALUES ($1, $2, $3) RETURNING id",
        format!("disabled-file-{}", Uuid::new_v4().simple()),
        "Disabled file Bear",
        false
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    bears_db::grant_membership(&pool, owner, disabled, Some("admin"))
        .await
        .unwrap();
    let page = super::knowledge::page(&pool, owner, "Disabled file access", "Source").await;
    let attached = file(
        &pool,
        owner,
        disabled,
        &page,
        b"shared bytes",
        "text/plain",
        true,
    )
    .await;
    let reader = Reader::new(b"shared bytes");
    assert!(attachment_read::list(&pool, &scope(disabled), &page, true)
        .await
        .is_err());
    assert!(attachment_read::read_text(
        &pool,
        &scope(disabled),
        &page,
        &attached,
        TextRange::new(0, 100).unwrap(),
        Some(&reader)
    )
    .await
    .is_err());
    assert_eq!(reader.calls.load(Ordering::Relaxed), 0);
}
