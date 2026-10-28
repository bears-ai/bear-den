# Building and testing the Bears Apple client

The [active app plan](../../../docs/roadmap/MACOS_BEARS_CLIENT_APP_PLAN.md) selects AHP 1.0 plus `/v1.0` JSON APIs. Source now contains the first OAuth/Bear/profile/conversation/text-chat slice. These are instructions for local validation, **not evidence of a successful native build, DMG or released app**.

## Prerequisites and targets

- Apple Silicon Mac running macOS 13+ for the preview UI (Intel displays an unsupported-architecture screen).
- Xcode with Swift 5.10 or a compatible toolchain (for example Xcode 15.3+).
- A reachable Den API with the matching native routes and a manually registered public OAuth client (below).
- **No `bear-armature`, ACP client, local workspace or sandbox is required for remote login/chat.** Optional Armature settings retain the earlier install/repair workflow.

`Package.swift` exposes the platform-neutral `BearsClient` library and, on macOS, the `Bears` executable backed by `BearsApp`. Strict concurrency checking is enabled. The manifest's iOS deployment declaration is shared-core groundwork, not an actual iOS app target or evidence of an iOS build. `Shared/Client/` has no upstream AHP SDK dependency; the custom wire models target the bounded `spec/v1.0.0` subset, not every AHP host capability.

## Den and OAuth setup

### 1. Route the actual API origin

Use the API **deployment root**, including any ingress path prefix, in the app's **Connection → Den API URL** field. Do not append `/v1.0`, `/oauth` or `/agent-host`: the client appends endpoint paths.

Den defaults to separate web/API listeners: `PORT=3000` for web, `API_PORT=3001` for API with `RUN_API=true`. A deployment may publish a different HTTPS origin/port. Do not assume `http://localhost:3000` exposes the API. The client requires HTTPS except for `http://localhost` and `http://127.0.0.1` development URLs. Environment/deployment settings belong in `.env`, not compiled app defaults.

Ingress must route all of these to the API listener:

- `/oauth/authorize`, `/oauth/token`, `/oauth/userinfo`;
- `/v1.0/bears`, `/v1.0/chat/conversations`, `/v1.0/chat/history`;
- **WebSocket Upgrade at `/v1.0/agent-host`**, preserving the bearer `Authorization` header and the long-lived connection. An ordinary HTTP-only proxy route is insufficient.

The authorization browser also needs web login pages/assets and a valid Den browser session. Either publish web and API paths behind one correctly routed public origin, or deliberately configure trusted sibling web/API hosts with matching session backing/keys and an appropriate `SESSION_COOKIE_DOMAIN`. Host-only cookies do not cross sibling hosts; broaden a cookie domain only when every included host is trusted. Keep `WEB_SERVER_URL` and `API_SERVER_URL` consistent with the deployed routes. Login redirect loops or missing consent can indicate incorrect routing/session cookies, not an app credential problem. The native JSON/WebSocket clients use bearer tokens, not browser cookies.

