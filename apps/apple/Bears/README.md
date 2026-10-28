# Bears Apple client

The active [app roadmap](../../../docs/roadmap/MACOS_BEARS_CLIENT_APP_PLAN.md) selects an **AHP 1.0 subset** for conversation interaction and versioned `/v1.0` JSON APIs for discovery, creation and history. This directory contains the first text-only client slice in source, not a released or deployment-validated app.

## First slice

- OAuth Authorization Code + S256 PKCE in the system authentication browser.
- Browse current Bear memberships, read the selected Bear's profile/shared hats, and browse authorized conversations.
- Explicitly select a hat to create a conversation; retries retain the same request UUID and hat.
- Read canonical history, stream text, stop the connection's reply and reconnect using fresh snapshots. Sends are never automatically replayed; read access does not grant send authority.
- Separate Connection and optional Armature settings. **No helper installation, ACP client or local workspace is required for remote chat.** ACP/BearWire remains the local-work path, not the human OAuth API.

The supported preview UI is Apple Silicon macOS 13+. `Shared/Client` is reusable Swift core groundwork for future iOS; there is **no actual iOS app target or validated iOS build**. The AHP review baseline is upstream `spec/v1.0.0`; the current client is custom Swift with no upstream SDK dependency, not a complete AHP host/client implementation.

## Connect to Den

Follow [BUILDING.md](BUILDING.md#den-and-oauth-setup) before testing login:

- Manually register a public client on each Den: `public=true`, `trusted=false`, exact callback `bears://oauth/callback`, scopes `profile:read data:read data:write`. Supply that client ID in Connection settings; do not embed a client secret or use an Armature token.
- Use the **API deployment root**, not an assumed web address. API defaults to port `3001`, web to `3000`; ingress must serve `/oauth/*`, `/v1.0/*` and WebSocket Upgrade at `/v1.0/agent-host`, with browser login/session cookies correctly routed/shared.
- Current access tokens last about one hour; reauthorize rather than refresh. Tokens are in device-only, non-synchronizing Keychain items scoped by Den URL/client ID. Sign out stops local activity and attempts Keychain removal; it is **not server-side token revocation** or browser-session logout. Removal failures are shown with a retry action.

## Layout

- `Shared/Client/` — typed identifiers, JSON/OAuth/AHP clients, transports and transcript reducer; independent of SwiftUI/AppKit/Keychain.
- `BearsApp/Client/Model/` — app-owned observable state and injectable API/socket/browser/token dependencies.
- `BearsApp/Client/Views/` — native connection, Bear/profile/conversation and text-chat views.
- `BearsApp/Client/Platform/` — system authentication browser, secure PKCE generation, preferences and Keychain adapters.
- `BearsApp/` — macOS entry point and existing optional armature install/diagnostics UI.
- `Tests/BearsClientTests/`, `Tests/BearsAppTests/` — core protocol/HTTP/reducer fixtures and model/platform regression tests.

## Limits and evidence (2026-10-28)

Text only: **no permission/question replies, subagents, refs/resources, attachments, local tools, Wasmer/local sandbox, live multi-client fan-out, Docket management or Bear-admin editing**. AHP snapshot reconnect is not durable event replay or resumable inference; disconnect cancels the connection's turn. Inventory is capped at 100 visible conversations; oversized snapshots fall back to paged JSON history. Broader roadmap requirements remain follow-ons, not completed features. Real-time voice/video and external agent interoperability are excluded.

The implementation effort reports **30 core tests passing in Linux Swift 5.10.1 Docker**, Apple syntax checks, and **17 earlier model/composer fake-dependency tests** in an isolated harness. The server checkpoint reports 57 OAuth, 18 native API and 9 channel-runtime tests passing with locked/offline Cargo and disposable Postgres. These are not evidence of a Mac app compile/test run, actual browser or Keychain behavior, DMG/signing, deployed ingress or live-provider chat. Native/platform acceptance and release remain pending; see [build/test instructions](BUILDING.md).

Agenthesia is a UX/protocol reference only: its GPL-3.0 code and macOS 26 baseline cannot be copied/linked into this PolyForm Noncommercial/macOS 13 app without licensing resolution. No Agenthesia code or SDK is a dependency here.
