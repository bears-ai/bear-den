//! Read-only schema gate for untrusted bundle SQLite, before the store manager opens it.

use std::path::Path;

use den_core::DenError;
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};

use crate::{migrate::MEMORY_LINKS_VIEW_SQL, personal_facts::GENERIC_MEMORY_RECORDS_VIEW_SQL};

/// Reject executable imported schema before initialization, migration, or quarantine writes.
///
/// Missing newer tables are allowed: the store manager adds them after this gate succeeds.
/// This validates schema safety, not the contents or completeness of a memory database.
pub async fn validate_imported_memory_schema(path: &Path) -> Result<(), DenError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .pragma("trusted_schema", "OFF");
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(schema_error)?;
    let result = inspect_schema(&mut connection).await;
    let closed = connection.close().await.map_err(schema_error);
    result.and(closed)
}

async fn inspect_schema(connection: &mut SqliteConnection) -> Result<(), DenError> {
    let trigger = sqlx::query_scalar!(
        r#"SELECT name AS "name!: String" FROM sqlite_schema WHERE type = 'trigger' LIMIT 1"#,
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(schema_error)?;
    if let Some(name) = trigger {
        return Err(DenError::ValidationError(format!(
            "imported memory schema contains a trigger: {name}"
        )));
    }

    let views = sqlx::query!(
        r#"
        SELECT name AS "name!: String", sql AS "definition!: String"
        FROM sqlite_schema WHERE type = 'view'
        "#,
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(schema_error)?;
    // SQLite omits IF NOT EXISTS from stored CREATE VIEW statements.
    for view in views {
        let expected = match view.name.as_str() {
            "memory_links" => Some(MEMORY_LINKS_VIEW_SQL),
            "generic_memory_records" => Some(GENERIC_MEMORY_RECORDS_VIEW_SQL),
            _ => None,
        };
        let expected = expected.map(|sql| {
            normalized_view_sql(&sql.replacen("CREATE VIEW IF NOT EXISTS", "CREATE VIEW", 1))
        });
        if expected.as_deref() != Some(normalized_view_sql(&view.definition).as_str()) {
            return Err(DenError::ValidationError(format!(
                "imported memory schema contains a noncanonical view: {}",
                view.name
            )));
        }
    }

    let virtual_table = sqlx::query_scalar!(
        r#"
        SELECT CAST(name AS TEXT) AS "name!: String" FROM pragma_table_list
        WHERE "schema" = 'main' AND (
            type NOT IN ('table', 'view')
            OR (name = 'generic_memory_records' COLLATE NOCASE AND type <> 'view')
        )
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(schema_error)?;
    if let Some(name) = virtual_table {
        return Err(DenError::ValidationError(format!(
            "imported memory schema contains a virtual, shadow, or reserved table: {name}"
        )));
    }
    // Canonical memory tables have no foreign keys. Cascades are executable schema too:
    // a legacy-table migration must not silently delete protected records' access rules.
    let foreign_key_table = sqlx::query_scalar!(
        r#"
        SELECT CAST(s.name AS TEXT) AS "name!: String"
        FROM sqlite_schema AS s, pragma_foreign_key_list(s.name) AS fk
        WHERE s.type = 'table'
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(schema_error)?;
    if let Some(name) = foreign_key_table {
        return Err(DenError::ValidationError(format!(
            "imported memory schema contains a foreign key: {name}"
        )));
    }
    Ok(())
}

fn normalized_view_sql(sql: &str) -> String {
    sql.trim()
        .trim_end_matches(';')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn schema_error(error: sqlx::Error) -> DenError {
    DenError::ValidationError(format!(
        "cannot inspect imported memory SQLite schema: {error}"
    ))
}

#[cfg(test)]
mod tests;
