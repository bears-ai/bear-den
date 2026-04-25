-- Phase 1: default operator account for fresh installs (homelab / first login).
--
-- Default credentials (change or delete this user in production):
--   Username: admin
--   Password: Never deploy with default passwords.
--   Email:    admin@localhost (for display / future flows; sign in with username "admin")
--
-- Passhash is Argon2id (PHC string) from `password_auth` crate v1 `generate_hash` — must stay
-- in sync with `verify_password` in the app. See `tests/bootstrap_admin_passhash.rs`.

INSERT INTO users (email, username, display_name, passhash, admin_flag, is_admin)
SELECT 'admin@localhost',
    'admin',
    'Admin',
    '$argon2id$v=19$m=19456,t=2,p=1$r9jUROUtlVPS5+Segpn0uw$tno+kfSkOqyaZUuJE1FopBe/aDHxzdpNsRONzSM6rG4',
    true,
    true
WHERE NOT EXISTS (
    SELECT 1
    FROM users u
    WHERE u.username = 'admin'
);
