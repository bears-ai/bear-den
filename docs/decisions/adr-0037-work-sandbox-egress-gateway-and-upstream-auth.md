# ADR-0037 — Work sandbox, egress gateway, and multi-identity upstream auth

**Status:** Accepted (2026-06-09)  
**Deciders:** Hans  
**Related:**
- [ADR-0035 — Den-native in-process agent runtime](adr-0035-den-native-in-process-agent-runtime.md)
- [ADR-0036 — Bear profile registry](adr-0036-bear-profile-registry.md)
- [ADR-0039 — Trust profiles and governance](adr-0039-trust-profiles-and-governance.md)
- [ADR-0034 — Jobs and tasks (Docket)](adr-0034-jobs-and-tasks-work-management.md)
- [ADR-0006 — Bear work surfaces](adr-0006-bear-work-surfaces.md)
- [ADR-0040 — Connections and user-facing work-surface presentation](adr-0040-connections-and-work-surface-presentation.md)
- [ADR-0028 — Environment affordance and resource boundaries](adr-0028-environment-affordance-and-resource-boundaries.md)
- [`interactive-stances-and-role-axes.md`](../architecture/interactive-stances-and-role-axes.md)
- [`work-surfaces-and-conversations.md`](../guides/work-surfaces-and-conversations.md)
- [Hat-owned tool and network authorization plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending)
- [`den-runtime.md`](../architecture/den-runtime.md)
- [`DEN_NATIVE_RUNTIME_PLAN.md`](../roadmap/DEN_NATIVE_RUNTIME_PLAN.md) Phase 7

## Hat access-policy amendment (accepted 2026-10-01; partial execution cutover)

Positive tool and outbound-network authorization belongs to the canonical Bear-owned hat, shared by authorized conversations and Job runs wearing it. The per-surface outbound-host list in §5.1 remains a **resource-specific maximum** intersected with hat-authorized destinations, not a second independent permission source. A host must pass both bounds; a surface suggestion alone grants nothing, and a hat host grant cannot expand a surface's reachable network. Membership, actor/Connection credential scope, Den network safety rules, Job assignment, and a connected armature where needed remain separate hard constraints. Internal curation/observation use Den-owned system authority rather than borrowing a hat. No credential is placed inside the sandbox by a hat grant.

The implemented Den web-fetch/search paths now check owned hat tool and exact-host grants. An approved Den web-fetch call either consumes its exact one-shot obligation or, with both hat grants, executes without a new native ACP prompt. A Bear-admin persistent ACP exact-host choice writes the Den-owned fetch tool+host grant pair; client-local actions on configured hats remain one-time-only. Newly provisioned eligible Work sandboxes receive a run-scoped intersection with the work surface's saved host ceiling, enforced again by an upgraded provider; existing relays and provider-owned upstream operations are not dynamically revoked. Registered profile checks and other tool families still await the full cutover. The [active implementation plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending) owns their fail-closed cutover and the one-time versus hat-persistent permission UX.

## Context

Phase 7 replaces Codepool and Letta Code with a Den-native **`work`** execution path: shell, filesystem, and filtered network inside an isolated environment, while the agent loop stays in-process in Den ([ADR-0035](adr-0035-den-native-in-process-agent-runtime.md)).

Design discussions settled several product constraints:

- **`chat` never gets a sandbox.** When execution armature is required, `chat` delegates to a **`work`** run (typically via Docket). The chat channel streams phase events so UX is better than a blocking “please wait.”
- **`pair` defaults to client armature** (ACP client tools, local workspace). Server-hosted checkout for mobile/web is the same sandbox machinery with different profile policy — deferred to Phase 7.1.
- **Durable workspaces from v1** — sandbox sessions are materialized instances of Den-owned workspace records, not invisible throwaway implementation details. A workspace survives across turns and can be archived/recovered even if the runner instance is gone.
- **No warm pool in v1** — a working hypothesis that cold start is tolerable; **telemetry from day one** will validate or falsify that (see §11).
- **Git upstreams are Den-managed** at bear level (origins UI, auth to GitHub / GitLab / Gitea). Legacy `memfs_repo_path` is not the long-term checkout source of truth.
- **Upstream auth is inherently multi-actor.** A single token per sandbox is wrong for UX and security. The desired default on GitHub when a human is in the loop: the **Bear service identity commits**, the **requesting human opens the PR** — without exposing multiple real tokens inside the workspace.
- **Arbitrary repos must work without Bear-specific scaffold.** Users configure a remote origin and auth in Den; the runner clones and discovers tooling from whatever the repo already contains. No required `mise.toml`, repo manifest, or local checkout on the operator machine.

