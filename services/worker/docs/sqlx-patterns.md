# SQLx Query Patterns and Preparation

This document explains the correct patterns for using SQLx in this project, including query macros, preparation workflow, and common pitfalls.

## Overview

This starter uses **SQLx** for all database operations with compile-time query validation. This provides:
- Type safety at compile time
- SQL syntax validation
- Protection against SQL injection
- Offline compilation support (via `.sqlx/` metadata)

## Query Macros

### 1. `sqlx::query!` - Simple Queries

**Use when**: You need to execute a query but don't need to map results to a struct.

**Pattern**:
```rust
let result = sqlx::query!(
    r#"
    INSERT INTO tracking_email (email_message_id, user_id, event_type, logged_at)
    SELECT $1, ec.user_id, 'load', NOW()
    FROM email_messages em
    INNER JOIN email_configs ec ON em.email_config_id = ec.id
    WHERE em.id = $1
    ON CONFLICT (email_message_id, event_type) DO NOTHING
    "#,
    message_id
)
.execute(&pool)
.await?;
```

**Key points**:
- Use `r#"..."#` raw string literals for multi-line SQL
- Parameters use `$1`, `$2`, etc. (PostgreSQL style)
- Returns `QueryResult` which you can call `.execute()`, `.fetch_one()`, `.fetch_all()`, etc.
- Use `.execute()` for INSERT/UPDATE/DELETE
- Use `.fetch_one()` for single row results
- Use `.fetch_all()` for multiple rows

### 2. `sqlx::query_as!` - Typed Results

**Use when**: You need to map query results to a Rust struct.

**Pattern**:
```rust
#[derive(Debug, sqlx::FromRow)]
struct EntityTypeStatsRaw {
    entity_type: String,
    total_count: i64,
    last_7_days_count: i64,
}

let results = sqlx::query_as!(
    EntityTypeStatsRaw,
    r#"
    SELECT
        entity_type,
        COUNT(*) as "total_count!",
        COUNT(*) FILTER (WHERE logged_at >= NOW() - INTERVAL '7 days') as "last_7_days_count!"
    FROM tracking_web
    GROUP BY entity_type
    ORDER BY COUNT(*) DESC
    "#
)
.fetch_all(pool)
.await?;
```

**Key points**:
- First parameter is the struct type
- Struct must derive `FromRow` (or implement it manually)
- Column names must match struct field names (case-sensitive)
- Use `"field_name!"` syntax in SQL to mark fields as NOT NULL
- Use `"field_name?"` syntax for Option types (though this is less common)

**Column name matching**:
- SQL column `entity_type` → Rust field `entity_type`
- SQL alias `total_count` → Rust field `total_count`
- Use `as "field_name"` in SQL if names don't match

### 3. `sqlx::query_scalar!` - Single Value Results

**Use when**: You only need a single value from the query (e.g., COUNT, MAX, etc.).

**Pattern**:
```rust
let total_count: i64 = sqlx::query_scalar!(
    r#"
    SELECT COUNT(*) FROM users WHERE active = true
    "#
)
.fetch_one(pool)
.await?;
```

**Key points**:
- Returns a single value of the specified type
- Use for aggregate functions (COUNT, SUM, MAX, MIN, etc.)
- Type annotation is required: `let result: Type = ...`

### 4. `QueryBuilder` - Dynamic Queries

**Use when**: You need to build queries dynamically (conditional WHERE clauses, variable number of parameters).

**Pattern**:
```rust
use sqlx::{QueryBuilder, Postgres};

let mut query_builder = QueryBuilder::<Postgres>::new(
    "SELECT COUNT(*) FROM users WHERE (username ILIKE ",
);
query_builder.push_bind(&pattern);
query_builder.push(" OR display_name ILIKE ");
query_builder.push_bind(&pattern);
query_builder.push(")");

// Example optional clause:
// if filter_admin {
//     query_builder.push(" AND admin_flag = true");
// }

let total_count: i64 = query_builder.build_query_scalar().fetch_one(pool).await?;
```

**Key points**:
- Use for complex conditional queries
- `.push()` adds SQL fragments
- `.push_bind()` adds parameterized values
- `.build_query_scalar()` for single values
- `.build_query_as::<Struct>()` for typed results
- `.build()` for untyped queries

