use super::*;

#[test]
fn cabinet_startup_requires_only_database_and_dedicated_origin() {
    let mut config = Config::test_stub();
    config.cabinet_web_origin =
        crate::config::cabinet::parse_optional_origin(Some("https://cabinet.example")).unwrap();
    // Den's flags and upstream/runtime settings must not select Cabinet capabilities.
    config.run_api = true;
    config.run_workers = true;
    config.run_sandbox = true;
    config.llm_api_url.clear();
    config.bifrost_base_url.clear();
    config.bear_sqlite_data_dir = "/not-mounted/native-memory".into();
    validate_cabinet_config(&config).unwrap();
    config.database_url.clear();
    assert!(validate_cabinet_config(&config).is_err());
    config.database_url = "postgres://localhost/shared".into();
    config.cabinet_web_origin = None;
    assert!(validate_cabinet_config(&config).is_err());
}

#[test]
fn cabinet_startup_revalidates_programmatically_constructed_origins() {
    let mut config = Config::test_stub();
    config.cabinet_web_origin = Some(url::Url::parse("https://cabinet.example/path").unwrap());
    assert!(validate_cabinet_config(&config).is_err());
}

fn applied_migrations(migrator: &Migrator) -> Vec<AppliedMigration> {
    migrator
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| AppliedMigration {
            version: migration.version,
            checksum: migration.checksum.clone(),
        })
        .collect()
}

#[test]
fn cabinet_schema_admission_rejects_pending_checksum_drift_and_unknown_migrations() {
    let migrator = sqlx::migrate!();
    let mut applied = applied_migrations(&migrator);
    validate_cabinet_migrations(&migrator, &applied).unwrap();
    let missing = applied.pop().unwrap();
    let error = validate_cabinet_migrations(&migrator, &applied).unwrap_err();
    assert!(error.to_string().contains(&missing.version.to_string()));
    assert!(error.to_string().contains("den migrate"));
    applied.push(missing);

    let first_version = applied[0].version;
    applied[0].checksum = vec![0; 48].into();
    assert!(matches!(
        validate_cabinet_migrations(&migrator, &applied),
        Err(StartupError::Migrate(sqlx::migrate::MigrateError::VersionMismatch(version))) if version == first_version
    ));
    let mut applied = applied_migrations(&migrator);
    let unknown_version = first_version - 1;
    applied.push(AppliedMigration {
        version: unknown_version,
        checksum: vec![0; 48].into(),
    });
    assert!(matches!(
        validate_cabinet_migrations(&migrator, &applied),
        Err(StartupError::Migrate(sqlx::migrate::MigrateError::VersionMissing(version))) if version == unknown_version
    ));
}

// SQLx creates a fresh, independent test database; no shared application schema
// is dropped or rewritten. This exercises exactly the migration job's helper.
#[sqlx::test(migrations = false)]
async fn cabinet_schema_admission_is_read_only_and_migration_job_owns_sessions(pool: sqlx::PgPool) {
    assert!(ensure_cabinet_schema_ready(&pool).await.is_err());
    let mut connection = pool.acquire().await.unwrap();
    assert!(
        connection.list_applied_migrations().await.is_err(),
        "Cabinet must not create SQLx bookkeeping"
    );
    drop(connection);

    crate::startup::run_sqlx_migrations(&pool).await.unwrap();
    let error = ensure_cabinet_schema_ready(&pool).await.unwrap_err();
    assert!(matches!(error, StartupError::SessionStore(_)));
    assert!(error.to_string().contains("den migrate"));
    assert!(
        PostgresStore::new(pool.clone())
            .load(&Id::default())
            .await
            .is_err(),
        "Cabinet must not create the session table"
    );

    crate::migrate_database_schema(&pool).await.unwrap();
    // Repeated migration jobs remain safe after the session schema exists.
    crate::migrate_database_schema(&pool).await.unwrap();
    let read_only_options = (*pool.connect_options())
        .clone()
        .options([("default_transaction_read_only", "on")]);
    let read_only_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(read_only_options)
        .await
        .unwrap();
    ensure_cabinet_schema_ready(&read_only_pool).await.unwrap();
    let record = axum_login::tower_sessions::session::Record {
        id: Id::default(),
        data: Default::default(),
        expiry_date: time::OffsetDateTime::now_utc() + time::Duration::days(1),
    };
    assert!(
        PostgresStore::new(read_only_pool.clone())
            .save(&record)
            .await
            .is_err(),
        "the admission pool must actually reject writes"
    );
    read_only_pool.close().await;
}