The [API route inventory](../../../services/den/crates/den-api/src/ROUTES.md#native-apple-client-first-slice) documents the implemented edge. No deployment, proxy or Compose change is performed by these instructions.

### 2. Manually register a public client on each Den

As a Den operator, use web **OAuth clients** (`/admin/oauth_clients/`) to create an active native client with:

| Setting | Value |
| --- | --- |
| Public client | `public=true` (PKCE; no client secret) |
| Trusted client | `trusted=false` (retain human consent) |
| Exact redirect URI | `bears://oauth/callback` |
| Allowed/requested scopes | `profile:read data:read data:write` |

Copy that Den's public client ID into **Public OAuth client ID**, save the connection, then choose **Sign in with Den**. There is no dynamic registration or globally pre-provisioned app client ID. Do not paste a Bear-scoped Armature token, provider credential or confidential-client secret into this field. The older [OAuth module guide](../../../services/den/crates/den-oauth/src/oauth/README.md) includes confidential-client examples; those secret-based examples are not the native app configuration.

The system authentication browser uses Authorization Code + S256 PKCE with secure random verifier/state. The client checks callback scheme/host/path/state before code exchange. Den requires public-client S256 PKCE and binds untrusted consent to the authenticated browser session. The app requests an ephemeral browser session; existing Safari login state is not guaranteed to be reused. The generated app bundle registers the `bears` URL scheme; verify callback delivery with the packaged app on a real Mac rather than assuming `swift run` establishes the complete bundle/Launch Services behavior.

### 3. Token lifetime and sign out

Current access tokens last about **one hour**. Public clients receive no refresh credential and the app has no refresh flow; sign in again after expiry or an invalid-token response.

Connection URL/client ID are stored in UserDefaults. Tokens are stored only in non-synchronizing, `WhenUnlockedThisDeviceOnly` Keychain items, scoped by normalized Den URL and public client ID. Login is not reported persistent if the Keychain write fails. **Sign out** cancels local activity and attempts to remove the saved token; a deletion failure is shown with a retry action. It does **not** revoke the access token on Den or sign out the browser session. Operator token revocation/client deactivation is a separate server operation. Real Keychain/browser behavior still needs macOS acceptance.

## Build and run

From the package root on macOS:

```bash
cd apps/apple/Bears
swift build --product Bears
swift run Bears
```

The main window owns the human client; Settings separates Connection from optional Armature management. Browse a member Bear, inspect its profile/shared hats, select an explicit hat for a new conversation, then chat. Listed history may be read-only: Bear-admin visibility is not permission to send as another owner, and archived/Work transcripts remain inspection-only.

## Tests and current evidence

On macOS, from the package root:

```bash
swift test
```

Focused suites:

```bash
swift test --filter BearsClientTests
swift test --filter NativeClientModelTests
swift test --filter ComposerStateTests
swift test --filter ClientPlatformTests
```

On Linux, the manifest includes only `BearsClient` and its tests; `swift test` there cannot validate SwiftUI/AppKit/AuthenticationServices/Security or the app target. Platform tests use injected browser/Keychain dependencies where applicable; passing those also does not substitute for real browser/Keychain acceptance.

**Reported implementation checkpoint, 2026-10-28:**

- 30 shared-core tests pass in Linux Docker with Swift 5.10.1 (HTTP/OAuth/AHP wire and transcript fixtures).
- Apple source syntax checks pass; this is not native compilation or Apple-framework type-checking.
- 17 earlier model/composer tests pass with fake dependencies in an isolated harness; no native `BearsAppTests`/platform-suite run is claimed.
- Focused server tests pass using standard `SQLX_OFFLINE=true`, Cargo `--locked --offline` and disposable `DATABASE_URL=postgres://bears:bears@127.0.0.1:5432/den`: 57 OAuth, 18 native API, 9 channel-runtime tests. Runtime tests capture fake-provider requests and canonical persistence, including next-turn replay, tool exchanges and terminal outcomes; not live-provider inference.

[Apple client CI](../../../.github/workflows/apple-client.yml) now builds/tests the native package and creates an unsigned development DMG on a macOS runner; it has not been executed in this Linux workspace and is not a green-run claim. Focused browser source-admission tests and all-target strict Clippy for the five affected server crates also pass. The production Alpine/musl image builds with `docker build --build-arg SQLX_OFFLINE=true -t bears-den:native-client-validation services/den`; no container was deployed. The Dockerfile's online-SQLx default fails without a build-reachable database, so the offline argument is required for this validation. **Still unverified:** Mac app compile/tests, actual system-browser callback/Keychain, DMG/signing/notarization, live provider, deployed proxy/WebSocket and end-to-end release. No release or deployment follows from the above results.

For focused server reruns from the repository root, configure a **disposable** local Postgres database in `DATABASE_URL` first (database-backed tests create isolated databases; do not use production credentials):

```bash
SQLX_OFFLINE=true cargo test --manifest-path services/den/Cargo.toml --locked --offline -p den-oauth --lib
SQLX_OFFLINE=true cargo test --manifest-path services/den/Cargo.toml --locked --offline -p den-api --lib v1::native
SQLX_OFFLINE=true cargo test --manifest-path services/den/Cargo.toml --locked --offline -p den-runtime --lib native_runtime::channel_turn::tests
```

### Native acceptance checklist (pending)

- Public-client login, explicit consent/denial/cancel, wrong callback/state rejection, expired token/reauthorization; actual Keychain save/load/removal and visible failure recovery.
- Member Bear/profile/hat inventory, no-hat state, explicit creation and retry with the same UUID/hat; wrong-owner/archived/Work send denial.
- Live text stream, stop, partial/failure/empty outcomes and next-turn canonical history. Verify uncertain delivery preserves the draft and **never automatically retries a send**.
- Disconnect/window close cancels the owned reply; reconnect uses fresh snapshots, not event/send replay or resumable inference. A second client does not receive live fan-out or control another connection's turn.
- Oversized snapshots can be inspected through paged JSON history. Inventory is capped at 100 visible conversations; this is not a complete catalog/offline sync.
- Repeat remote login/chat with no helper installed; verify real HTTPS ingress and WebSocket Upgrade before claiming deployed support.
- Review keyboard/accessibility/error UX; test callback delivery from the packaged app separately from command-line launch.

Permissions/questions, subagents, refs/resources, attachments/multimodal content, local tools, Wasmer/local sandbox, live multi-client, Docket management and Bear-admin editing are **not implemented in this MVP**. They remain scoped roadmap follow-ons, not acceptance items already completed. Real-time voice/video and external agent interoperability are excluded.

## Optional armature install and repair

This workflow is separate from remote chat. The preview can use a bundled `bear-armature` resource (legacy resource filename `bears-acp-adapter`) or download an adapter artifact when no bundle is present. From the package root, optionally prepare a local bundle:

```bash
bash Scripts/prepare_adapter.sh
```

The helper checks existing debug builds before attempting Cargo. Release or explicit prebuilt overrides are available:

```bash
PROFILE=release bash Scripts/prepare_adapter.sh
ADAPTER_BINARY=/path/to/bear-armature bash Scripts/prepare_adapter.sh
```

It places the resource at `BearsApp/Resources/Adapter/bears-acp-adapter`. Without a bundle, the existing download flow uses `https://bears-ai.github.io/bear-den/bears-acp-adapter/stable/macos.json`, reading its `version` and `pkg_url`. Development overrides:

```bash
BEARS_ADAPTER_MANIFEST_URL=https://example.com/path/to/macos.json swift run Bears
BEARS_ADAPTER_DOWNLOAD_URL=https://example.com/path/to/bears-acp-adapter-aarch64-apple-darwin.pkg swift run Bears
```

The install flow accepts a Mach-O adapter binary or `.pkg`; a `.pkg` invokes the system installer targeting `/` and may prompt for administrator credentials. Publishing a rebuilt adapter package requires bumping `version` in `tools/bear-armature/Cargo.toml`, updating the lockfile if resolution changes and rebuilding/republishing release assets. Packaging-script changes alone can otherwise leave the existing version/tag/download in use. None of this is a prerequisite to app OAuth or text chat.

## Version metadata and regression tests

The optional install manager probes `bear-armature version --json`: one JSON object on stdout, with no Den authentication or browser/network probing. Human `--version` output remains on stderr. The app checks both streams independently, retains legacy text fallback and rejects diagnostics as a version. On macOS:

```bash
swift test --filter AdapterVersionReaderTests
```

Binary CLI regressions, from repository root:

```bash
cargo test --manifest-path tools/bear-armature/Cargo.toml --locked --test version_cli
```

A probe failure is not proof of corruption. Inspect its executable path and launch/probe/parse diagnostics for permissions, architecture, quarantine or missing/incompatible files. Source fixes are not evidence that a published app/package contains them; rebuild/release the relevant artifact and obey the adapter version-bump rule above.

## Building a distributable DMG

On macOS, from repository root:

```bash
./packaging/macos/build-dmg.sh --app-version 0.1.0
```

The script is intended to build the release `Bears` executable, wrap it in `Bears.app`, include resources and register the `bears` URL scheme, then create `dist/macos/Bears-0.1.0.dmg` with an `/Applications` symlink. An optional `--application-identity` (or `MACOS_APPLICATION_CERT_IDENTITY`) enables Developer ID Application signing/verification; without it, the app is unsigned. **This checkpoint has not run a Mac build or produced/validated that DMG.**

Shipping distribution still needs actual native build/resource/callback validation, signing/notarization and release acceptance. Sparkle and automated app publishing are not delivered by the text-client slice. Do not treat packaging source or the added CI lane as a signed/notarized release.