## SQLx Preparation Workflow

### Critical: Always Run `cargo sqlx prepare`

**When to run**:
1. After creating or modifying database migrations
2. After changing any SQL query in your code
3. After modifying database schema
4. Before creating a production build

**Command**:
```bash
# From project root
cargo sqlx prepare
```

**What it does**:
1. Connects to your database (via `DATABASE_URL`)
2. Validates all SQL queries against the current schema
3. Generates `.sqlx/` directory with query metadata
4. Enables offline compilation (no database needed at build time)

**Important**:
- Must be run from project root
- Requires `DATABASE_URL` environment variable
- Database must be migrated to latest schema
- Validates both SQL syntax AND Rust compilation

### Development Workflow

**Typical workflow**:
```bash
# 1. Make code changes (especially SQL queries)
# 2. Ensure database is up-to-date
sqlx migrate run

# 3. Prepare SQLx queries
cargo sqlx prepare

# 4. Continue development
cargo run
```

**After schema changes**:
```bash
# 1. Create migration
sqlx migrate add description_of_change

# 2. Edit migration files in migrations/
# 3. Apply migration
sqlx migrate run

# 4. CRITICAL: Prepare SQLx queries
cargo sqlx prepare

# 5. Test your changes
cargo check
cargo run
```

### Production Builds

**Before production build**:
```bash
# 1. Ensure all migrations are applied
# 2. Run SQLx preparation
cargo sqlx prepare

# 3. Build with production features
cargo build --release --features production
```

**Why**: Production builds use the `.sqlx/` metadata directory for offline compilation. Without running `cargo sqlx prepare`, the metadata will be stale or missing, causing build failures.

## Database migration safety

Schema changes can cause data loss or downtime. Treat migrations as production-critical.

### Core practices

1. **Destructive operations** — Do not run `DROP TABLE`, `DROP DATABASE`, bulk `DELETE`, or destructive `DOWN` migrations on shared or production databases without an explicit plan, backups, and approval.
2. **Follow existing conventions** — Use the same layout and naming as files under [`migrations/`](../migrations/); read [`migrations/README.md`](../migrations/README.md) if present.
3. **Review before apply** — Check SQL, types, constraints, indexes, and interaction with existing SQLx queries.
4. **Naming** — Timestamp-prefixed files, e.g. `YYYYMMDDHHMMSS_description.up.sql`; add matching `.down.sql` only when you need a reversible migration.
5. **SQLx** — Migrations are tracked (e.g. `_sqlx_migrations`). After schema changes, apply migrations, then run `cargo sqlx prepare` so `.sqlx/` matches the schema (see above).
6. **Dependencies** — New columns/tables must not break existing queries until code and `.sqlx/` are updated.
7. **Production** — Prefer additive, backward-compatible steps; plan backups before major changes.

### Pitfalls

- Down migrations must reference the same column/table names as the up migration.
- Use `IF EXISTS` / `DROP … IF EXISTS` in down migrations where appropriate.
- Avoid duplicate index names; order drops to respect foreign keys.
- Type changes and casts can lose data — verify on a copy of real data when possible.

## Common Patterns

### Pattern 1: Insert with RETURNING

```rust
let result = sqlx::query!(
    r#"
    INSERT INTO email_messages (email_config_id, message_type, parameters)
    VALUES ($1, $2, $3)
    RETURNING id
    "#,
    config.email_config_id,
    type_name,
    serde_json::to_value(ctx.clone())?
)
.fetch_one(sqlx_pool)
.await?;

let message_id = result.id;
```

### Pattern 2: Conditional WHERE Clauses

```rust
// Use QueryBuilder for dynamic WHERE clauses
let mut query_builder = QueryBuilder::<Postgres>::new(
    "SELECT * FROM users WHERE 1=1"
);

if let Some(username) = username_filter {
    query_builder.push(" AND username = ");
    query_builder.push_bind(username);
}

if let Some(active) = active_filter {
    query_builder.push(" AND active = ");
    query_builder.push_bind(active);
}
```

### Pattern 3: Array Parameters

