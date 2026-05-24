# macOS Bears client app implementation plan

Status: proposed implementation plan.

## Goal

Evolve the macOS distribution from a signed `.pkg` that installs `bears-acp-adapter` into a native **Bears client app** while keeping the Linux/devcontainer adapter lean, testable, and free of macOS UI specifics.

The app should improve onboarding for non-technical macOS users without moving protocol authority, Bear provisioning authority, or runtime policy out of Den.

Longer term, the app should be designed so it can become a **universal Apple client** with an iOS/iPadOS version. The first shipping target remains macOS, but architectural choices should avoid baking in assumptions that only work for macOS helper binaries, local filesystem access, or ACP process launching.

## Summary decision

Build a native **SwiftUI macOS app** as a platform shell around the existing shared Rust adapter/runtime core, while keeping app UI/state architecture portable enough for a future iOS/iPadOS target.

```text
Shared Rust core
  ├── Linux/devcontainer CLI: bears-acp-adapter
  └── macOS app shell: Bears.app
```

The shared Rust core remains the source of truth for:

- ACP stdio behavior;
- BearWire/Den protocol behavior;
- config validation;
- `doctor` checks;
- adapter diagnostics;
- local tool/capability negotiation;
- future reconnect/resume logic.

The Apple client app owns:

- native onboarding UI;
- token/login setup;
- Keychain integration;
- local config editing where the platform permits it;
- running `doctor` and showing results on macOS;
- configuring supported ACP clients when possible on macOS;
- showing logs/status;
- installing/updating the helper CLI on macOS;
- app self-update on macOS with Sparkle;
- future BearWire desktop runtime features on macOS;
- future mobile-friendly Bear status, administration, notifications, and remote-control UX on iOS/iPadOS.

## Non-goals

- Do not put ACP/BearWire protocol logic primarily in Swift.
- Do not make the macOS app required for Linux/devcontainer use.
- Do not add macOS frameworks to the Linux adapter build.
- Do not put macOS-only assumptions into shared Apple UI/state code that should later run on iOS/iPadOS.
- Do not make local runtime access imply Bear administration authority.
- Do not replace Den admin APIs or the Den operator console.
- Do not introduce a persistent LaunchAgent until there is a clear need.
- Do not turn the app into an external agent protocol surface; A2A remains the future external agent interoperability path.

## Architecture

### Universal Apple app direction

The app should be structured as an Apple client, not only as a macOS installer UI.

Architectural implications:

- Prefer SwiftUI and shared view models that can compile for macOS and iOS/iPadOS.
- Keep macOS-only features behind platform-specific services/protocols.
- Treat helper binary management, ACP client auto-configuration, local filesystem access, LaunchAgents, and process spawning as macOS capabilities, not universal app assumptions.
- Treat iOS/iPadOS as a remote/administrative client: it can authenticate with Den, show Bear status, manage allowed administration flows, receive notifications, and interact with remote BearWire-capable runtimes, but it cannot host the same local ACP stdio adapter model.
- Keep Den/BearWire APIs usable by both macOS and iOS clients without requiring local helper availability.

Suggested layering:

```text
Shared Apple app layer
  - SwiftUI views
  - app navigation/state
  - Den API client
  - auth/session model
  - Bear/admin/status models

macOS services
  - helper install/update
  - run doctor
  - ACP client configuration
  - local logs
  - workspace registration
  - optional LaunchAgent

iOS/iPadOS services
  - remote Den auth
  - notifications
  - Bear/admin/status UI
  - remote workspace/runtime selection
  - no local ACP stdio helper
```

### Shared Rust core

Keep shared behavior in Rust modules/crates that build on Linux and macOS.

Responsibilities:

- parse and validate config;
- resolve config from args/env/config files;
- run ACP stdio server;
- connect to Den/BearWire;
- run `doctor` checks;
- expose machine-readable JSON status;
- perform client-tool and permission mediation;
- produce structured logs.

Target structure, either as crates or modules:

```text
tools/bears-acp-adapter/
  src/
    main.rs
    acp/
    bearwire/
    config/
    den/
    doctor/
    tools/
```

A later crate split can happen when useful:

```text
crates/bears-acp-core
crates/bearwire-client
crates/bears-acp-cli
```

Do not make this refactor a blocker for the first app shell.

### CLI shell

