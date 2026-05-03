### Migrations

#### Do not edit applied migration files

SQLx records a **checksum per version** in `public._sqlx_migrations`. At startup, [`run_sqlx_migrations`](../src/startup.rs) runs embedded migrations and **verifies** that each already-applied file still matches that checksum.

- **Never** change the contents of an existing `services/den/migrations/*_up.sql` that has been applied to **any** environment (including comments-only edits). That breaks checksum verification and can prevent Den from starting.
- **Do** add a **new** migration file with the next version timestamp (`sqlx migrate add …` from `services/den/`) for any schema change, correction, or column drop.

If you need different wording in old migrations, document it in this README or in planning docs—**not** by editing the SQL file.

#### Repairing checksum mismatch (after a mistaken edit)

1. **Revert** the migration file in git to the canonical version (e.g. match `main` or the last known-good commit).
2. From `services/den/`, run **`sqlx migrate info`** (with `DATABASE_URL` pointing at the affected database). If a version shows **`(different checksum)`**, the database still stores the checksum from the wrong file.
3. **Align the database** with the reverted file: update `public._sqlx_migrations` for that version so `checksum` equals the **local** checksum reported by `sqlx migrate info` (the value after “local migration has checksum …”). Alternatively, restore the on-disk file to match the checksum that was actually applied (worse—keeps the mistake in git).

Only the migration **checksum** row may need fixing if the executed SQL was identical (e.g. comment-only change). If you changed executable SQL and it already ran in production, you need a **new** migration to fix the schema, not a rewrite of the old file.

---

At **process startup**, `den` runs embedded migrations against `DATABASE_URL` (same files as below). For **authoring** schema changes you still use **`sqlx migrate run`** / **`sqlx migrate add`** from the `services/den/` directory when developing locally.

| File | Purpose |
|------|---------|
| [`20250309000000_trestle.up.sql`](20250309000000_trestle.up.sql) | Starter: `users`, invites, email, OAuth tables |
| [`20250331120000_phase1_den_registry.up.sql`](20250331120000_phase1_den_registry.up.sql) | **Phase 1 M1**: `bears`, `user_bear`, `audit_chat`; `users.webui_account_id` + index; `users.is_admin` |
| [`20250401120000_phase1_bear_provisioning_fields.up.sql`](20250401120000_phase1_bear_provisioning_fields.up.sql) | **Phase 1 M1b**: `bears.system_prompt`; nullable `bears.letta_agent_id` until provisioned |
| [`20250401130000_phase1_bootstrap_admin.up.sql`](20250401130000_phase1_bootstrap_admin.up.sql) | **Bootstrap operator** (only if no `username = 'admin'` yet): see **Default operator account** below |
| [`20250413120000_bear_letta_sync_fields.up.sql`](20250413120000_bear_letta_sync_fields.up.sql) | `bears.letta_agent_type`, `bears.letta_tool_ids` |
| [`20260416120000_bear_chat_activity.up.sql`](20260416120000_bear_chat_activity.up.sql) | `bear_chat_activity` (later dropped) |
| [`20260416120100_drop_bear_chat_activity.up.sql`](20260416120100_drop_bear_chat_activity.up.sql) | Drop `bear_chat_activity` |
| [`20260418130000_drop_users_webui_account_id.up.sql`](20260418130000_drop_users_webui_account_id.up.sql) | Drop `users.webui_account_id` + index |
| [`20260429120000_acp_tokens.up.sql`](20260429120000_acp_tokens.up.sql) | ACP code tokens and scopes |
| [`20260430120000_acp_client_tool_calls.up.sql`](20260430120000_acp_client_tool_calls.up.sql) | Legacy persisted ACP client tool relay calls; superseded by process-local Codepool waiters |
| [`20260430121000_acp_sessions.up.sql`](20260430121000_acp_sessions.up.sql) | ACP sessions and Codepool session binding |
| [`20260501120000_archived_conversations.up.sql`](20260501120000_archived_conversations.up.sql) | Archived conversation tracking |
| [`20260501121000_drop_users_admin_flag.up.sql`](20260501121000_drop_users_admin_flag.up.sql) | Backfill canonical `users.is_admin` and drop legacy `users.admin_flag` |
| [`20260502120000_drop_acp_client_tool_calls.up.sql`](20260502120000_drop_acp_client_tool_calls.up.sql) | Drop obsolete `acp_client_tool_calls`; Den now proxies ACP tool results statelessly to Codepool |

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

Production **container** runs apply the same migrations automatically on startup (`src/lib.rs`); you do not need a separate deploy-time migration step for normal hosting.
