# Renaming from the Trestle starter (`newapp`)

After cloning, treat this as a checklist so URLs, cookies, and the crate line up with your product.

## Crate and binary

- [ ] [`Cargo.toml`](../Cargo.toml): `name = "..."` (library + default binary crate name).
- [ ] Binary remains `src/main.rs` calling `newapp::run()` until you rename the package—then use `yourcrate::run()`.
- [ ] [`src/web/assets/manifest.json`](../src/web/assets/manifest.json) PWA `name` / `short_name` if you ship those assets.

## Docker and devcontainer

- [ ] Root [`Dockerfile`](../Dockerfile): `ARG APP_NAME=...` and copy path `target/release/$APP_NAME`.
- [ ] [`.devcontainer/devcontainer.json`](../.devcontainer/devcontainer.json): `"name"` field.

## Domains, URLs, CORS, JWT

- [ ] [`src/config.rs`](../src/config.rs): production `URL_PREFIX`, default `WEB_SERVER_URL` / `API_SERVER_URL` placeholders.
- [ ] [`src/api/service.rs`](../src/api/service.rs) and [`src/api/oauth/router.rs`](../src/api/oauth/router.rs): production CORS allowlist.
- [ ] [`src/api/oauth/jwt.rs`](../src/api/oauth/jwt.rs): `JWT_ISSUER`, `JWT_AUDIENCE`, test/default secrets.
- [ ] [`src/core/email/mod.rs`](../src/core/email/mod.rs): `MAIL_FROM_ADDRESS`.
- [ ] README and docs: example URLs and hostname references.

## Sessions

- [ ] Production: set `SESSION_COOKIE_DOMAIN` when sessions must be shared across subdomains (e.g. `.example.com`). Leave unset for host-only cookies.

## Observability

- [ ] [`src/lib.rs`](../src/lib.rs): default `RUST_LOG` filter uses the crate module path (`newapp::...`). After renaming the package, update those strings or rely fully on `RUST_LOG`.

## SQLx offline metadata

- [ ] Regenerate and commit [`.sqlx/`](../.sqlx/) after schema/query changes: `sqlx migrate run` then `cargo sqlx prepare` against a migrated database (see [`sqlx-patterns.md`](sqlx-patterns.md)).

## Grepping

```bash
rg -n 'newapp\.example|\bnewapp\b' --glob '!target' --glob '!.sqlx' .
```

Optional helper (non-strict lists matches; `--strict` exits non-zero if any remain):

```bash
./scripts/verify-rename.sh
./scripts/verify-rename.sh --strict
```

On an unchanged starter clone, `--strict` is expected to fail until you complete the rename.