The existing `bears-acp-adapter` binary remains the canonical CLI/runtime entry point.

It should support:

```text
bears-acp-adapter --help
bears-acp-adapter --version
bears-acp-adapter doctor
bears-acp-adapter doctor --json
bears-acp-adapter --check-config
bears-acp-adapter --check-server
```

Future app-friendly commands:

```text
bears-acp-adapter config get --json
bears-acp-adapter config set ...
bears-acp-adapter auth status --json
bears-acp-adapter clients list --json
bears-acp-adapter clients configure aizen
bears-acp-adapter logs path
```

The app should prefer `--json` commands rather than scraping human-readable text.

### macOS app shell

Add a native macOS app under:

```text
apps/apple/Bears/
```

Use a universal-first structure from the start. The macOS target is the first shipping target, but reusable SwiftUI views, view models, auth/session state, and Den API clients should live in shared Apple code so an iOS/iPadOS target can be added later without rewriting product logic.

The app should bundle or install the same signed adapter binary:

```text
Bears.app/Contents/Resources/bin/bears-acp-adapter
```

The app bundles the helper and installs/updates it into a stable user-local location on first launch:

```text
~/Library/Application Support/Bears/bin/bears-acp-adapter
```

The app-managed ACP client command should use the fully expanded absolute path, not `~`, for example:

```text
/Users/alice/Library/Application Support/Bears/bin/bears-acp-adapter
```

The app should not install or symlink into `/usr/local/bin`, because that commonly triggers an administrator authentication prompt even for admin users. Keep `/usr/local/bin/bears-acp-adapter` only for the separate `.pkg` / technical CLI install path.

## Config and secrets

### Config resolution order

The shared adapter should resolve config in this order:

1. explicit CLI args;
2. environment variables;
3. app-managed config file;
4. macOS Keychain token if configured;
5. diagnostic failure with `doctor` guidance.

Environment variables remain the primary devcontainer/Linux path:

```text
DEN_API_URL
DEN_BEAR_SLUG
DEN_TOKEN
DEN_TOKEN_ENV
BEARS_ACP_CLIENT
```

### App-managed config file

macOS app-managed config should live at:

```text
~/Library/Application Support/Bears/config.json
```

Cross-platform fallback for CLI/dev use:

```text
~/.config/bears/config.json
```

Example:

```json
{
  "den_api_url": "https://api.bears.example",
  "bear_slug": "bruno",
  "token_source": "keychain",
  "default_client": "aizen"
}
```

### Keychain

The macOS app should store Den tokens in Keychain.

Suggested identity:

```text
service: ai.bears.client
account: <profile-or-human-id>
```

The Swift app owns Keychain access. The Rust helper should not read Keychain directly in the initial architecture.

For app-managed launches or client auto-configuration, the app provides the token to the helper through the selected client configuration mechanism, typically environment variables or client-specific config generated by the app. Linux/devcontainer remains env/config based.

Do not store Den tokens in plain-text config files.

## App UX phases

### Phase 1 — setup companion with self-update

The first app should be intentionally small, but it must include app self-update. Keeping the client compatible with a rapidly evolving Den/BearWire server is an initial motivation for shipping an app rather than only a `.pkg`.

Use **Sparkle** for macOS app updates in Phase 1.

Screens/features:

- Welcome / install status;
- Den API URL;
- Bear selection or Bear slug;
- token/login setup;
- run `doctor`;
- ACP client setup instructions;
- copy adapter command;
- logs/help;
- update status / check for updates;
- update channel display, initially `beta` or equivalent.

Primary user flow:

```text
Open Bears.app
  → enter Den URL / token / Bear
  → run Doctor
  → configure aizen/Zed or copy command
  → open ACP client
```

No background daemon required.

Sparkle update requirements:

- appcast generation must be part of the macOS app release pipeline;
- app update signing keys must be distinct from Apple Developer ID certificates;
- release notes must include both app and bundled adapter versions;
- the app must re-check/install the bundled helper after an app update;
- the update channel should be explicit so beta users are not surprised by rapid updates.

### Phase 2 — client configuration helper

Add client-specific setup helpers.

Initial target order:

1. **Zed auto-configuration** — first because the external-agent config path is documented and already validated with the adapter.
2. **aizen guided/manual configuration** — provide copy/paste instructions first; only add auto-write once its config format and rollback behavior are verified.

