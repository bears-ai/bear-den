# ACP Tool-Call And Permission UX Spec

## Purpose

This document turns [ADR-0049](../decisions/adr-0049-acp-tool-call-and-permission-ux.md) into a concrete redesign spec for ACP tool-call presentation and permission UX.

It defines:

- the user-facing action taxonomy;
- approval scope policy by action family;
- copy and state rules for tool activity and permission prompts;
- data requirements for the presentation layer.

This is a product and edge-projection spec. It does not move ACP semantics into Den core. The [hat access plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-accepted-target-not-deployed) owns the intended grant policy; this document specifies how the armature presents it. Current armature remembered local-tool approvals are not yet hat-scoped. For URL-targeted actions it offers only `Just this time` or denial; old host/account/global URL cache entries are not reused, and stale host/account selections are rejected. Den-hosted web fetch consumes an exact one-shot runtime approval without creating a remembered Bear grant; Bear admins can separately configure Den-owned hat web grants, but the ACP persistent hat option is not deployed.

## Goals

- Present tool activity in user-meaningful terms.
- Make permission requests understandable without exposing implementation jargon.
- Make remembered approval scopes predictable.
- Reuse one semantic model across tool updates and permission prompts.
- Use ACP's tool-call surface more completely where it improves trust and follow-along behavior.

## Non-goals

- Define the underlying turn-obligation state machine; see ADR-0048.
- Standardize every client's visual layout.
- Replace ACP protocol shapes.
- Finalize exact strings for every locale.

## Canonical presentation model

Each tool or permissionable action should project into one presentation descriptor.

Required fields:

- `action_family`: one of the canonical user-facing families below
- `title`: concise action title
- `progress_verb`: short running-state verb phrase
- `permission_operation`: human-readable approval phrase
- `target_summary`: primary path, URL, host, command, or other target summary
- `risk_class`: coarse risk grouping
- `eligible_scopes`: approval scopes allowed for this action
- `locations`: affected files or other follow-along locations when relevant
- `preview`: optional structured preview such as diff, command context, or URL context
- `raw_input`: structured diagnostic input
- `raw_output`: structured diagnostic output or result summary

Optional fields:

- `subtitle`
- `category`
- `arguments_summary`

## Action taxonomy

The product should classify actions using this canonical taxonomy before projecting into ACP `kind` values.

| Action family | Description | Preferred ACP kind | Typical targets |
|------|----------|--------------------|-----------------|
| `read` | Read file or structured local content | `read` | file, directory listing, metadata |
| `search` | Search workspace or indexed content | `search` | query, glob, path root |
| `edit` | Create or modify content | `edit` | file, patch set |
| `delete` | Remove content | `delete` | file, directory |
| `move` | Rename or relocate content | `move` | source and destination paths |
| `execute` | Run a command, process, or terminal action | `execute` | command, cwd |
| `fetch` | Retrieve external data directly | `fetch` | URL, host, query |
| `browse` | Open or inspect content through a browser surface | `fetch` in ACP, but user copy stays browser-specific | URL, host, tab/page context |
| `plan` | Request or review an implementation plan | `think` or client-specific special treatment | plan artifact, plan id |
| `other` | Fallback for unmapped actions | `other` | n/a |

### Classification notes

- `browse` is intentionally distinct from `fetch` in Bear Den's internal semantics even though ACP does not provide a separate browser kind.
- Browser navigation or inspection should use browser-specific copy such as `Open page` or `Inspect page`, not `Fetch`.
- Placeholder tool names should be normalized into one of these families whenever arguments permit.
- The current split between `process_run` and `terminal_run_command` is an execution-detail split. The preferred long-term model-facing abstraction is a single command tool such as `run_command`, with routing performed by policy.

### Preferred future command abstraction

The product should converge toward one model-facing command tool:

```text
run_command
```

Routing policy should then decide whether the request becomes:

- a dedicated-tool redirect,
- `process_run`,
- or `terminal_run_command`.

Routing defaults:

