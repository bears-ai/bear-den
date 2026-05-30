# Bears macOS app implementation plan

Status: proposed implementation plan.

This plan captures the current scope, implementation shape, and phased delivery path for a native macOS Bears app that distributes and updates the ACP adapter for end users.

It intentionally focuses on the macOS app as a packaging, install, update, diagnostics, and support shell around the adapter’s existing transactional CLI model. It does not move ACP protocol authority, BearWire authority, or Den administration authority into the app.

Related decision:

- [`../decisions/adr-0029-bears-macos-app-for-acp-adapter.md`](../decisions/adr-0029-bears-macos-app-for-acp-adapter.md)

## Current milestone

We have a settled phase-0 product direction for a first Bears app release:

1. Ship a **windowed SwiftUI macOS app**.
2. Bundle the ACP adapter inside the app.
3. Install and update the adapter into a **per-user Application Support** location.
4. Keep the adapter as a **transactional CLI** invoked directly by ACP clients.
5. Use **manual client configuration** in phase 0.
6. Use **Sparkle** for app self-update.
7. Keep **adapter version coupled to app version**.
8. Support **in-app ACP usage log viewing**, including filtering by client, bear, and session ID.
9. Use **direct distribution** rather than Mac App Store distribution.

## Product boundary

### User-facing goal

The app should make the ACP adapter simple to install and keep current on macOS without requiring:

- Terminal use;
- administrator credentials;
- writing into system-managed executable locations.

A user should be able to:

- install the app;
- open it and let it prepare the local adapter automatically;
- see whether the adapter is ready;
- copy the exact adapter path for manual ACP client setup;
- inspect app and adapter versions;
- inspect ACP usage logs;
- receive app updates via Sparkle.

### First-release non-goals

The first release does **not** include:

- app-driven ACP client auto-configuration;
- direct Den bear-admin features from the app;
- a continuously running local adapter service;
- iOS shipping support;
- Mac App Store distribution.

## Architecture direction

### Universal Apple direction without overcommitting phase 0

The first shipping target is macOS, but architectural choices should not block a future iOS app.

Implications:

- use **SwiftUI**;
- keep shared app/domain state separated from macOS-only services;
- isolate adapter installation, local filesystem management, and Sparkle integration behind macOS-specific service boundaries;
- avoid baking local helper assumptions into shared UI/view model code.

The future iOS app is expected to omit the local adapter while reusing shared app/domain and Den-facing layers where appropriate.

### App layering

Recommended structure:

```text
apps/apple/Bears/
  BearsApp/                 # macOS app target
  Shared/
    AppCore/                # models, state, view models, use cases, protocol seams
    UI/                     # reusable SwiftUI components where practical
  macOS/
    AdapterInstall/
    Logging/
    Updates/
    Diagnostics/
    Platform/
```

### Shared app responsibilities

Shared code should own:

- app/domain models;
- install/update state models;
- version and status models;
- log query/filter models;
- view models and use-case orchestration;
- protocol abstractions for platform-specific services.

Suggested protocol seams:

- `AdapterInstallationManaging`
- `AdapterVersionProviding`
- `AdapterPathProviding`
- `AcpUsageLogProviding`
- `AppUpdateManaging`
- `DiagnosticsProviding`

### macOS-specific responsibilities

macOS code should own:

- bundled adapter extraction/copy;
- Application Support path management;
- adapter validation and repair;
- app bundle resource inspection;
- Sparkle integration;
- local structured log discovery and indexing;
- Finder reveal / copy-path / platform diagnostics conveniences.

## Adapter runtime and install model

### Transactional CLI model remains authoritative

The adapter remains a CLI executable invoked by ACP clients on demand.

The app does **not** run a persistent adapter process in phase 0. It is not a broker and does not proxy ACP transactions. Its role is to make sure the adapter is present, current, inspectable, and supportable.

### Bundling and install location

The app should bundle the adapter binary as a resource, for example:

```text
Bears.app/Contents/Resources/bin/bears-acp-adapter
```

The app should install and run the adapter from a user-managed location under:

```text
~/Library/Application Support/Bears/
```

Do not execute the adapter directly from the app bundle as the primary operational path. Installing to Application Support gives us a writable, user-scoped location for replacement, diagnostics, and future repair flows without mutating the signed bundle.

### Managed filesystem layout

Recommended baseline layout:

```text
~/Library/Application Support/Bears/
  adapter/
    current/
      bears-acp-adapter
      VERSION
  logs/
    acp/
      usage.ndjson
      archives/
  config/
    app.json
  state/
    install-state.json
```

Preferred refinement:

```text
~/Library/Application Support/Bears/
  adapter/
    0.1.0/
      bears-acp-adapter
    current -> 0.1.0
  logs/
    acp/
      usage.ndjson
      archives/
  state/
    install-state.json
```