Potential helpers:

- detect supported ACP client installation/config;
- generate copy/paste config;
- write config when safe and reversible;
- detect Zed settings and add/update the custom agent entry;
- validate configured command path;
- explain missing permissions/env vars.

Keep automatic config writes conservative and reversible.

### Phase 3 — BearWire desktop runtime

Once BearWire exists, the app can become a richer connected runtime.

Potential features:

- persistent Den connection while app is open;
- connected workspace registration;
- notifications;
- permission prompt surface;
- local runtime status;
- workspace/work-surface hints;
- diagnostic upload/export.

Still avoid LaunchAgent unless needed.

### Phase 4 — universal Apple client expansion

Before adding substantial macOS-only background infrastructure, identify which app features should also exist on iOS/iPadOS.

Universal Apple feature priority:

1. **Bear status/admin** — Bear list/detail/status, role provisioning state, Den connection health, membership/admin controls where authorized, and basic diagnostics.
2. **Lightweight chat/status** — a simple mobile-friendly Bear interaction/status surface after status/admin is useful.
3. **Notifications and review inbox** — task approvals, Reflection proposals, memory review items, and work handoffs once backend queues are mature.
4. **Remote runtime selection/control** — later, once BearWire connected runtimes exist.

Likely universal features:

- Den login/session management;
- Bear list/detail/status;
- Bear administration for authorized humans;
- provisioning/reconciliation status;
- memory/reflection/diagnostic summaries;
- lightweight chat/status;
- notifications and review inbox;
- remote workspace/runtime selection once BearWire runtimes exist.

macOS-only features:

- install/update local helper;
- run ACP stdio adapter;
- configure local ACP clients;
- read local logs;
- register local workspaces;
- local permission prompt surface for filesystem/process tools.

### Phase 5 — optional LaunchAgent/helper

Start with no LaunchAgent. Then use app-open helper management first. Promote the helper to a persistent LaunchAgent only when always-on behavior is needed.

Introduce a persistent helper only when the product needs background behavior such as:

- always-on BearWire connection;
- remote/mobile access to connected workspaces;
- background notifications;
- workspace presence without the app open.

If introduced, it must have:

- clear UI controls;
- uninstall path;
- token revocation path;
- bounded local capabilities;
- audit-friendly logs;
- signed/notarized helper binary.

## Bear administration in the app

The app may eventually present Bear administration features:

- create Bear;
- duplicate Bear;
- provision missing roles;
- view role health;
- manage membership;
- manage skills/MCP attachments;
- view Den/Bear diagnostics.

These remain Den control-plane operations.

Rules:

1. The app acts as an authenticated Den admin/operator client.
2. Den remains the system of record for Bears, membership, role agents, skills, MCP attachments, and provisioning state.
3. Admin operations use the same Den policy, validation, audit, and reconciliation paths as the browser operator console and admin JSON APIs.
4. BearWire may carry admin requests only under explicit admin/operator authorization.
5. Local runtime presence must never imply permission to administer a Bear.

## Versioning and update model

The app and adapter have separate versions.

```text
Bears.app version = product/UI/update version
bears-acp-adapter version = runtime/protocol/helper version
```

They may move together early, but equality must not be assumed.

Every app release declares the adapter version it bundles.

Example release metadata:

```json
{
  "app": {
    "version": "0.3.0",
    "build": "42",
    "git_sha": "appsha123"
  },
  "adapter": {
    "version": "0.5.2",
    "git_sha": "adaptersha456",
    "bearwire_protocol": 1
  },
  "compatibility": {
    "min_den_version": "0.8.0",
    "supported_bearwire_protocols": [1]
  }
}
```

Bundle this metadata in the app:

```text
Bears.app/Contents/Resources/bears-release.json
```

The app should display version diagnostics such as:

```text
Bears.app 0.3.0
Adapter 0.5.2 (adaptersha456)
Den 0.8.4 (densha789)
BearWire protocol 1
```

### App-managed helper update policy

For app-managed installs, the app owns:

```text
~/Library/Application Support/Bears/bin/bears-acp-adapter
```

On app launch and after every app update:

1. read bundled adapter version metadata;
2. read installed helper version using `bears-acp-adapter --version --json`;
3. if helper is missing or older than bundled helper, replace it automatically;
4. if helper is the same version, do nothing;
5. if helper is newer, keep it if protocol-compatible and warn only if incompatible.