1. redirect to a dedicated tool when one clearly fits the requested operation;
2. use `process_run` for short, bounded, structured commands;
3. use `terminal_run_command` for high-output, long-running, or user-visible commands;
4. default unknown commands in interactive armature flows to `terminal_run_command`.

The dedicated-tool redirect should be a soft wall: first redirect, then require an explicit override to force command execution when truly necessary.

Initial soft-wall examples:

- `rg` / `grep` should redirect to `fs_search_files` for workspace text search.
- `sed` should redirect to a dedicated edit tool only when the requested operation is clearly representable as a structured text edit.

To reduce reliance on redirects alone, model-facing direct-tool descriptors should also carry concise intent hints, including preferred use instead of common shell fallbacks such as `rg`, `grep`, and targeted `sed` replacements.

## Initial tool mapping targets

This table defines the intended near-term mapping for common current tools.

| Current tool | Action family | Default title pattern | Risk class |
|------|----------|-----------------------|------------|
| `fs_read_text_file` | `read` | `Read {path}` | `workspace_read` |
| `fs_list_directory` | `read` | `List {path}` | `workspace_read` |
| `fs_stat` | `read` | `Inspect {path}` | `workspace_read` |
| `fs_search_files` | `search` | `Search for "{query}"` | `workspace_read` |
| `fs_find_paths` | `search` | `Find {pattern}` | `workspace_read` |
| `fs_edit_file` | `edit` | `Edit {path}` | `workspace_write` |
| `fs_replace_text` | `edit` | `Edit {path}` | `workspace_write` |
| `fs_create_text_file` | `edit` | `Create {path}` | `workspace_write` |
| `fs_create_directory` | `edit` | `Create directory {path}` | `workspace_write` |
| `fs_copy_path` | `move` | `Copy {source}` | `workspace_write` |
| `fs_move_path` | `move` | `Move {source}` | `workspace_write` |
| `fs_apply_patch` | `edit` | `Apply patch` or `Edit {count} files` | `workspace_write` |
| `fs_delete_path` | `delete` | `Delete {path}` | `workspace_delete` |
| `git_status` | `read` | `Check git status` | `workspace_read` |
| `git_diff` | `read` | `View git diff` | `workspace_read` |
| `git_log` | `read` | `View git history` | `workspace_read` |
| `git_show` | `read` | `Inspect git object` | `workspace_read` |
| `git_blame` | `read` | `Blame {path}` | `workspace_read` |
| `git_add` | `edit` | `Stage changes` | `workspace_write` |
| `git_restore` | `edit` | `Restore changes` | `workspace_write` |
| `git_commit` | `edit` | `Create git commit` | `workspace_write` |
| `git_stash` | `edit` | `Stash changes` | `workspace_write` |
| `process_run` | `execute` | `Run {command}` | `process_execute` |
| `terminal_run_command` | `execute` | `Run {command}` | `process_execute` |
| `web_fetch` | `fetch` | `Fetch {url}` | `network_fetch` |
| `web_search` | `fetch` | `Search the web for "{query}"` | `network_fetch` |
| `http_request` | `fetch` | `Request {url}` | `network_fetch` |
| `chrome_open` | `browse` | `Open {url}` | `network_fetch` |
| `chrome_snapshot` | `browse` | `Inspect page snapshot` | `network_fetch` |
| `chrome_console_messages` | `browse` | `Read page console` | `network_fetch` |
| `chrome_network_requests` | `browse` | `Inspect page network activity` | `network_fetch` |
| `chrome_screenshot` | `browse` | `Capture page screenshot` | `network_fetch` |

## Approval scope policy

Approval scopes are product policy, not merely cache implementation details.

Canonical scopes:

- `once`
- `directory`
- `workspace`
- `site_account`
- `host`
- `command_exact_workspace`
- `command_family_workspace`
- `hat_wide` (an action family within one hat, subject to all other hard bounds)

### Scope matrix by action family

