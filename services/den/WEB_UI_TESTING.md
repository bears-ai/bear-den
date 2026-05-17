# Bear web UI testing in development

Use this when you want to test the bear UI with the **real Den web routes, handlers, MiniJinja templates, and browser JavaScript** while swapping selected integration data for deterministic fixtures.

## Quick start

Build/run Den with fixture capability compiled in:

```bash
cargo run -p den --features web-ui-fixtures
```

Enable a named fixture profile at runtime:

```bash
UI_FIXTURE_PROFILE=bear-details-rich cargo run -p den --features web-ui-fixtures
```

You can also use other supported profiles:

- `bear-details-rich`
- `bear-details-warning`
- `bear-chat-basic`
- `bear-chat-error`

## Important behavior

Fixture-backed bear UI testing is enabled only when **both** are true:

1. the binary is compiled with Cargo feature `web-ui-fixtures`
2. `UI_FIXTURE_PROFILE` is set to a supported profile name

Without both, Den keeps using the real integrations.

When a fixture profile is active, the base web template shows a visible banner naming the active profile.

## Where it is implemented

Primary docs:

- `src/web/WEB_UI_FIXTURES.md`
- `src/web/ROUTES.md`

Primary code:

- `src/web/data/mod.rs`
- `src/web/data/letta.rs`
- `src/web/data/memory.rs`
- `src/web/data/chat_transport.rs`
- `src/web/data/fixtures.rs`

App wiring:

- `src/web/mod.rs`
- `src/config.rs`

Main bear UI entrypoints:

- `src/web/bear_management.rs`
- `src/web/bear_chat.rs`
- `src/web/v1/mod.rs`

## What this is for

Use this workflow when you want to iterate on:

- bear details pages
- bear role detail panes
- conversations/memory pages
- chat UI/history loading
- front-end behavior tied to the real JSON routes

without depending on live Letta, MemFS Manager, or Codepool data.

## Caveat

Fixture-backed streaming send behavior for `/v1/chat/send` is only partially wired right now. Read paths are fixture-capable; send streaming fixture behavior may still return a clear not-yet-wired error depending on the selected profile.