**External references:**

- **[DAM](https://github.com/dam-agents/dam)** — paired gateway pods, credential injection at egress, Connections scoped by owner. Bears adopts the structural pattern on Docker Compose, adapted to Bear profiles, Docket runs, and bear-level service identities.
- **[Locki](https://github.com/janpokorny/locki)** — system containers inside a VM boundary, git/gh **command bridge** with allowlisted operations, parallel sandboxes without port/tag collisions, optional repo-level image hints. Locki targets **local dev**: harness runs inside the sandbox, worktrees live on the host, and it explicitly defers exfiltration protection to DAM. Bears takes Locki’s **parallel isolation and bridge UX** ideas, not its local-worktree or harness-in-box workflow.

## Decision

### 1. Sandbox scope by profile

| Profile | Den-managed sandbox in Phase 7 | Notes |
|---------|-------------------------------|--------|
| **`work`** | **Yes** | Docket dispatch → sandbox session bound to `(bear_id, run_id)` + work surface |
| **`chat`** | **No** | Delegates to `work` when shell/fs/network execution is needed |
| **`pair`** | **No** (default) | Client armature via ACP; Den loop only |
| **`pair` (hosted)** | Phase 7.1 | Same runner + gateway; conversation-scoped binding; interactive approvals |
| **`curate` / `watch`** | **No** | Narrow in-process tool rosters only |

### 2. `bears-sandbox-runner` and pluggable isolation backends

Add a compose service **`bears-sandbox-runner`** (name may be shortened in compose labels) that owns **execution substrate**, not control-plane policy.

The runner exposes a **`SandboxBackend`** abstraction. Den’s `SandboxManager` calls a stable RPC surface; the backend chooses how to isolate. **The durable decision is the abstraction and RPC contract**, not a pre-selected second implementation.

| Backend | Status | Role |
|---------|--------|------|
| **`docker_workspace`** | **Phase 7 (v1)** | Paired workspace + egress gateway containers on an isolated Docker network. Default for `minimal` sandbox profile. |
| **Future backends** | **After v1 telemetry** | Candidates when `full_os` surfaces or ops data justify stronger isolation — e.g. Incus system containers, VM boundaries (Locki’s Lima thesis). Technology choice is **not** committed in this ADR. |

[Locki’s thesis](https://github.com/janpokorny/locki) that thin app containers struggle with nested Docker/k8s/systemd is accepted as **motivation for a second backend**, not as a mandate to ship Incus in 7.2+. Backend selection follows the same telemetry discipline as warm pools.

**Runner responsibilities (all backends):**

- Create/teardown **paired execution + egress gateway** instances per sandbox session
- Materialize **run workspaces** (git clone/checkout) from instructions issued by Den
- Execute **fs** and **shell** RPCs with path jail and resource limits
- Emit **timing telemetry** back to Den (`queued_at`, `instance_started_at`, `git_started_at`, `git_finished_at`, `ready_at`, byte counts, error class, `backend_kind`)

**Den responsibilities (`SandboxManager` client):**

- Decide **when** to acquire/release sessions (aligned with Docket run lifecycle and turn `CancellationToken`)
- Select **`sandbox_profile`** for the work surface / run (see §5)
- Resolve **work surface → origin** and build **`RunAuthContext`** (see §9)
- Push **gateway policy** (egress rules, injection map, git-bridge rules, HITL hooks) per session
- Store **Connections**, secrets, bear service identities, and operator UI
- Stream **delegation/phase events** to `chat` and run detail surfaces

**Ownership rule:** Den owns the **contract** (origins, credentials, policy, audit). The runner owns **materialized bits** (cloned trees, instance state, optional future bare mirrors). Operator configuration of “which GitHub repo is BEARS” lives only in Den.

**Parallel runs:** Multiple concurrent Docket runs on one Bear each get an isolated session (network, branch namespace, ports). This is a first-class requirement — one lesson taken directly from Locki.

### 2.1 Durable workspaces and work lifecycle

Borrowing from Jean's coherent project/worktree/session model, Phase 7 treats the sandbox as part of a durable work lifecycle, not just a container allocator.

Den owns a **workspace record** for each `work` run/work surface. The runner owns materialized instances for that workspace. The workspace record tracks at minimum:

- workspace id, Bear id, Docket run/task ids, stance (`work`), and work surface/origin
- base ref, working branch, clone depth, branch namespace, and PR association when present
- sandbox backend, materialized path/ref, runner instance id, and readiness timestamps
- status (`queued`, `planning`, `running`, `waiting_for_input`, `waiting_for_approval`, `review_ready`, `completed`, `failed`, `archived`, `orphaned`)
- dirty state, changed-file summary, commits produced, last command/check result, and latest artifact refs
- archive, recovery, and cleanup state

A workspace is separate from model turns. V1 may run one active `work` loop per workspace, but the data model must allow later session purposes inside the same workspace: investigation, implementation, review/fixup, conflict resolution, and recap.

Den should expose a minimal **work canvas / run dashboard** over these records so humans can answer: what is in flight, what needs approval, what changed, what failed, what is review-ready, and what can be archived.

### 2.2 Workflow actions, output shaping, and recaps

The coding harness should expose repeatable workflow actions rather than only free-form shell turns. V1 actions should be represented as Docket/run artifacts with prompt/model/tool defaults where useful:

- investigate
- implement
- run checks
- summarize diff
- draft PR
- address review comments
- resolve conflicts
- handoff or recap summary

Shell/fs RPCs must shape output for the model. Full stdout/stderr and large artifacts are stored as run artifacts; model-facing tool results carry bounded summaries and excerpts: status, exit code, concise summary, stdout/stderr excerpts, artifact refs, and truncation markers.

Every nontrivial workspace should be able to produce a durable recap artifact: attempted work, changed files, commands/checks run, current blockers, PR/commit status, and next recommended action. Completed workspaces can be archived/restored; orphaned, dirty, or stale runner instances have explicit recovery and cleanup paths.

### 2.3 Deferred Jean-inspired conveniences

- **Linked projects / linked work surfaces** are deferred. V1 writes only to the active workspace and reads only configured task/work-surface context plus Bear memory. Read-only cross-repo context can come later as an explicit link graph.
- **Auto-sweeps from task sources** (periodic issue/security/Linear intake with filters, active hours, and run limits) are deferred until durable workspace lifecycle and manual Docket dispatch are stable.

### 3. Paired workspace + egress gateway (structural boundary)

Each sandbox session comprises **two isolation peers** on a dedicated network (Docker containers in v1):

1. **Workspace instance** — writable run tree, local git/fs/shell; **no route to TCP 80/443 except the paired gateway**
2. **Egress gateway** — sole path to upstream HTTPS; **credential injection on the wire**; optional **git/gh command bridge** (§4, defense-in-depth); optional **ext_authz** callback to Den for HITL

**Security guarantee (primary):** the workspace instance must not receive real upstream tokens. Injection happens only in the gateway on the wire — matching DAM [ADR-038](https://github.com/dam-agents/dam/blob/main/docs/adrs/038-paired-gateway-pod.md) (paired boundary, not cooperative `HTTPS_PROXY` inside a shared network namespace). This credential boundary is the structural control Bears relies on.

**Isolation guarantee (v1, weaker):** Phase 7 uses Docker bridge networking between workspace and gateway. That contains casual misconfiguration and keeps secrets off the workspace filesystem, but it is **not** a strong boundary against a determined agent executing arbitrary shell (container escape, same-host reachability remain in scope). Stronger isolation is a **backend upgrade**, not something v1 Compose networking claims to provide (see Consequences).

Den must not execute arbitrary user/agent code in the `bears-den` process for `work` turns. The **native agent loop stays outside** the workspace; only tool execution (shell, fs, bridged git/gh) runs inside — unlike Locki, which runs the harness in the sandbox.

### 4. Git/gh command bridge (defense-in-depth, not the security guarantee)

The paired gateway (§3) is the real control. Additionally, adopt Locki’s **command bridge** pattern for routing, audit, and coarse policy — implemented at the **egress gateway** (and/or as workspace shims that RPC to it):

- `git` and `gh` inside the workspace are **shims** forwarding to the gateway
- The gateway may validate invocations against an **allowlisted grammar** of subcommands and flags (Locki publishes a similar filter in its sandbox `AGENTS.md`)
- Policy is applied from **`RunAuthContext`** (§9), not from repo layout:
  - **Branch namespace** — pushes only to configured prefixes (e.g. `bear/{bear_slug}/{run_id}`)
  - **Operation class** — `read`, `push`, `pr_create` map to actor + credential injection
  - **Scope** — current origin/repo only unless work surface grants broader access

**Limits of grammar allowlists:** `git -c …`, aliases, hooks, `GIT_*` env vars, and `gh api` with arbitrary paths are notoriously leaky against an adversarial agent. The bridge is **defense-in-depth and observability**, not a substitute for the credential boundary. Prefer **Den-brokered tools** (e.g. `github.open_pull_request`) for sensitive, auditable operations; restrict or omit `gh api` in v1 bridge rules where possible.

Direct unbridged `git push` over HTTPS from the workspace without passing the gateway is **forbidden** in v1.

### 5. Arbitrary repos, origins, and sandbox profiles

**Access model (more general than Locki for cloud users):** Locki assumes the operator `cd`s to a **local** git repo and gets a host worktree. Bears assumes the operator configures a **remote origin** in Den; the runner **clones** into the session workspace. No local checkout required; no repo-specific Bear config required to start.

**Clone depth** is configurable per origin or work surface (`clone_depth`: shallow default, full history, or explicit depth). Default shallow clone optimizes cold start; surfaces that need blame, bisect, merge-base, or tag discovery should set `full` or a sufficient depth.

**Tooling discovery (opportunistic, not prescriptive):**

- The `work` profile **detects** toolchain hints from the repo (`mise.toml`, `.tool-versions`, `.nvmrc`, `pyproject.toml`, `package.json`, CI workflows, `README`, …) and runs appropriate install commands.
- **No repo file is required.** Absence of `mise.toml` must not fail materialization or block the run.
- Base workspace images may include common tools (git, gh, curl, build essentials, **optional mise**) as convenience, not as contract.

**Optional work-surface hints** (Locki’s optional `locki.toml` pattern, Bears-owned):

```text
work_surface.sandbox_profile ∈ { minimal, full_os }   // default minimal
work_surface.base_image_ref     // optional override
work_surface.clone_depth        // shallow | full | N
work_surface.resource_limits    // optional CPU/mem/disk
```

- **`minimal`** — `docker_workspace` backend (v1).
- **`full_os`** — requests a **stronger future backend** when available; technology TBD from telemetry (§2).

**Sandbox briefing:** Den injects a short **environment appendix** into the `work` turn context (cf. [ADR-0028](adr-0028-environment-affordance-and-resource-boundaries.md)): workspace root, branch policy, how push/PR actors differ, run id for observability, and explicit guidance to **detect tooling from the repo — do not assume mise or any single package manager**.

### 5.1 Surface-owned outbound-host allowlist

`work` sandboxes are **default-deny for outbound network access**. A managed work surface owns one small, inspectable list of permitted outbound hostnames. The gateway receives that list when Den dispatches a sandbox for the surface; it is the only authority for sandbox DNS resolution and outbound connections.

The initial rule is intentionally narrow: an entry permits DNS resolution and **HTTPS on port 443** to one exact hostname. It does not permit arbitrary URLs, wildcard domains, IP ranges, other ports, private-network destinations, or credential access. The gateway must enforce both controlled name resolution and the subsequent connection so a hostname rule cannot be bypassed through a resolver, redirect, or DNS rebinding. Credentials remain separately governed by Connections and gateway injection rules (§7); permitting a host does not supply authentication for it.

At work-surface setup, the selected base image and configured upstream origin may suggest hostnames appropriate to the surface. The setup UI presents those suggestions for acceptance, removal, or addition before the surface is saved. For example, the Rust image suggests `index.crates.io` and `static.crates.io` so Cargo can resolve and download crates; an upstream of `https://github.com/acme/project.git` can suggest `github.com` for the surface's checkout. Once saved, these are ordinary surface entries: an image or origin has no ongoing authority to alter an existing surface's network access when its catalog record or URL changes. A surface can add an unrelated integration endpoint, such as `api.staging.example.com`, to the same list.

This deliberately avoids separate egress profiles, profile versions, grant types, and per-run egress exceptions in v1. If repeated, organization-wide host bundles become a demonstrated need, they may be added later as setup-time conveniences that materialize into the same surface-owned allowlist rather than as a second runtime authorization model.

The effective v1 policy is therefore simply:

```text
allowed outbound destinations = the dispatched work surface's saved hostnames
```

Den audits the surface and its allowlist revision used for each dispatch. Edits apply to future dispatches; an already-running sandbox keeps its dispatch-time policy until it ends. This makes active runs reproducible and prevents a surface edit from silently broadening an existing run.

### 6. Bear-level origins

Den exposes operator UI and APIs to configure **bear-level upstream origins**:

- Provider kind: GitHub, GitLab, Gitea (extensible)
- Canonical remote URL / org-repo identity
- Default branch and branch namespace conventions (e.g. `bear/{bear_slug}/{run_id}`)
- **`service_identity_id`** — which Bear service identity (§8) applies to this origin
- Binding from **work surface slug** → **origin id**
- Optional **sandbox profile** and **clone_depth** fields (§5)

Auth attachments reference **Connections** (§7), not raw secrets in origin rows.

Legacy MemFS git worktrees and `memfs_repo_path` may be used only during migration; new work surfaces use Den-managed origins.

### 7. Connections and Contributions

Adopt a DAM-shaped **Connection** model in Den Postgres (exact table names are implementation detail):

```text
Connection.owner ∈ { user_id, bear_id }
Connection.template ∈ { github, gitlab, gitea, … }
Connection.auth     → encrypted secret refs (OAuth refresh, app credentials, PAT)
Connection.grants   → bear, work_surface, or run-scoped attachment
```

**Contribution kinds** (realized into gateway policy and/or runner config):

| Kind | Purpose |
|------|---------|
| `egress-host` | Legacy/general Connection contribution shape. V1 sandbox routing instead uses the work surface's exact HTTPS hostname allowlist (§5.1); use credential-injection match rules for narrower authenticated API paths where needed. |
| `credential-injection` | Map placeholder → secret ref + match rules (host, method, path) |
| `git-identity` | Committer name/email; optional signing key ref (gateway-only) |
| `operation-policy` | Which actor applies to `read`, `push`, `pr_create`, … |
| `git-bridge-rule` | Allowlisted git/gh subcommand patterns for the command bridge (§4) |

Secrets are stored in Den (encrypted at rest). Gateway instances receive **short-lived credential leases** minted by Den for the session lifetime. **Mount refs are discouraged**; if used for gateway-only material, they must be attached only to the gateway peer and **revoked on session teardown** — never mounted into the workspace instance. The workspace must not hold secret bytes in env, files, or mounts.

### 8. Bear service identities (GitHub App and machine user)

A Bear may hold **multiple service identities**, keyed by upstream scope — not a single “primary” identity:

```text
bear_service_identity {
  bear_id
  provider              // github | gitlab | …
  org_scope             // e.g. GitHub org login, or app installation target id
  identity_kind         // github_app | machine_user
  display_name          // optional UX label
}
// UNIQUE (bear_id, provider, org_scope)
```

**Rationale:** real Bears work across multiple orgs, each with its own GitHub App installation or bot account. Origins (§6) bind to the identity that matches their upstream; `RunAuthContext` carries the resolved `bear_service_identity_id` for the run’s origin.

| `identity_kind` | Intended deployment | Configuration |
|-----------------|---------------------|---------------|
| **`github_app`** | Public / hosted Den | GitHub App installation; installation id + app credentials in Den |
| **`machine_user`** | Self-hosted Den | Dedicated GitHub user (bot account) + PAT or OAuth; seat/licensing docs for operators |

Both kinds are first-class. Policy, gateway injection, and the git bridge treat them identically at the **`push`** operation class — only acquisition and admin UX differ.

**Requester identity** remains a separate Connection owned by `user_id` (human OAuth link). A single run may draw on **multiple Connection owners** (bear identity + requester) without merging tokens.

### 9. `RunAuthContext` and operation-scoped injection

Every **`work`** run (including those spawned from `chat` delegation) carries a **`RunAuthContext`** resolved at dispatch:

```text
RunAuthContext {
  bear_id
  run_id
  work_surface_ref
  requester_user_id           // null ⇒ autonomous run (see pr_create below)
  origin_id
  bear_service_identity_id    // from origin binding (§6, §8)
  sandbox_profile
  run_mode                    // interactive | autonomous — coarse projection of governance (ADR-0039)
  operations: {
    read:        ActorSelection
    push:        ActorSelection
    pr_create:   ActorSelection
  }
}
```

**ActorSelection** resolves to a Connection + injection profile:

```text
ActorSelection ∈ {
  bear_service_identity,
  requester_user,
  operator_connection,
  denied
}
```

**Default GitHub policy** — split by **`run_mode`** (derived from whether `requester_user_id` is set and whether dispatch came from `chat`):

| Operation class | Interactive (human in loop) | Autonomous (`work` / cron / Docket, no requester) |
|-----------------|----------------------------|---------------------------------------------------|
| **`read`** | First satisfied: origin’s bear identity → requester OAuth → operator connection | Same |
| **`push`** | **Bear service identity** for origin | **Bear service identity** for origin |
| **`pr_create`** | **Requester user** (HITL or dispatch consent) | **Bear service identity** → **draft PR** by default |
| **`merge`** | **Denied** | **Denied** |

**Autonomous `pr_create` fallback (closes the null-requester gap):** when `requester_user_id` is null and the run needs a PR, Den-brokered `github.open_pull_request` uses the **bear service identity** and creates a **draft PR** unless the work surface sets `pr_policy = branch_only` (push completes; Docket result includes branch URL, no PR step). Autonomous runs must **not** pause indefinitely waiting for a human OAuth link.

**Interactive vs autonomous asymmetry:**

| Missing credential | Behavior |
|--------------------|----------|
| **`push`** (no bear service identity for origin) | **Fail fast** at dispatch — setup UX (“Connect Bear to GitHub for org X”) |
| **`pr_create`** (no requester OAuth) | **Interactive only:** pause resumably (“Connect GitHub to open PR”). **Autonomous:** use bear draft PR or `branch_only` per surface policy — never indefinite pause |

The gateway selects credentials from **`RunAuthContext.operations`** using request metadata (host, method, path, operation class from Den-brokered tools or git-bridge callbacks). The agent loop and workspace do not choose tokens.

### 10. `chat` → `work` delegation and observability

When `chat` requires execution:

1. Den creates or resumes a Docket **`work`** run with `requester_user_id` from the chat session and `run_mode = interactive`.
2. `chat` returns immediately with a **correlation id** (`run_id`).
3. Den emits **phase events** on the chat SSE/web channel and run detail UI, for example:

   ```text
   dispatch.created
   sandbox.queued
   workspace.materializing
   workspace.ready
   work.turn.started
   work.tool.*
   work.git.pushed        // author=bear identity, branch=…
   work.github.pr.ready   // will_open_as=requester (interactive) or bear draft (autonomous)
   work.github.pr.opened
   work.turn.completed
   ```

4. Timings are recorded on every session to inform warm-pool and backend decisions (§11).

### 11. Phase scope and telemetry gates

| Phase | Deliverables |
|-------|----------------|
| **7** | `bears-sandbox-runner` with **`docker_workspace`** backend only; paired gateway; surface-owned exact-host HTTPS egress allowlist with image-suggested setup defaults; git/gh bridge (defense-in-depth); Den origins + Connections; **multi** bear service identities; `RunAuthContext` with interactive/autonomous `pr_create`; opportunistic tooling detection; sandbox briefing; `work` + Docket; chat delegation + phase SSE; **telemetry** |
| **7.1** | Hosted **`pair`** (conversation-scoped sessions, channel approvals) on same runner |
| **Post-7 (data-driven)** | Second `SandboxBackend` if telemetry warrants; bare mirror / warm pool; per-surface policy knobs; fork-style “run entirely as requester” |

**Telemetry gates (hypotheses, not settled facts):**

- **Cold start tolerable?** — measure `ready_at - queued_at`; warm pool only if p95 exceeds product threshold.
- **Docker sufficient?** — track escape-adjacent failures, nested-docker pain, `full_os` surface requests; second backend choice follows data, not this ADR.

### 12. Compose change

Phase 7 implementation **requires** adding `bears-sandbox-runner` to `docker-compose.yaml`. That edit remains subject to explicit approval per repository rules when implementation lands. Stronger-isolation backend dependencies are runner-host concerns and need not appear in the default dev compose stack.

### 13. Explicit non-adoptions from Locki

To preserve arbitrary-repo and cloud-hosted use cases, Bears **does not** adopt:

| Locki pattern | Bears choice |
|---------------|--------------|
| Local git worktree on operator host | Remote clone in runner workspace |
| Harness (Claude Code, etc.) inside sandbox | Den native loop outside; tools only inside |
| Required `mise.toml` / mise-first startup | Opportunistic detection; mise optional in base image |
| Open network; no credential gateway | Paired gateway + `RunAuthContext` (DAM model) |
| Human commits from host IDE as primary flow | Autonomous `work` + bear push; interactive PR as requester |
| `#locki-<id>` branch suffix | `bear/{slug}/{run_id}` (or configured prefix) |
| Grammar allowlist as exfiltration defense | Credential boundary at gateway; bridge is auxiliary |

## Consequences

### Positive

- Replaces Codepool’s long-lived harness with a **structurally enforced credential boundary** (tokens never in workspace).
- Supports hosted multi-tenant GitHub (**App**) and self-hosted (**machine user**) without diverging execution logic.
- **Multi-org service identities** avoid a premature one-identity cap.
- **Interactive commit / requester PR** and **autonomous commit / bear draft PR** are both defined.
- **`chat` stays simple**; execution complexity lives under `work` with visible progress.
- **Arbitrary remotes** work without local checkout or repo-specific Bear configuration.
- Pluggable **`SandboxBackend`** avoids pre-committing Incus before v1 data.
- Telemetry enables data-driven warm-pool and isolation investment.

### Negative / tradeoffs / security posture (v1)

- **v1 runs untrusted agent shell with container-level isolation only.** Docker bridge networking is operationally convenient, not a strong security boundary. Prompt injection + arbitrary shell inside the workspace remains largely unmitigated ([lethal trifecta](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/): untrusted repo content, upstream credentials via gateway, outbound communication). This is an explicit **MVP posture**, not “sufficient” isolation.
- **Confidentiality is partial even with egress allowlists.** The agent legitimately needs **write** access to the upstream for `push`; encoded data in commits, branch names, or PR bodies can exfiltrate to allowlisted hosts. Allowlists and HITL on interactive `pr_create` reduce casual abuse; they do **not** prevent determined exfiltration through permitted write paths.
- Two isolation peers per active run increases resource use vs a monolithic sandbox.
- Gateway + multi-identity policy + git bridge is more moving parts than a single PAT in env.
- GitHub App onboarding is heavier for self-hosters; machine user docs and warnings are required.
- Legacy MemFS checkout migration is retired for production because there are no production Letta-runtime Bears; archived bundles can use optional import tooling if needed.
- Git command grammar allowlists require ongoing maintenance and remain bypass-prone.

## Non-goals (Phase 7)

- Warm workspace pool (deferred pending telemetry).
- Committing to a **specific** second isolation technology (Incus, VM, …) before v1 data.
- Hosted **`pair`** (Phase 7.1).
- Autonomous merge to protected branches.
- Claiming Docker Compose networking alone satisfies a **strong** isolation threat model.
- Requiring **`mise.toml`** or any repo manifest for sandbox materialization.
- Local host worktrees as the primary workspace model (optional future DX for self-host adjacency, not cloud default).
- Running external coding harnesses inside the sandbox.
- Replacing Den’s existing OAuth provider for **Den login** — upstream Connection OAuth is a separate concern.
- Full CaMeL / quarantined-LLM architecture for prompt injection — acknowledged open problem. Partial measures: credential boundary, coarse egress allowlists, HITL on sensitive interactive operations, audit logs. **No claim** that egress allowlists fully address exfiltration when write to upstream is allowed.

## References

- DAM architecture: [security model](https://github.com/dam-agents/dam/blob/main/docs/strategy/security-model.md), [paired gateway ADR-038](https://github.com/dam-agents/dam/blob/main/docs/adrs/038-paired-gateway-pod.md), [connections](https://github.com/dam-agents/dam/blob/main/docs/architecture/connections.md)
- Locki: [README](https://github.com/janpokorny/locki), [sandbox AGENTS.md (git bridge filters)](https://github.com/janpokorny/locki/blob/main/src/locki/data/AGENTS.md)
- Meta [**Agents Rule of Two**](https://ai.meta.com/blog/practical-ai-agent-security/)
- Simon Willison, [*The lethal trifecta for AI agents*](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/)
