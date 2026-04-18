### Migrations

At **process startup**, `den` runs embedded migrations against `DATABASE_URL` (same files as below). For **authoring** schema changes you still use **`sqlx migrate run`** / **`sqlx migrate add`** from the `den/` directory when developing locally.

| File | Purpose |
|------|---------|
| [`20250309000000_trestle.up.sql`](20250309000000_trestle.up.sql) | Starter: `users`, invites, email, OAuth tables |
| [`20250331120000_phase1_den_registry.up.sql`](20250331120000_phase1_den_registry.up.sql) | **Phase 1 M1**: `bears`, `user_bear`, `audit_chat`; `users.is_admin` (historically also added `webui_account_id`, dropped below) |
| [`20260418130000_drop_users_webui_account_id.up.sql`](20260418130000_drop_users_webui_account_id.up.sql) | Remove unused `users.webui_account_id` + index |
| [`20250401120000_phase1_bear_provisioning_fields.up.sql`](20250401120000_phase1_bear_provisioning_fields.up.sql) | **Phase 1 M1b**: `bears.system_prompt`; nullable `bears.letta_agent_id` until provisioned |
| [`20250401130000_phase1_bootstrap_admin.up.sql`](20250401130000_phase1_bootstrap_admin.up.sql) | **Bootstrap operator** (only if no `username = 'admin'` yet): see **Default operator account** below |

### Default operator account

After migrations have been applied (first container start or local `cargo run` on an empty DB), you can sign in at `/login` with:

| Field | Value |
|-------|--------|
| Username | `admin` |
| Password | `Never deploy with default passwords.` |
| Email (stored) | `admin@localhost` |

**Production:** Change this password immediately (or remove the user and create operators another way). The password is documented here and in the migration file on purpose for local bootstrap only.

The stored `passhash` is Argon2id (PHC). If you change the password string in the migration, regenerate the hash with `password_auth::generate_hash` from the same `password-auth` version as in `Cargo.toml`, and update [`tests/bootstrap_admin_passhash.rs`](../tests/bootstrap_admin_passhash.rs).

**Note:** Legacy `users.id` is still `serial`. `user_bear.user_id` is `INTEGER` FK to `users(id)` so the schema is consistent without a UUID cutover. A later milestone may migrate identity to UUID per [PHASE1_BOOTSTRAP.md](../../docs/planning/PHASE1_BOOTSTRAP.md). Prefer column **`is_admin`** for operators; legacy **`admin_flag`** remains for older queries until fully retired.

Production **container** runs apply the same migrations automatically on startup (`src/lib.rs`); you do not need a separate deploy-time migration step for normal hosting.