The app-generated ACP client config should continue to use the user-local helper path. The app should not mutate `/usr/local/bin`.

### Compatibility checks

Compatibility should be explicit and capability/protocol based, not only semver equality.

The adapter should expose:

```text
bears-acp-adapter --version --json
```

with fields for:

- adapter version;
- git SHA;
- build time;
- supported BearWire protocol versions;
- supported local capabilities.

`doctor --json` should include Den compatibility information when reachable:

- Den version;
- Den git SHA;
- supported BearWire protocol versions;
- ACP gateway availability;
- auth/token status.

The app should surface mismatches clearly:

```text
Adapter too old for this Den.
Den too old for this adapter.
Installed helper is newer than bundled helper but compatible.
```

### Release tags and pipeline inputs

Use separate release identities:

```text
adapter/v0.5.2
bears-app/v0.3.0
```

The app release pipeline should take an explicit adapter version input or read a pinned file such as:

```text
apps/apple/Bears/ADAPTER_VERSION
```

The app workflow downloads or builds that adapter version, embeds it, writes `bears-release.json`, and signs/notarizes the result.

### Sparkle update policy

Sparkle is included in Phase 1 for macOS app self-update.

Initial policy:

- one beta update channel is acceptable;
- stable/beta/dev channels may be added later;
- Sparkle updates the app bundle;
- after Sparkle installs a new app version, the app updates the user-local helper from the bundled helper;
- Sparkle appcast entries must include app version, bundled adapter version, minimum Den version, and release notes;
- update checks should be user-visible and not silently disruptive during active ACP sessions.

Sparkle does not update `/usr/local/bin` or the standalone `.pkg` install. CLI-only users continue to update through `.pkg`/release artifacts.

## Packaging and distribution

### Keep the `.pkg`

Continue shipping the signed/notarized `.pkg` for CLI-only installs while the app matures.

Current package installs:

```text
/usr/local/bin/bears-acp-adapter
```

This remains useful for:

- technical users;
- CI/manual testing;
- fallback installs;
- non-app ACP client configuration.

### Add `.app` in `.dmg`

The macOS app should ship as:

```text
Bears.dmg
  Bears.app
```

The app bundle should include the adapter helper binary or download/install the matching signed helper.

Signing/notarization requirements:

- sign helper binary with Developer ID Application;
- sign app bundle;
- notarize app or `.dmg`;
- staple notarization ticket;
- keep release artifacts traceable to git SHA.

### CI separation

Keep workflows separate:

```text
.github/workflows/acp-adapter.yml
.github/workflows/macos-app.yml
```

`acp-adapter.yml`:

- builds Linux/devcontainer CLI;
- builds macOS helper CLI;
- signs/notarizes `.pkg`.

`macos-app.yml`:

- builds SwiftUI macOS app;
- embeds/downloads signed helper;
- writes `bears-release.json`;
- signs/notarizes app/dmg;
- signs Sparkle update artifact;
- generates or updates Sparkle appcast;
- uploads app artifact.

Future Apple client CI may add:

```text
.github/workflows/apple-client.yml
```

for shared Swift package tests and iOS/iPadOS builds that do not embed the macOS helper.

App build failures should not block Linux/devcontainer adapter releases unless explicitly configured for a release gate.

## Linux/devcontainer alignment

The Linux/devcontainer version stays aligned by depending on the same shared Rust core and test suite.

Linux build should not depend on:

- Swift;
- Xcode;
- Keychain APIs;
- LaunchServices;
- AppKit/SwiftUI;
- LaunchAgent plist logic;
- macOS signing/notarization steps.

macOS-only code should live in:

```text
apps/macos/Bears/
```

or under a macOS-specific target inside a future shared Apple app tree:

```text
apps/apple/Bears/macOS/
```

Shared SwiftUI/state code should avoid direct dependencies on helper processes, local filesystem privileges, and AppKit-only APIs so it can later compile for iOS/iPadOS.

Rust macOS-specific code should stay behind explicit `cfg(target_os = "macos")` feature gates if it must be in Rust.

The default devcontainer workflow should continue to build and test only the portable adapter/core.

## Testing strategy

### Shared core tests

Run on Linux and macOS:

