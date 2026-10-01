# ADR-0049: ACP tool-call and permission UX semantics

**Status:** Proposed  
**Date:** 2026-07-04  
**Deciders:** Hans

**Related:**

- [ADR-0043: ACP Is an Edge Adapter; the Den Runtime Is Protocol-Agnostic](adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md)
- [ADR-0048: Core turn/client-obligation coordinator](adr-0048-core-turn-client-obligation-coordinator.md)
- [ADR-0025: Tool naming and execution strategy](adr-0025-tool-naming-and-execution-strategy.md)
- [ACP Runtime Contract](../architecture/acp-runtime-contract.md)
- [Hat-owned tool/network access policy](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending)
- [Chat UX guidelines](../guides/chat-ux-guidelines.md)
- [Agent Client Protocol: Tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls)

**Hat-qualified scope target (accepted direction 2026-10-01; partial Den web-fetch implementation):** A remembered tool or network permission is a Den-owned grant for the currently verified hat plus the narrower action/target scope shown to the approver. Interactive prompts offer `Just this time` (one exact obligation, no reusable approval) and, to an authorized hat-policy manager, `Always for [hat]` (including future eligible Job runs wearing it). Non-managers may request a persistent hat grant but cannot create one. Workspace, directory, command, account, and host constraints remain intersections, not alternate Bear-wide authorizations. This qualifies the unscoped examples below; the [ACP UX spec](../architecture/acp-tool-call-permission-ux-spec.md) gives target copy. The exact one-shot Den web-fetch boundary and an admin-only ACP `Always for [hat]` exact-HTTPS-host choice for Den-hosted web fetch are implemented. The latter stores both the descriptor tool and host in Den, affects later authorized conversations, and does not create a client cache entry. Persistent local-tool scopes, other network actions, dynamic Job egress revocation, and full profile-role retirement remain outside this implemented slice.

## Context

ACP tool calls and permission requests are now materially more reliable, especially on the BearWire and Den-native path. The remaining problems are primarily user-experience and policy problems rather than transport correctness problems.

Observed issues include:

- permission prompts using the wrong operation label, such as "fetch" for unrelated actions;
- inconsistent or unintuitive permission scope offerings, especially when workspace-scoped approval would be sensible;
- tool activity titles that expose implementation details rather than user-meaningful actions;
- divergence between the semantics used for tool execution updates and the semantics used for permission prompts;
- underuse of ACP's richer presentation surface, including action kind, locations, diffs, content, and structured permission options.

These failures are not just copy bugs. They indicate that the product lacks a durable, descriptor-owned semantic layer for presenting tool activity and permission policy to humans.

ADR-0043 established that ACP belongs at the edge. This ADR extends that direction: ACP projection remains edge-owned, but the semantics projected to humans must be stable, intentional, and shared across tool updates and approval UX.

## Decision

Bear Den will treat ACP tool-call and permission UX as a first-class semantic surface rather than a thin transport projection.

The system will adopt the following principles.

### 1. User-facing action semantics are canonical

Tool and permission UI must describe the action being attempted from the user's perspective, not the transport, adapter, or provider implementation detail.

Prefer action phrases such as:

- read file;
- search workspace;
- edit file;
- delete file;
- run command;
- fetch URL;
- open browser page.

Avoid exposing raw provider names, generic placeholders, or transport-specific labels such as `tool`, `local_tool`, or unrelated action words such as `fetch` when the operation is not a fetch.

### 2. One semantic model must drive both tool updates and permission prompts

The same descriptor-owned presentation model must power:

- initial tool-call creation;
- tool-call progress updates;
- tool-call completion and failure summaries;
- permission request titles, bodies, and option labels.

The product must not maintain one user-facing vocabulary for running tools and another for approvals.

### 3. Permission prompts must answer action, target, and scope

Every permission request should make three things immediately clear:

- what action is being requested;
- what target will be affected;
- what approval scopes are available.

If any of these are missing, the prompt is incomplete even if the protocol exchange is valid.

### 4. Approval scopes should follow user mental models

Remembered permission choices should be offered in scopes the user can naturally reason about, including where applicable:

- only this time;
- this directory;
- this workspace;
- this host;
- this exact command in this workspace;
- this safe command family in this workspace;
- always for the selected hat within an explicit action/target scope.

Scope availability should not be an accidental consequence of the raw event shape. It should be determined by explicit product policy per action family.

### 5. Workspace-scoped approval is a normal case for trusted local work

For workspace-bounded armature actions such as reading files, searching files, many file edits, and git-read operations, "always for [hat] in this workspace" should be a common and expected option.