Recommendation:

- prefer **versioned adapter directories** plus a `current` symlink or pointer record;
- keep logs and state outside the versioned adapter directory;
- preserve logs and state when replacing binaries.

This gives us safer upgrades, easier reasoning, and clearer support state.

## Install and repair flow

### First-run install

On first launch the app should:

1. resolve Application Support paths;
2. create missing directories;
3. inspect the bundled adapter version;
4. copy/install the bundled adapter into the managed adapter location;
5. ensure executable permissions are correct;
6. write install-state metadata;
7. expose the installed adapter path in the UI.

### Ongoing install/update check

On subsequent launches, or on explicit refresh, the app should:

1. inspect bundled adapter version;
2. inspect installed adapter version;
3. compare versions;
4. replace or promote the installed adapter if the bundled version is newer, missing, or corrupt;
5. preserve logs and app state;
6. surface install or repair errors in the UI.

### Repair behavior

The app should detect and repair at least these cases:

- missing managed adapter binary;
- unreadable or non-executable managed binary;
- version mismatch;
- broken `current` pointer or install-state record;
- invalid or missing install metadata.

A one-click **Repair installation** action should reinstall the managed adapter from the bundled resource.

## Client configuration model

Phase 0 uses **manual ACP client configuration**.

The app should clearly show:

- the exact absolute installed adapter path;
- a copy button for that path;
- short manual setup guidance for supported ACP clients.

The app should not attempt auto-configuration in phase 0, but it should not structure the UI in a way that would prevent adding client-specific configuration flows later.

## Update model

### App updates via Sparkle

The app should use **Sparkle** for self-update.

This requires:

- Sparkle integration in the macOS app;
- appcast/signing pipeline;
- update settings or menu affordances;
- QA of clean install, update, and reinstall flows.

### Adapter updates coupled to app version

The adapter version should remain **coupled to the app version**.

Implications:

- each app release carries one intended adapter version;
- app updates deliver the adapter payload;
- adapter upgrades happen through the app install/update path;
- there is no separate adapter hotfix/update channel in phase 0.

This keeps the support model simple and makes compatibility easier to explain.

## Logging and diagnostics

### ACP usage logs

The app should expose **ACP usage logs** in-app.

Required segmentation/filtering:

- by client;
- by bear;
- by session ID.

Preferred implementation direction:

- the adapter emits structured usage logs, preferably **NDJSON**;
- log files live under `~/Library/Application Support/Bears/logs/acp/`;
- the app reads, indexes, and filters those logs locally.

Suggested minimum usage-log fields:

- timestamp;
- client identifier;
- bear slug or bear id;
- ACP session id;
- conversation id if available;
- invocation outcome or status;
- duration;
- error summary if any.

### App logging

The app itself can use normal macOS logging conventions.

Preferred direction:

- use **OSLog** / unified logging for app internals;
- keep ACP usage history in app-readable structured files rather than relying solely on unified logging.

### Diagnostics UI

Phase 0 diagnostics should include:

- installed adapter path;
- app version;
- bundled adapter version;
- installed adapter version;
- last install/update result;
- access to ACP usage logs;
- a copyable diagnostic summary.

## UI shape

Use a **windowed SwiftUI app**.

### Main views

#### Overview
Show:

- installation state / readiness;
- installed adapter path;
- app version;
- adapter version;
- last install or repair event;
- copy adapter path action;
- repair installation action.

#### Client setup
Show:

- short explanation that client configuration is manual in phase 0;
- exact adapter path;
- copy action;
- client-specific setup notes if available.

#### ACP logs
Show:

- list or table of usage entries;
- filters for client, bear, and session ID;
- date/time and status summary;
- detail view for an individual usage record.

#### Settings / Updates
Show:

- Sparkle update controls or preferences;
- app version and update status;
- diagnostic actions.

### Nice-to-have support UX

- reveal log folder in Finder;
- export filtered ACP log slices;
- copied-path confirmation;
- copy diagnostic summary.

## Distribution and trust model

The app must:

- require no administrator privileges;
- require no Terminal use;
- avoid writes into protected system locations;
- keep managed runtime files in the user domain only;
- be signed and notarized for direct distribution.

The app should also:

- avoid placing secrets in ordinary logs;
- redact sensitive fields from usage logs where needed;
- store any future Den credentials in Keychain rather than plain config files.

## Current design direction

### Keep the app narrow in phase 0

The app should focus on:

- install;
- update;
- status;
- log inspection;
- diagnostics;
- manual setup guidance.

Do not prematurely expand phase 0 into Den administration, runtime brokering, or client auto-configuration.

### Treat adapter installation and observability as the main app value

The strongest early value is reducing user friction and support friction:

