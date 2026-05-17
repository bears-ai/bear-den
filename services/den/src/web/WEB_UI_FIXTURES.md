# Feature-gated web UI fixtures

This document explains Den's **feature-gated web UI fixture support** for browser/UI smoke testing.

## Purpose

The goal is to let developers exercise the real web stack:

- real Axum routes
- real handler/orchestration logic
- real MiniJinja templates
- real browser JavaScript
- real `/v1/chat/*` JSON endpoints

while swapping selected integration data sources for deterministic fixture-backed providers.

This makes it easier to iterate on page changes without depending on live Letta, MemFS Manager, or
Codepool services.

## Safety model

Fixture support is intentionally split into two layers:

1. **Cargo feature** — `web-ui-fixtures`
   - controls whether the fixture-backed providers are even compiled into the binary.
2. **Runtime selector** — `UI_FIXTURE_PROFILE`
   - chooses a named fixture profile for a running process.

Both are required for fixture-backed web data to activate.

### Why both?

This avoids confusion for other development activities:

- normal builds do not include fixture-specific behavior unless explicitly requested
- deployed builds can omit the capability entirely
- even a fixture-capable build still uses real integrations unless a profile is selected

## Current scope

Phase 1 routes selected **integration-like web dependencies** through explicit ports:

- `WebLettaDataSource`
- `WebMemoryDataSource`
- `WebChatTransportDataSource`

The following web surfaces continue to use their normal real handlers/templates while pulling data
through those ports:

- `/bear/{slug}/details`
- `/bear/{slug}/details/roles/{role}`
- `/bear/{slug}/details/conversations`
- `/bear/{slug}/details/memory`
- `/v1/chat/conversations`
- `/v1/chat/history`
- `/v1/chat/conversations/{conversation_id}`
- `/v1/chat/send`

The feature is intentionally **not** a generic fake-mode for the whole service.

## Building with fixture capability

Example:

```bash
cargo run -p den --features web-ui-fixtures
```

If you do **not** compile with `web-ui-fixtures`, Den logs a warning and keeps using the real
integrations even if `UI_FIXTURE_PROFILE` is set.

## Selecting a fixture profile

Set `UI_FIXTURE_PROFILE` to one of:

- `bear-details-rich`
- `bear-details-warning`
- `bear-chat-basic`
- `bear-chat-error`

Example:

```bash
UI_FIXTURE_PROFILE=bear-details-rich cargo run -p den --features web-ui-fixtures
```

When active, Den:

- logs a startup warning naming the active fixture profile
- shows a visible banner in the base web template

This makes it obvious when you are looking at fixture-backed data.

## Expectations

Fixture mode is intended for:

- page/layout iteration
- browser smoke testing
- stable UI states during development
- validating handler/template changes without live external services

Fixture mode is **not** a replacement for real integration testing. In particular, it does not prove
that live SQL queries or external service contracts are correct.

## Design rule

We avoid stubbing at the page/view-model layer. Instead, the web routes continue to execute real
logic and only selected data sources are swapped underneath them.

That means the following stay real:

- route parsing
- auth/session checks
- page branching and filtering
- JSON response shaping
- template rendering
- browser JavaScript behavior

## Future work

Phase 1 focuses on the highest-value web dependencies. If later development shows that local DB
setup is still a bottleneck, additional **data-layer** ports can be introduced carefully, while
keeping the same explicit feature-gated safety model.