| Action family | Once | Directory | Workspace | Site account | Host | Command exact in workspace | Command family in workspace | Hat-wide action |
|------|------|-----------|-----------|--------------|------|-----------------------------|-----------------------------|--------|
| `read` | yes | yes | yes | no | no | no | no | yes |
| `search` | yes | yes when path-bounded | yes | no | no | no | no | yes |
| `edit` | yes | yes when path-bounded | yes | no | no | no | no | yes |
| `delete` | yes | yes when path-bounded | yes | no | no | no | no | cautious yes |
| `move` | yes | yes when path-bounded | yes | no | no | no | no | cautious yes |
| `execute` | yes | no | limited yes | no | no | yes | yes when safe family exists | no arbitrary commands |
| `fetch` | yes | no | optional product policy only if tied to workspace context | known-site accounts only | yes | no | no | no arbitrary hosts |
| `browse` | yes | no | optional product policy only if tied to workspace context | known-site accounts only | yes | no | no | no arbitrary hosts |
| `plan` | once-only by default | no | no | no | no | no | no | no |
| `other` | yes | case by case | case by case | case by case | case by case | no | no | only with a descriptor-owned bounded target |

### Scope policy rules

1. `workspace` should be a normal option for workspace-bounded local actions.
2. `directory` should only appear when the target is meaningfully narrower than the workspace root.
3. `site_account` should be offered for supported known sites when the target can be normalized to a stable trust boundary narrower than host and broader than a single URL.
4. `host` should be the sticky network/browser scope for sites outside the known-site set.
5. Exact-URL remembered approval should not be offered.
6. `command_exact_workspace` should be available for command execution when a stable command string exists.
7. `command_family_workspace` should only appear for explicitly whitelisted safe command families.
8. Every remembered scope is qualified by the canonical hat ID, regardless of its narrower directory, workspace, command, host, or account target. A hat-wide action-family option is exceptional for higher-risk actions and never means any outbound host.
9. `plan` approvals are intentionally not remembered by default.
10. `once` settles only the matching tool/permission obligation; a time-to-live Bear-wide approval is not an implementation of `once`. `Always for [hat]` includes eligible future Job runs and requires hat-policy management authority. Current human membership, connected armature for local tools, workspace and surface bounds, actor credentials, Job scope and Den safety policy still apply.

### Known-site account scope

`site_account` is a sticky approval scope for supported sites where the product can derive a stable trust boundary narrower than host and broader than a single URL.

Initial intended examples:

- GitHub account or organization scope, such as `github.com/bears-ai`
- Other future known sites only when the product has explicit parsing and copy rules for the account boundary

Rules:

1. This scope should only exist for a small explicit allowlist of known sites.
2. The derived scope must be stable and legible to the user.
3. If the site-specific account boundary cannot be derived confidently, fall back to host scope.
4. This scope is intended for network and browser actions only.

## Permission option copy

Option labels should describe the policy created.

Preferred patterns:

- `Just this time`
- `Always for {hat}: read this directory ({path})`
- `Always for {hat}: read files in this workspace`
- `Always for {hat}: edit files in this workspace`
- `Always for {hat}: delete files in this workspace`
- `Always for {hat}: access this GitHub account ({account})`
- `Always for {hat}: access this host ({host})`
- `Always for {hat}: run this command in this workspace`
- `Always for {hat}: run safe {family} commands in this workspace`
- `Always for {hat}: allow this bounded action family`
- `Deny`
- `Always deny`

Avoid patterns such as:

- `Allow always`
- `Allow this fetch once` for non-fetch actions
- `allow_workspace`
- `allow_host`

## Tool state copy rules

Each action should have one stable semantic identity across lifecycle states.

| State | Rule | Example |
|------|------|---------|
| Pending | Use base title | `Run cargo test -p den-bearwire` |
| Awaiting permission | Use same title with approval context in body, not a different action identity | `Run cargo test -p den-bearwire` |
| In progress | Use progress verb derived from same action | `Running cargo test -p den-bearwire` |
| Completed | Prefer concise past-tense or result summary | `Ran cargo test -p den-bearwire` |
| Failed | Preserve action identity and add failure summary | `Failed to run cargo test -p den-bearwire` |