- users do not need Terminal instructions;
- the adapter has a stable user-local path;
- support has a log viewer and clear version/install state;
- updates are understandable and unified.

## Implementation phases

### 1. Foundation and shell

Goal:

Get a minimal macOS app running that installs the adapter and shows status.

Deliverables:

- SwiftUI macOS app target;
- shared AppCore models and protocol seams;
- bundled adapter resource;
- Application Support path manager;
- first-run install flow;
- status UI with copyable adapter path.

Acceptance checks:

- a fresh user can install and open the app;
- the app installs the adapter into user Application Support without admin prompts;
- the app shows the exact installed adapter path;
- a user can manually configure an ACP client using that path.

### 2. Durable install/update lifecycle

Goal:

Make the managed adapter install durable, version-aware, and repairable.

Deliverables:

- bundled-vs-installed version comparison;
- versioned adapter directory layout;
- replace/promote logic for new adapter versions;
- repair installation action;
- install metadata/state tracking;
- corruption and missing-file recovery behavior.

Acceptance checks:

- app upgrades replace or promote adapter versions cleanly;
- a missing or damaged managed adapter can be repaired from the UI;
- logs and state survive adapter replacement.

### 3. ACP usage log pipeline and viewer

Goal:

Make adapter usage visible and supportable from inside the app.

Deliverables:

- structured ACP usage log format;
- log writer location and archive/rotation policy;
- in-app ACP log viewer;
- filters for client, bear, and session ID;
- export or copy diagnostics support.

Acceptance checks:

- adapter invocations produce structured usage logs;
- the app can filter logs by client, bear, and session ID;
- support can inspect a user-provided log slice without Terminal steps.

### 4. Sparkle integration and release pipeline

Goal:

Ship updateable direct-distributed app releases.

Deliverables:

- Sparkle integration;
- update controls and preferences;
- appcast/signing configuration;
- release packaging notes;
- notarization and update validation.

Acceptance checks:

- the installed app can discover and apply a Sparkle update;
- upgrading the app refreshes the adapter payload as expected;
- install and update flows work on non-developer machines.

### 5. Support polish

Goal:

Make the app understandable and supportable for non-technical users.

Deliverables:

- refined overview/setup/logs/settings UX;
- copyable diagnostic summary;
- Finder reveal actions for logs/runtime files;
- final user-facing manual client setup guidance;
- QA pass over install, repair, and update journeys.

Acceptance checks:

- a non-technical user can get the adapter installed and manually configured using only app guidance plus client-specific instructions;
- common install and repair failures are understandable from the UI.

## Subsystem work breakdown

### App shell

- create SwiftUI app target and navigation structure;
- define app state container;
- build Overview, Client Setup, Logs, and Settings views.

### Install manager

- resolve Application Support paths;
- copy bundled binary;
- set permissions;
- compare versions;
- support repair and replace operations;
- record install metadata.

### Adapter/app contract

- define how the app reads adapter version/build metadata;
- define structured ACP usage log schema;
- add any minimal CLI surfaces needed for version or status introspection.

### Logs

- choose NDJSON or equivalent format;
- implement archive/rotation policy;
- parse, index, and filter log entries in app;
- support export and copy summary.

### Updates and release engineering

- integrate Sparkle;
- build appcast pipeline;
- sign and notarize app;
- test upgrade paths.

## Risks and mitigations

### Install/update complexity around copied binaries

Mitigation:

- keep adapter version coupled to app version;
- use versioned install directories;
- add explicit repair flow.

### Manual client configuration may still confuse users

Mitigation:

- show the exact installed path prominently;
- provide copy action and concise setup notes;
- later add client-specific automation only after the base path is solid.

### ACP logs may capture too much sensitive context

Mitigation:

- log metadata and outcomes by default, not full sensitive payloads;
- redact tokens and secrets;
- review schema before making logs durable.

### Architecture may drift into macOS-only assumptions

Mitigation:

- keep shared AppCore free of install and filesystem assumptions where practical;
- isolate Sparkle, local paths, and install logic in macOS services.

## Open questions

- Should the stable exposed adapter path be `.../adapter/current/bears-acp-adapter` or a flatter stable path?
- Should version selection use symlinks or an install-state pointer file?
- What exact CLI/API surface should the adapter expose for version/build metadata?
- What is the final ACP usage log schema and retention period?
- Should the app maintain a lightweight recent-activity summary in addition to raw logs?

## Suggested immediate next steps

1. Create the app target and directory structure under `apps/apple/Bears/`.
2. Define the managed filesystem layout and install-state format.
3. Add adapter version introspection and structured usage-log requirements on the adapter side.
4. Implement the phase-1 install flow and status UI.
5. Integrate Sparkle once the base app shell is functional.