It should not be treated as a rare special case.

### 6. ACP tool kinds are product semantics, not just icon hints

ACP `kind` values help clients render activity, but Den and armature code must assign them according to user-meaningful operation semantics.

Where ACP's built-in kinds are coarse, Bear Den may preserve a richer internal action taxonomy and project it into ACP while still presenting accurate user copy. For example, browser navigation should not be described to the user as a generic fetch merely because it involves network access.

### 7. Titles should be concrete, stable, and target-first

Tool and permission titles should prefer concise, target-specific phrasing such as:

- `Read src/main.rs`
- `Search for "request_permission" in this workspace`
- `Run cargo test -p den-bearwire`
- `Open https://example.com`

Titles should remain semantically stable across pending, running, completed, and failed states.

### 8. Detail should be progressive

The compact view should be understandable in one line.

Expanded views may include structured context such as:

- arguments summary;
- target path or URL;
- working directory;
- timeout or output limits;
- affected locations;
- diffs for edits;
- raw input and raw output for diagnostics.

Raw structured payloads are secondary diagnostics, not the primary explanation.

### 9. Consent quality improves when previews are available

When a safe preview materially improves the user's ability to approve or deny an action, the client should surface it.

Examples:

- diffs for edits;
- exact paths for destructive actions;
- command line, cwd, and execution limits for command execution;
- URL and host for network or browser actions.

Blind approvals should be avoided when the product already has enough information to preview the action.

### 10. Permission copy must describe future policy, not internal flags

Remembered approval options should be phrased as the policy they create, for example:

- `Always for Security Review: read files in this workspace`
- `Always for Security Review: open pages on github.com`
- `Always for Security Review: run this command in this workspace`

Avoid leaking internal identifiers such as `allow_workspace` or generic labels such as `Allow always` without scope context.

### 11. Destructive and high-risk actions require clearer framing

The product should distinguish between low-risk repeated local work and higher-risk actions such as command execution, deletion, broader egress, or externally visible side effects.

Higher-risk actions may still offer remembered scopes, but those scopes should be intentionally selected and more carefully worded.

### 12. Plan approval remains distinct from tool approval

Implementation-plan approval is a different user commitment from approving an individual tool call. It should remain a distinct UX path with its own titles, content, and options rather than being collapsed into ordinary tool permission UX.

## Rationale

ACP already provides enough protocol surface to support strong UX:

- `title` for concise action naming;
- `kind` for coarse action category;
- `status` for lifecycle state;
- `content` for explanations and previews;
- `locations` for follow-along behavior;
- `rawInput` and `rawOutput` for diagnostics;
- structured permission options for remembered policy choices.

The current gap is therefore not lack of protocol expressiveness. The gap is lack of a stable semantic presentation layer that intentionally maps internal tool operations into human-meaningful actions and scopes.

OpenCode's ACP implementation is useful inspiration here because it centralizes more of the tool-call mapping and permission projection. Bear Den should adopt the same general discipline without copying OpenCode's specific wording or simplifying assumptions.

## Consequences

### Positive

- Permission prompts become more trustworthy because they name the real action.
- Workspace-scoped and other remembered approvals become predictable rather than incidental.
- Tool execution updates and permission requests share one vocabulary and one classification model.
- ACP clients can render richer activity with less guesswork.
- Future channels and armatures can reuse the same semantic model even if they do not use ACP directly.

### Costs

- Requires introducing or strengthening a descriptor-owned presentation layer for client tools and permission requests.
- Requires revisiting existing tool classifications, titles, and permission-scope policy.
- Requires migration of ad hoc wording and legacy branches that currently hardcode operation copy.
- Requires tests that validate semantic projection, not just protocol correctness.

## Non-goals

- Do not move ACP framing or client-specific rendering policy into Den core.
- Do not eliminate BearWire or armature-local ownership of permission UI.
- Do not require every client to render every optional detail the same way.
- Do not expose raw protocol payloads as the primary user-facing explanation.

## Review checklist

When adding or modifying ACP tool-call or permission UX:

1. Does the visible action name describe the user's operation rather than the implementation detail?
2. Is the same semantic action model used for both tool progress and permission prompts?
3. Does the permission prompt make the action, target, and scope clear?
4. Is workspace-scoped approval available when the action is naturally workspace-bounded?
5. Are higher-risk actions classified and worded more carefully than low-risk local actions?
6. Are previews surfaced when they materially improve user consent?
7. Are raw input/output treated as diagnostics rather than the primary explanation?
8. Are plan approvals kept distinct from ordinary tool-call approvals?
