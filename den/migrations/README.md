### Migrations

Applied with **`sqlx migrate run`** from the `den/` directory (requires `DATABASE_URL`).

| File | Purpose |
|------|---------|
| [`20250309000000_trestle.up.sql`](20250309000000_trestle.up.sql) | Starter: `users`, invites, email, OAuth tables |
| [`20250331120000_phase1_den_registry.up.sql`](20250331120000_phase1_den_registry.up.sql) | **Phase 1 M1**: `bears`, `user_bear`, `audit_chat`; `users.webui_account_id`, `users.is_admin` |

**Note:** Legacy `users.id` is still `serial`. `user_bear.user_id` is `INTEGER` FK to `users(id)` so the schema is consistent without a UUID cutover. A later milestone may migrate identity to UUID per [PHASE1_BOOTSTRAP.md](../../docs/planning/PHASE1_BOOTSTRAP.md).

Production builds with `--features=production` also run `sqlx migrate` from `build.rs` when `DATABASE_URL` is set at compile time.