- config resolution;
- `doctor --json` schema;
- Den URL validation;
- token-env behavior;
- ACP stdio request handling;
- BearWire client message encoding once implemented;
- local tool policy decisions.

### Apple app tests

Run shared Swift tests for app models, Den API clients, auth/session state, and view models without requiring a macOS helper.

Run macOS-specific tests on macOS CI:

- app builds;
- helper is embedded or discoverable;
- signed helper passes `codesign --verify`;
- config file read/write works in a temp home;
- `doctor --json` can be invoked from the app wrapper;
- Sparkle framework is present and configured;
- Sparkle update artifact/appcast can be generated on release builds;
- notarization succeeds on release builds.

Future iOS/iPadOS tests should verify that shared UI/state code builds without helper-process assumptions.

### Manual beta checklist

- Fresh install on Apple Silicon macOS;
- Gatekeeper accepts artifact;
- app opens without Terminal;
- user can enter config/token;
- `doctor` result is readable;
- Zed setup path works;
- aizen guided/manual setup is understandable;
- Sparkle detects and installs a beta update;
- after app update, helper is updated from the bundled helper;
- uninstall removes app/helper/config if user chooses;
- token can be revoked/removed.

## Security and privacy

- Store secrets in Keychain, not config files.
- Do not log tokens, full file contents, or unbounded command output.
- Local tool permissions remain enforced by Den policy plus local runtime/client checks.
- The app should show clearly when it can access local workspaces.
- Admin actions require Den admin/operator authorization.
- The helper should expose no unauthenticated local network control surface.
- If a local HTTP/IPC helper is added, bind to loopback by default and use bearer or OS-mediated authorization.
- Protect Sparkle signing keys separately from Apple Developer ID certificates.
- Do not store Sparkle private keys in the repository.
- Treat the appcast hosting location as release infrastructure; unauthorized appcast changes could steer client updates.

## Open questions

1. What app name should be used in Finder and releases: `Bears`, `BEARS`, or `Bears Client`?
2. Where should Sparkle appcasts be hosted for beta and future stable channels?
3. What update channels should exist initially: beta only, or beta + stable?
4. What exact Zed settings mutation strategy is safest: direct JSON edit, generated snippet, or user-confirmed patch preview?
5. What exact aizen configuration format and rollback behavior should the app support before enabling auto-configuration?
6. Which app features require online Den connectivity versus local-only status?
7. What backend API shape should the first iOS/iPadOS Bear status/admin screens use?
8. When does app-open helper management become insufficient and justify a LaunchAgent?

## Suggested implementation sequence

1. Add `doctor --json` to `bears-acp-adapter`.
2. Add app-managed config file support to the shared Rust adapter.
3. Add stable machine-readable config/auth status commands.
4. Add `--version --json` and release metadata needed by the app.
5. Create minimal SwiftUI app under `apps/apple/Bears/` with shared view models separated from macOS helper services.
6. Bundle the existing signed adapter helper and install/update it to `~/Library/Application Support/Bears/bin/` on first launch.
7. Add Sparkle to the macOS app and create beta appcast generation in CI.
8. Add Den URL/token/Bear config UI.
9. Run and render `doctor --json` in the app.
10. Add Zed auto-configuration and aizen guided/manual setup.
11. Add signing/notarization workflow for `.app`/`.dmg`.
12. Add shared Apple app tests that can later run for iOS/iPadOS.
13. Beta test app + existing `.pkg` side by side, including Sparkle update from one beta build to another.
14. Add BearWire desktop runtime features only after BearWire v1 is defined and stable.
15. Add iOS/iPadOS target only after the shared app model and Den APIs are stable enough to avoid duplicating product logic, starting with Bear status/admin, then lightweight chat/status, then notifications/review inbox.

## Related documents

- [BearWire protocol ADR](../architecture/adr/bearwire-protocol.md)
- [ACP Session Bindings ADR](../architecture/adr/acp-session-bindings.md)
- [ACP direct local tool runtime implementation plan](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md)
- [ACP Adapter Improvement Plan](ACP_ADAPTER_IMPROVEMENT_PLAN.md)
- [Phase 1 bootstrap plan](PHASE1_BOOTSTRAP.md)
- [Bears and Den](../concepts/BEARS_AND_DEN.md)
- [Identity and Membership](../concepts/IDENTITY_AND_MEMBERSHIP.md)