```rust
let results = sqlx::query!(
    r#"
    SELECT * FROM areas
    WHERE id = ANY($1)
    "#,
    &area_ids as &[i32]  // PostgreSQL array parameter
)
.fetch_all(pool)
.await?;
```

### Pattern 4: JSONB Parameters

```rust
let result = sqlx::query!(
    r#"
    INSERT INTO email_messages (email_config_id, message_type, parameters)
    VALUES ($1, $2, $3)
    "#,
    config.email_config_id,
    type_name,
    serde_json::to_value(ctx)?  // Convert to serde_json::Value
)
.execute(sqlx_pool)
.await?;
```

### Pattern 5: Transactions

```rust
let mut tx = pool.begin().await?;

sqlx::query!("INSERT INTO table1 ...")
    .execute(&mut *tx)
    .await?;

sqlx::query!("INSERT INTO table2 ...")
    .execute(&mut *tx)
    .await?;

tx.commit().await?;
```

## Error Handling

### Common SQLx Errors

**1. Column type mismatch**:
```
Error: column "field_name" is of type integer but query expects text
```
**Fix**: Check your struct field types match the database column types.

**2. Missing column**:
```
Error: column "field_name" does not exist
```
**Fix**: Ensure column name matches exactly (case-sensitive).

**3. Stale metadata**:
```
Error: query metadata is stale
```
**Fix**: Run `cargo sqlx prepare` after schema changes.

**4. Parameter count mismatch**:
```
Error: expected 2 parameters, got 1
```
**Fix**: Check that all `$1`, `$2`, etc. in SQL have corresponding arguments.

## Best Practices

### 1. Always Use Parameterized Queries

**❌ WRONG** - SQL injection risk:
```rust
let query = format!("SELECT * FROM users WHERE username = '{}'", username);
```

**✅ CORRECT** - Parameterized:
```rust
sqlx::query!("SELECT * FROM users WHERE username = $1", username)
```

### 2. Use Raw String Literals for Multi-line SQL

**✅ CORRECT**:
```rust
sqlx::query!(
    r#"
    SELECT
        id,
        username,
        email
    FROM users
    WHERE active = true
    "#
)
```

### 3. Mark NOT NULL Fields Explicitly

**✅ CORRECT**:
```rust
sqlx::query_as!(
    MyStruct,
    r#"
    SELECT
        id as "id!",
        username as "username!",
        email as "email?"  -- Optional field
    FROM users
    "#
)
```

### 4. Validate Queries Before Committing

**Workflow**:
1. Write your query
2. Run `cargo sqlx prepare` to validate
3. Fix any errors
4. Test with `cargo check`
5. Commit your changes

### 5. Keep Queries Close to Usage

**✅ CORRECT** - Query in the function that uses it:
```rust
async fn get_user_by_id(pool: &PgPool, user_id: i32) -> Result<User, CustomError> {
    let user = sqlx::query_as!(
        User,
        "SELECT * FROM users WHERE id = $1",
        user_id
    )
    .fetch_one(pool)
    .await?;
    Ok(user)
}
```

## Troubleshooting

### Issue: `cargo sqlx prepare` fails

**Possible causes**:
1. Database not running
2. `DATABASE_URL` not set or incorrect
3. Database schema not migrated
4. SQL syntax errors in queries

**Solution**:
```bash
# Check database connection
psql $DATABASE_URL

# Ensure migrations are applied
sqlx migrate run

# Try prepare again
cargo sqlx prepare
```

### Issue: Build fails with "query metadata is stale"

**Solution**:
```bash
# Regenerate metadata
cargo sqlx prepare

# Clean and rebuild
cargo clean
cargo build
```

### Issue: Type mismatch errors

**Solution**:
1. Check database column types: `\d table_name` in psql
2. Verify struct field types match
3. Use `as "field_name!"` syntax for NOT NULL fields
4. Run `cargo sqlx prepare` to validate

## Related Documentation

- [`quickstart.md`](quickstart.md) — local `DATABASE_URL`, migrations, and running the app before `cargo sqlx prepare`
- [infrastructure-and-ops.md](infrastructure-and-ops.md) — processes, `DATABASE_URL`, deployments
- Migration safety — this document (section above)

