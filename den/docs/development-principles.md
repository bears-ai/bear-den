# Implementation philosophy (general)

Agent- and product-agnostic practices for building a **small surface-area** web application: few moving parts, boring URLs, and a frontend that stays thin. Use this as a pattern guide; it is not tied to any particular domain.

---

## 1. Keep dependencies intentionally small

- **Prefer one clear library per concern** (HTTP server, database access, templates, serialization) instead of stacking overlapping tools.
- **Disable default features** on heavy crates when possible; enable only what you use (smaller graphs, faster builds, less risk).
- **Treat optional integrations as optional**: if a feature is not in use, remove it from the dependency graph rather than carrying commented “maybe later” stacks.
- **Separate dev-only tools** from production dependencies so release binaries and security audits stay focused.
- **Favor compile-time verification** for high-risk boundaries (e.g. SQL, routing contracts) when the cost is acceptable—fail at build time, not only in production.

---

## 2. Server-first UI with dumb templates

- **Render HTML on the server**; templates describe structure and light conditionals, not business rules.
- **Do data shaping in handlers** (or small dedicated modules): filters, joins, defaults, and permission checks happen before the template sees the data.
- **Avoid template dialect features that encourage logic**; if the engine is a subset of a larger templating language, accept that constraint and keep templates portable and reviewable.

---

## 3. Bare-bones browser code

- **Vanilla JavaScript** for app-specific behavior; avoid SPA frameworks unless the product truly needs client-managed state at scale.
- **Progressive enhancement**: core flows should remain usable if scripts fail or load slowly.
- **Minimal front-end toolchain**: no mandatory Node/bundler step for day-to-day work if plain assets suffice. When a heavy capability is needed (e.g. maps), **load it explicitly** (CDN or a single vendored bundle) rather than importing an entire frontend ecosystem.
- **Centralize styling** in shared stylesheets; avoid inline styles and one-off patterns that fight the rest of the design system.

---

## 4. Straightforward URL mapping

- **Path structure matches the conceptual model**: nested resources use nested routes; public vs authenticated areas are separated in the router, not only in templates.
- **Use the framework’s current path syntax** (e.g. named segments in braces) consistently so routes stay copy-pasteable and documented.
- **Stable API prefixes**: mount machine-facing routes under a short, explicit prefix; **version** external APIs (`/v1.0/...`) when consumers exist outside the same deployable.
- **Keep a living route map** (even a single markdown index) updated when top-level paths change—humans and agents both benefit.

---

## 5. Assets and deployment simplicity

- **Embed static assets in the binary** when it reduces operational drift (“no missing files on disk”) and keeps deploys single-artifact.
- **Prefer environment configuration** over runtime config servers for small deployments.
- **Optimize the developer binary, not only release**: pragmatic compiler profiles (lighter debug info for dependencies, etc.) reduce iteration friction without sacrificing production settings.

---

## 6. Observability proportional to scale

- **Structured logging** as the default backbone; add distributed tracing or hosted APM only when pain justifies the dependency and ops cost.
- **Clear error typing** at boundaries (HTTP, parsing, external services) so logs and responses stay consistent without a framework sprawl.

---

## Summary

**Smaller dependency graphs, server-owned logic, vanilla browser code, and readable URLs** reduce long-term cost: less breakage from upstream rewrites, fewer security patches, faster onboarding, and easier automation against a stable surface.
