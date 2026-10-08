use super::*;
use minijinja::context;
use serde_json::json;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

#[tokio::test]
async fn failed_rollback_preserves_surviving_sqlite_files_and_pool() {
    let directory = std::env::temp_dir().join(format!("den-import-rollback-{}", Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    let sqlite = directory.join("surviving.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        fs::write(
            format!("{}{suffix}", sqlite.display()),
            b"private surviving memory",
        )
        .unwrap();
    }
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let id = Uuid::new_v4();
    let outcome = compensate(
        id,
        "retained-import".into(),
        CreationStage::Initialization,
        async { Err(DenError::Database("private-db-cause-never-display".into())) },
        || async {
            cleanup_calls.fetch_add(1, Ordering::SeqCst);
            pool.close().await;
            for suffix in ["", "-wal", "-shm"] {
                fs::remove_file(format!("{}{suffix}", sqlite.display())).unwrap();
            }
            Ok(())
        },
    )
    .await;
    assert!(
        matches!(outcome, CreationFailure::Retained { bear_id, stage: CreationStage::Initialization, .. } if bear_id == BearId::new(id))
    );
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
    assert!(!pool.is_closed());
    for suffix in ["", "-wal", "-shm"] {
        assert_eq!(
            fs::read(format!("{}{suffix}", sqlite.display())).unwrap(),
            b"private surviving memory"
        );
    }
    assert!(!serde_json::to_string(&outcome)
        .unwrap()
        .contains("private-db-cause-never-display"));
    pool.close().await;
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn confirmed_deletion_is_required_before_cleanup_and_cleanup_failure_is_visible() {
    let calls = AtomicUsize::new(0);
    let removed = compensate(
        Uuid::new_v4(),
        "rolled-back".into(),
        CreationStage::Memory,
        async { Ok(()) },
        || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert!(matches!(
        removed,
        CreationFailure::RolledBack {
            storage_cleanup_pending: false,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let incomplete_cleanup = compensate(
        Uuid::new_v4(),
        "orphan-files".into(),
        CreationStage::Initialization,
        async { Ok(()) },
        || async { Err(CustomError::System("private-storage-cause".into())) },
    )
    .await;
    assert!(matches!(
        incomplete_cleanup,
        CreationFailure::RolledBack {
            storage_cleanup_pending: true,
            ..
        }
    ));
    let encoded = serde_json::to_string(&incomplete_cleanup).unwrap();
    assert!(encoded.contains("Private memory cleanup still needs a Den operator"));
    assert!(!encoded.contains("private-storage-cause"));
}

fn render_failure(failure: &CreationFailure) -> String {
    let mut environment = minijinja::Environment::new();
    environment
        .add_template("base.html", "{% block content %}{% endblock %}")
        .unwrap();
    environment
        .add_template(
            "review",
            include_str!("../../../templates/bear/manage/import_review.html"),
        )
        .unwrap();
    environment.get_template("review").unwrap().render(context! {
        manifest => json!({"bear": {"name": "Imported", "slug": "imported", "birthdate": "2020-01-01", "description": "Purpose"}, "prompts": {"system_prompt": "Steering"}, "model_configurations": [], "hats": [], "skills": []}),
        creation_failure => failure, creation_stage => failure.stage().label(),
        consumed => true, can_confirm => false, compatibility_checked => false,
        compatibility_errors => Vec::<String>::new(), legacy_profiles => "{}",
    }).unwrap()
}

#[tokio::test]
async fn consumed_failure_page_distinguishes_retained_rollback_and_unconfirmed_outcomes() {
    let retained = compensate(
        Uuid::new_v4(),
        "retained-handle".into(),
        CreationStage::Initialization,
        async { Err(DenError::Database("never-render-this-db-cause".into())) },
        || async { Ok(()) },
    )
    .await;
    let page = render_failure(&retained);
    assert!(page.contains("An incomplete Bear remains"));
    assert!(page.contains("retained-handle"));
    assert!(page.contains("Initialization"));
    assert!(page.contains("SQLite files were preserved"));
    assert!(!page.contains("No Bear has been created"));
    assert!(!page.contains("never-render-this-db-cause"));
    assert!(!page.contains("action=\"/bears/import/"));
    assert!(page.contains("href=\"/\""));
    let rolled_back = compensate(
        Uuid::new_v4(),
        "removed".into(),
        CreationStage::Birthday,
        async { Ok(()) },
        || async { Ok(()) },
    )
    .await;
    let page = render_failure(&rolled_back);
    assert!(page.contains("The incomplete Bear record was removed"));
    assert!(page.contains("fresh review and confirmation"));
    assert!(!page.contains("An incomplete Bear remains"));
    let page = render_failure(&CreationFailure::unconfirmed(CreationStage::Creation));
    assert!(page.contains("creation outcome is not confirmed"));
    assert!(!page.contains("No Bear has been created"));
}
