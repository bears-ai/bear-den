# Managed adapter install contract

Status: phase-0 working contract.

This document defines the initial managed install layout for the Bears macOS app.

It is intentionally optimized for the first execution slice:

- a Bears-owned system adapter installation path;
- per-user writable app metadata and logs;
- stable path for manual ACP client configuration;
- enough metadata to support install, display, and repair behavior;
- alignment with the notarized macOS package install location.

## Goals

The managed install contract must provide:

1. a stable executable path that users can paste into ACP client configuration;
2. a stable Bears-owned adapter install area plus per-user writable app metadata under Application Support;
3. a place to persist install metadata independently of the app bundle;
4. a path shape that can grow into versioned installs later if needed.

## Contract

### Per-user Application Support root

The Bears app stores user-specific metadata and logs under:

```text
~/Library/Application Support/Bears/
```

### Phase-0 adapter install root

The system adapter install root is:

```text
/Library/Application Support/Bears/adapter/
```

### Stable executable path

For phase 0, the app should expose this stable executable path to users and ACP clients:

```text
/Library/Application Support/Bears/adapter/bears-acp-adapter
```

This is the path the app should show in the UI and copy to the clipboard for manual client setup.

### Install-state file

Install metadata lives at:

```text
~/Library/Application Support/Bears/state/install-state.json
```

### Logs root

ACP usage logs should eventually live at:

```text
~/Library/Application Support/Bears/logs/acp/
```

This is not required to be fully implemented in the first slice, but the path is reserved now so later slices do not need to redesign the layout.

## Initial install-state shape

Phase-0 install metadata should be simple JSON with enough information for display and repair flows.

Suggested shape:

```json
{
  "schema_version": 1,
  "managed_adapter_path": "/Library/Application Support/Bears/adapter/bears-acp-adapter",
  "installed_version": "0.1.5",
  "bundled_version": "0.1.5",
  "installed_at": "2026-05-30T10:00:00Z",
  "last_install_status": "ok",
  "last_error": null
}
```

### Field meanings

- `schema_version` — version of the install-state file shape
- `managed_adapter_path` — absolute path users should configure into ACP clients
- `installed_version` — version read from the managed adapter copy
- `bundled_version` — version of the adapter bundled in the app at the time of install or repair
- `installed_at` — UTC timestamp of the last successful install or reinstall
- `last_install_status` — simple status such as `ok`, `missing`, `repair_needed`, or `error`
- `last_error` — most recent install or repair error summary, or `null`

## Phase-0 behavior rules

### First run

On first launch the app should:

1. create `/Library/Application Support/Bears/adapter/` if missing;
2. create `~/Library/Application Support/Bears/state/` if missing;
3. copy the bundled adapter to the stable executable path;
4. ensure the file is executable;
5. write `install-state.json`.

### Normal startup check

On later launches the app should:

1. check whether the managed adapter exists at the stable path;
2. check whether it appears executable;
3. compare bundled and installed versions if available;
4. update install-state metadata;
5. offer repair if the managed adapter is missing, invalid, or out of date.

### Repair

A repair action should:

1. replace the managed adapter at the stable path with the bundled adapter;
2. restore executable permissions;
3. refresh `install-state.json`;
4. preserve unrelated state and future log files.

## Deferred refinement

This contract intentionally keeps the executable path flat in phase 0.

A later slice may switch the internal storage layout to:

```text
~/Library/Application Support/Bears/adapter/<version>/bears-acp-adapter
```

while still preserving a stable exposed path via either:

- a `current` symlink; or
- a copied/promoted stable executable path.

That refinement is deferred until we need more sophisticated upgrade or rollback behavior.

## Current decision

For the first execution slice, prefer:

- a **flat stable executable path**;
- a **simple JSON install-state file**;
- minimal metadata sufficient for UI display and repair logic.
