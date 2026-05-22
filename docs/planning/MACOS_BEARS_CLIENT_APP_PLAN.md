# macOS Bears client app implementation plan

Status: proposed implementation plan.

## Goal

Evolve the macOS distribution from a signed `.pkg` that installs `bears-acp-adapter` into a native **Bears client app** while keeping the Linux/devcontainer adapter lean, testable, and free of macOS UI specifics.

The app should improve onboarding for non-technical macOS users without moving protocol authority, Bear provisioning authority, or runtime policy out of Den.

## Summary decision

Build a native **SwiftUI macOS app** as a platform shell around the existing shared Rust adapter/runtime core.

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

The macOS app owns:

- native onboarding UI;
- token/login setup;
- Keychain integration;
- local config editing;
- running `doctor` and showing results;
- configuring supported ACP clients when possible;
- showing logs/status;
- installing/updating the helper CLI;
- future BearWire desktop runtime features.

## Non-goals

- Do not put ACP/BearWire protocol logic primarily in Swift.
- Do not make the macOS app required for Linux/devcontainer use.
- Do not add macOS frameworks to the Linux adapter build.
- Do not make local runtime access imply Bear administration authority.
- Do not replace Den admin APIs or the Den operator console.
- Do not introduce a persistent LaunchAgent until there is a clear need.
- Do not turn the app into an external agent protocol surface; A2A remains the future external agent interoperability path.

## Architecture

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

Add a native macOS app, preferably under:

```text
apps/macos/Bears/
```

The app should bundle or install the same signed adapter binary:

```text
Bears.app/Contents/Resources/bin/bears-acp-adapter
```

The app can either run the bundled helper directly or install/symlink it into a stable location.

Preferred user-local helper location:

```text
~/Library/Application Support/Bears/bin/bears-acp-adapter
```

Keep `/usr/local/bin/bears-acp-adapter` support for the current `.pkg` path and technical users.

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
BEARS_DEN_API_URL
BEARS_BEAR_SLUG
BEARS_DEN_TOKEN
BEARS_DEN_TOKEN_ENV
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

Initially, the Swift app may read from Keychain and pass the token to the helper process through environment variables. Later, the Rust CLI can learn to read Keychain directly behind a macOS-only feature flag if that improves UX.

Do not store Den tokens in plain-text config files.

## App UX phases

### Phase 1 — setup companion

The first app should be intentionally small.

Screens:

- Welcome / install status;
- Den API URL;
- Bear selection or Bear slug;
- token/login setup;
- run `doctor`;
- ACP client setup instructions;
- copy adapter command;
- logs/help.

Primary user flow:

```text
Open Bears.app
  → enter Den URL / token / Bear
  → run Doctor
  → configure aizen/Zed or copy command
  → open ACP client
```

No background daemon required.

### Phase 2 — client configuration helper

Add client-specific setup helpers.

Potential helpers:

- detect aizen installation/config;
- generate copy/paste config;
- write config when safe and reversible;
- detect Zed settings and show custom agent snippet;
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

### Phase 4 — optional LaunchAgent/helper

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

- builds SwiftUI app;
- embeds/downloads signed helper;
- signs/notarizes app/dmg;
- uploads app artifact.

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

or behind explicit `cfg(target_os = "macos")` feature gates if it must be in Rust.

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

### macOS app tests

Run on macOS CI:

- app builds;
- helper is embedded or discoverable;
- signed helper passes `codesign --verify`;
- config file read/write works in a temp home;
- `doctor --json` can be invoked from the app wrapper;
- notarization succeeds on release builds.

### Manual beta checklist

- Fresh install on Apple Silicon macOS;
- Gatekeeper accepts artifact;
- app opens without Terminal;
- user can enter config/token;
- `doctor` result is readable;
- aizen/Zed setup path works;
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

## Open questions

1. Should the first SwiftUI app bundle the helper binary only, or install it into `~/Library/Application Support/Bears/bin/` on first launch?
2. Should `/usr/local/bin/bears-acp-adapter` remain the recommended ACP client command once the app exists, or should app-managed installs prefer the user-local helper path?
3. Should the Swift app own Keychain access permanently, or should the Rust helper learn macOS Keychain access behind a feature flag?
4. Which ACP clients should receive automatic configuration first: aizen, Zed, or both?
5. What app name should be used in Finder and releases: `Bears`, `BEARS`, or `Bears Client`?
6. When does the app need a LaunchAgent rather than app-open-only runtime management?

## Suggested implementation sequence

1. Add `doctor --json` to `bears-acp-adapter`.
2. Add app-managed config file support to the shared Rust adapter.
3. Add stable machine-readable config/auth status commands.
4. Create minimal SwiftUI app under `apps/macos/Bears/`.
5. Bundle or install the existing signed adapter helper.
6. Add Den URL/token/Bear config UI.
7. Run and render `doctor --json` in the app.
8. Add client setup guidance for aizen and Zed.
9. Add signing/notarization workflow for `.app`/`.dmg`.
10. Beta test app + existing `.pkg` side by side.
11. Add BearWire desktop runtime features only after BearWire v1 is defined and stable.

## Related documents

- [BearWire protocol ADR](../architecture/adr/bearwire-protocol.md)
- [ACP Session Bindings ADR](../architecture/adr/acp-session-bindings.md)
- [ACP direct local tool runtime implementation plan](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md)
- [ACP Adapter Improvement Plan](ACP_ADAPTER_IMPROVEMENT_PLAN.md)
- [Phase 1 bootstrap plan](PHASE1_BOOTSTRAP.md)
- [Bears and Den](../concepts/BEARS_AND_DEN.md)
- [Identity and Membership](../concepts/IDENTITY_AND_MEMBERSHIP.md)