### Title rules

1. Prefer target-first, compact titles.
2. Prefer user-intent words over tool-provider names.
3. Avoid generic titles such as `Permission request`, `Tool call`, or `Fetch` unless no better target exists.
4. For multi-file operations, show count in the compact title and full paths in expanded content.
5. For command execution, include the command string in the title when feasible.

## Permission prompt content rules

Every permission prompt should include:

- action summary;
- primary target summary;
- the currently verified hat's name and narrower workspace/directory/host/account scope;
- whether a remembered grant will be available to future conversations and eligible Job runs wearing that hat;
- who is authorized to create that hat-wide grant;
- enough context to judge the request;
- `Just this time` and, when the approver may manage shared hat policy, `Always for [hat]` with its actual bounded target. Other members may request a persistent grant from the hat manager.

### By action family

`read`, `search`, `edit`, `delete`, `move`:

- show target path or paths;
- show affected location list when available;
- show diff for edits when available;
- show source and destination for move/copy;
- show exact path for delete.

`execute`:

- show full command line;
- show cwd;
- show timeout and output limit when known;
- optionally show command family classification.

`fetch` and `browse`:

- show URL;
- show known-site account scope when applicable;
- show host;
- distinguish direct fetch from browser navigation or inspection in the title and body.

`plan`:

- show plan id or artifact path;
- show plan body or an explicit reason why the body is unavailable;
- use dedicated approve/reject wording rather than generic allow/deny wording.

## ACP projection rules

The presentation layer should project into ACP as follows.

### `title`

- always set from the canonical presentation descriptor
- should be human-meaningful without requiring the user to inspect raw input

### `kind`

- set from the internal action family mapping
- where internal semantics are richer than ACP, preserve the richer semantic in metadata and user copy

### `status`

- use ACP status values normally
- do not encode semantic differences in status text alone

### `content`

- use for concise explanation and previews
- include diffs for edits when available
- include command context for execute permissions when useful

### `locations`

- include affected paths for file operations
- include all touched paths for multi-file edits where reasonable
- use workspace/cwd location for command execution when helpful

### `rawInput` and `rawOutput`

- preserve structured diagnostics
- do not rely on them as the sole explanation for the user

### metadata

Internal metadata should be sufficient to reconstruct:

- action family
- permission class
- risk class
- target kind
- target path, URL, host, or command
- arguments summary
- preview availability

## Fallback rules

When tool metadata is incomplete:

1. infer action family from descriptor first
2. infer title from target path, command, URL, or query
3. prefer a concrete fallback such as `Read {path}` or `Run {command}`
4. only fall back to `Tool call` when no action or target can be safely inferred

## Migration guidance

Implementation should proceed in this order.

1. Fix incorrect classifications and copy leaks.
2. Introduce the canonical presentation descriptor shared by tool updates and permission prompts.
3. Normalize permission scope policy by action family.
4. Add previews for edits, commands, and browser/network actions where missing.
5. Add tests that validate semantic projection and scope offerings.

### Immediate bugs to correct

- stop using `fetch` wording for generic permission requests
- reclassify browser navigation and browser inspection actions away from generic fetch wording in user copy
- make workspace-scoped approval available by policy for normal workspace-bounded local actions

## Test expectations

Tests should cover:

- action-family classification for current tools
- title generation for representative file, command, and URL actions
- permission option generation by action family and target shape
- browser actions not being labeled as generic fetch in user copy
- workspace option presence for workspace-bounded local actions
- delete actions offering remembered workspace approval
- known-site account scope appearing for supported sites and host scope appearing otherwise
- diff preview presence for edit permissions where patch information exists

## Deferred discussion

Command normalization for sticky execution approval remains open.

Questions to refine:

1. How aggressively should commands be normalized before they stop matching user intent?
2. Which arguments should participate in exact-command matching?
3. Which command families are safe enough to support family-level workspace approval?
4. How should environment variables, shell wrappers, and relative paths affect matching and display?
