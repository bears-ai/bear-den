//! Ensures the PHC string in `migrations/20250401130000_phase1_bootstrap_admin.up.sql`
//! still verifies with `password-auth` (the same crate the web login uses).

#[test]
fn bootstrap_admin_migration_passhash_verifies() {
    const HASH: &str = concat!(
        "$argon2id$v=19$m=19456,t=2,p=1$r9jUROUtlVPS5+Segpn0uw$",
        "tno+kfSkOqyaZUuJE1FopBe/aDHxzdpNsRONzSM6rG4",
    );
    password_auth::verify_password("Never deploy with default passwords.", HASH)
        .expect("bootstrap admin passhash in migration must match password-auth verify_password");
}
