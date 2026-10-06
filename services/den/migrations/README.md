### Migrations

#### Do not edit applied migration files

SQLx records a **checksum per version** in `public._sqlx_migrations`. At startup, [`run_sqlx_migrations`](../src/startup.rs) runs embedded migrations and **verifies** that each already-applied file still matches that checksum.

- **Never** change the contents of an existing `services/den/migrations/*_up.sql` that has been applied to **any** environment (including comments-only edits). That breaks checksum verification and can prevent Den from starting.
- **Do** add a **new** migration file with the next version timestamp (`sqlx migrate add …` from `services/den/`) for any schema change, correction, or column drop.

If you need different wording in old migrations, document it in this README or in planning docs—**not** by editing the SQL file.

#### Reversible migration policy

Production deploys now run Den schema changes in a dedicated one-off migration job before the long-running `bears-den` service starts. To keep deploys diagnosable and rollback-friendly:

- Every new `*.up.sql` schema migration must include a matching `*.down.sql` unless the change is provably irreversible and explicitly documented in the migration review.
- Prefer **expand-contract** changes so both the currently running Den and the newly deployed Den can tolerate the schema during rollout.
- Treat destructive schema changes (`DROP COLUMN`, `DROP TABLE`, tightening `NOT NULL`, incompatible enum/value rewrites, semantic renames) as a second-step contract migration after the compatible application code is already deployed.
- Keep migrations short, transactional where possible, and separate long backfills from the schema step.

Reversible does **not** mean operators should blindly run all downs in production. It means each migration must have an intentional rollback story that can be rehearsed and reasoned about.

#### Cabinet upload cleanup acknowledgement

`20261006102750_add_artifact_content_removal_tracking` is an additive migration: it adds nullable `artifacts.content_removed_at` (existing/new rows default to NULL), restricts non-NULL acknowledgements to `deleted`/`expired` lifecycle states, and adds a partial queue index for registered Cabinet file uploads. It does not expire files, delete payloads or rewrite existing artifact data during migration.

Validation rehearses up → down → up on a populated isolated database, checks the valid partial index and NULL default, and confirms PostgreSQL rejects acknowledgement of pending/finalized content with the named check constraint. Recovery tests cover terminal acknowledgements, retention and retry.

Rollback drops the index, check and new acknowledgement column, so it loses removal acknowledgements but preserves the pre-existing artifact fields/audit rows. Reapplying starts those acknowledgements at NULL; already-retired keys can be deleted again idempotently, but removed content is not restored. Stop the new cleanup worker/use the matching older binary before dropping the column. Do not restart an older binary after applying this migration while its embedded migrator is behind the database.

#### Existing client-session ID collisions

`20260928101801_guard_client_session_id_ownership` adds a database trigger that serializes claims for the same opaque client-session ID and rejects any new cross-user or cross-Bear reuse. It deliberately does **not** delete, rename, or automatically deduplicate older bindings: event and Work records still refer to those IDs as text, and their ownership cannot be reconstructed by renaming only `client_sessions`. The down migration removes the guard without modifying historical data. BearWire preflight continues to fail closed on ambiguous IDs. Review a historical collision with its event/run provenance before a separate, explicit repair; never promote a NULL-owner conversation from a client-session match alone.

#### Startup schema compatibility guard

Den now checks the highest successful version recorded in `public._sqlx_migrations` before serving traffic. If the database has already been migrated to a version newer than the binary's embedded SQLx migrator, startup fails with a clear version-mismatch error instead of running unpredictably against an unknown schema.

This protects against old binaries being restarted after a newer deploy already advanced the database.

#### Repairing checksum mismatch (after a mistaken edit)

1. **Revert** the migration file in git to the canonical version (e.g. match `main` or the last known-good commit).
2. From `services/den/`, run **`sqlx migrate info`** (with `DATABASE_URL` pointing at the affected database). If a version shows **`(different checksum)`**, the database still stores the checksum from the wrong file.
3. **Align the database** with the reverted file: update `public._sqlx_migrations` for that version so `checksum` equals the **local** checksum reported by `sqlx migrate info` (the value after “local migration has checksum …”). Alternatively, restore the on-disk file to match the checksum that was actually applied (worse—keeps the mistake in git).

Only the migration **checksum** row may need fixing if the executed SQL was identical (e.g. comment-only change). If you changed executable SQL and it already ran in production, you need a **new** migration to fix the schema, not a rewrite of the old file.

---

At deploy time, the compose stack runs `den migrate` in a one-off container built from the same image as `bears-den`. The long-running service then starts with `den serve`, verifies the database schema version is not newer than the binary, and skips reapplying startup migrations. For local authoring you still use **`sqlx migrate run`** / **`sqlx migrate add`** from the `services/den/` directory when developing locally.

| File | Purpose |
|------|---------|
| [`20250309000000_trestle.up.sql`](20250309000000_trestle.up.sql) | Starter: `users`, invites, email, OAuth tables |
| [`20250331120000_phase1_den_registry.up.sql`](20250331120000_phase1_den_registry.up.sql) | **Phase 1 M1**: `bears`, `user_bear`, `audit_chat`; `users.webui_account_id` + index; `users.is_admin` |
| [`20250401120000_phase1_bear_provisioning_fields.up.sql`](20250401120000_phase1_bear_provisioning_fields.up.sql) | **Historical Phase 1 M1b**: `bears.system_prompt`; introduced now-retired nullable `bears.runtime_agent_id` |
| [`20250401130000_phase1_bootstrap_admin.up.sql`](20250401130000_phase1_bootstrap_admin.up.sql) | **Bootstrap operator** (only if no `username = 'admin'` yet): see **Default operator account** below |
| [`20250413120000_bear_letta_sync_fields.up.sql`](20250413120000_bear_letta_sync_fields.up.sql) | `bears.letta_agent_type`, `bears.letta_tool_ids` |
| [`20260416120000_bear_chat_activity.up.sql`](20260416120000_bear_chat_activity.up.sql) | `bear_chat_activity` (later dropped) |
| [`20260416120100_drop_bear_chat_activity.up.sql`](20260416120100_drop_bear_chat_activity.up.sql) | Drop `bear_chat_activity` |
| [`20260418130000_drop_users_webui_account_id.up.sql`](20260418130000_drop_users_webui_account_id.up.sql) | Drop `users.webui_account_id` + index |
| [`20260429120000_acp_tokens.up.sql`](20260429120000_acp_tokens.up.sql) | ACP code tokens and scopes |
| [`20260430120000_acp_client_tool_calls.up.sql`](20260430120000_acp_client_tool_calls.up.sql) | Legacy persisted ACP client tool relay calls; removed from the active architecture |
| [`20260430121000_acp_sessions.up.sql`](20260430121000_acp_sessions.up.sql) | ACP session bindings (historical column name `codepool_session_id`; renamed later) |
| [`20260501120000_archived_conversations.up.sql`](20260501120000_archived_conversations.up.sql) | Archived conversation tracking |
| [`20260501121000_drop_users_admin_flag.up.sql`](20260501121000_drop_users_admin_flag.up.sql) | Backfill canonical `users.is_admin` and drop legacy `users.admin_flag` |
| [`20260502120000_drop_acp_client_tool_calls.up.sql`](20260502120000_drop_acp_client_tool_calls.up.sql) | Drop obsolete `acp_client_tool_calls`; ACP client-tool relay through Codepool was removed |
| [`20260503120000_multi_agent_bears.up.sql`](20260503120000_multi_agent_bears.up.sql) | Multi-agent Bear registry (`bear_agents`), skill manifest/proposals, watch subscriptions, and one-time import from retired single-agent bear ids into `bear_agents(role='talk')` |
| [`20260503130000_acp_runtime_session_id.up.sql`](20260503130000_acp_runtime_session_id.up.sql) | Rename historical ACP `codepool_session_id` bindings to runtime-neutral `runtime_session_id` |
| [`20260503140000_drop_bears_runtime_agent_id.up.sql`](20260503140000_drop_bears_runtime_agent_id.up.sql) | Drop retired `bears.runtime_agent_id`; runtime Runtime ids live exclusively in `bear_agents(role, runtime_agent_id)` |

### Default operator account

After migrations have been applied (first container start or local `cargo run` on an empty DB), you can sign in at `/login` with:

| Field | Value |
|-------|-------|
| Username | `admin` |
| Password | `Never deploy with default passwords.` |
| Email (stored) | `admin@localhost` |

**Production:** Change this password immediately (or remove the user and create operators another way). The password is documented here and in the migration file on purpose for local bootstrap only.

The stored `passhash` is Argon2id (PHC). If you change the password string in the migration, regenerate the hash with `password_auth::generate_hash` from the same `password-auth` version as in `Cargo.toml`, and update [`tests/bootstrap_admin_passhash.rs`](../tests/bootstrap_admin_passhash.rs).

**Note:** Legacy `users.id` is still `serial`. `user_bear.user_id` is `INTEGER` FK to `users(id)` so the schema is consistent without a UUID cutover. A later milestone may migrate identity to UUID per [PHASE1_BOOTSTRAP.md](../../docs/planning/PHASE1_BOOTSTRAP.md). Column **`is_admin`** is the canonical operator flag; legacy **`admin_flag`** is backfilled into it and dropped by the 20260501121000 migration.

Production compose deploys should use the one-off migration job before switching `bears-den`. Keep normal application startups on `den serve` so migration failures happen before the app container is swapped into service.
