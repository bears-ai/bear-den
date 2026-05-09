# ADR: Bear MCP Services

**Status:** Accepted
**Date:** 2026-05-03
**Deciders:** Hans

## Context

Bears are composed of multiple Letta agents (talk, pair, curate, work, watch) running on a mix of Letta Code harnesses and direct Letta API connections, governed by Den. Each agent has its own tool profile drawn from several sources:

- **Letta-native tools** (server-side capabilities the Letta agent itself implements).
- **Letta Code's harness tools** (Bash, Read, Write, Edit, Task subagents) — available only to harness-backed agents (talk, work).
- **ACP-client tools** (the IDE's native capabilities and any MCP servers the user has configured) — available only to pair via the ACP handshake.
- **Bear-internal capabilities** that need to be available consistently across agents regardless of harness, with role-based access control: writing task intents, proposing skills, recording observations, querying Bear lifecycle state, etc.
- **User/Bear collaboration capabilities** — services where users and Bears share authoritative state, both contributing to the same data store through their respective interfaces. These services have their own human-facing UIs and are not solely Bear infrastructure; the Bear is one of several clients.

The latter two categories are the subject of this ADR. They share a delivery mechanism (Den-side MCP servers) but have meaningfully different roles in the architecture.

The naive approach — implementing these capabilities as Letta-native tools attached per agent — fails the auditability and consistency requirements. Tool implementations would live in five places (or get copy-pasted between harnesses), drift between them is a real risk, and there's no central enforcement point for role-based access.

## Decision

Capabilities are exposed via **Den-side MCP servers**. The agents call them as MCP tools; the servers enforce policy and are the single implementation of each capability.

### 1. Three initial MCP servers, in two tiers

**Bear-internal infrastructure** — central to Bear operation, owned by Den:

| Server | Domain | Optional? |
|---|---|---|
| `den` | Bear lifecycle, skill management, memory governance, observation recording, Den-internal queries | Required |

**User/Bear collaboration services** — privileged spaces where users and Bears share authoritative state:

| Server | Domain | Optional? |
|---|---|---|
| `docket` | Task intent lifecycle, approved task management, run results, project planning, human task UI | Optional |
| `cabinet` | Knowledge base search and contribution; durable referential knowledge | Optional |

The two tiers behave differently in the architecture. `den` is part of Bear's core operation — the Bear cannot function without it because skill management, observations, and memory governance all flow through it. `docket` and `cabinet` are sibling services the Bear collaborates with: they have their own human UIs as primary interfaces, their own data stores as systems of record, and the Bear is one of several clients. Deployments without `docket` lose autonomous task execution; deployments without `cabinet` lose the durable referential knowledge layer; both can be omitted independently.

### 2. Server boundaries

The boundaries between the three servers reflect different concerns and different rates of change:

**`den`** is for capabilities tightly coupled to Den's own lifecycle: provisioning, reconciliation, skill manifest mutations, observation recording (called by the watch agent), memory review requests, Reflection audit queries. Tools here change when Den itself changes. Examples: `den.skill.propose`, `den.observation.write`, `den.memory.request_review`, `den.reflection.status`, `get_bear_health`, `request_memory_rollback`. Required for any Bear deployment.

**`docket`** is for the task intent → approval → execution → results lifecycle, plus the project-management dimension that grows around it (dependencies, milestones, retrospectives, work-queue views, human reviewer assignments). Tools here change with the project-management feature set. Examples: `write_task_intent`, `approve_task_intent`, `reject_task_intent`, `query_task_status`, `write_run_result`. Although `docket` is necessary for the autonomous-work pipeline (talk and pair writing intents, curate approving them, work executing), it's a distinct service from `den` because:

- Project-planning depth would warp Den's data model if folded in.
- Tasks have a primary human UI (review queue, planning views) with very different needs from the operator UI Den has.
- Task-related metrics, retention policies, and reporting requirements are sufficiently different to warrant separate ownership.
- The Bear is not the only client — humans plan, review, and contribute to Docket directly through its UI.

**`cabinet`** is for durable referential knowledge — the broader knowledge layer the Bear and its users contribute to over time. Tools here are search and contribution surfaces over Cabinet's data store. Cabinet has its own human UI as a system of record. Examples: `kb_search`, `kb_contribute`, `kb_link`, `kb_review_proposal`. Cabinet is optional because not every deployment will include a knowledge layer of this scope; deployments without it lose the relevant tools but no other capability.

### 3. The Cabinet ↔ `core/` boundary

Cabinet and `core/` are both knowledge layers. They serve different purposes and the distinction must be explicit to avoid users (and agents) being confused about where information should live.

- **`core/`** is the Bear's distilled self. It is curated by the curate agent, narrow, and present in every agent's system prompt. It captures durable facts about the Bear's identity, the user, current projects, communication preferences, and accumulated learnings. It is small enough to fit in context.
- **Cabinet** is the broader knowledge layer. It captures referential material that is too voluminous to live in context: documentation, retrospectives, design docs, decisions, longer-form notes. It is searchable but not auto-loaded. It can have human contributors and is shared across users (subject to Cabinet's own access controls).

Curate's reflection cycles consider both sources. When integrating new content from channel branches, curate decides whether a learning belongs in `core/` (small, identity-shaping, useful in every conversation) or in Cabinet (larger, referential, useful when the topic comes up). Curate has tools for both: `core/` writes through privileged Den tools; Cabinet writes via `cabinet`'s contribution tools.

This split means agents looking for information try `core/` first (it's already in context) and search Cabinet when they need more depth. When users contribute via Cabinet's UI directly, that knowledge is available to the Bear without any agent action; when users teach the Bear via talk or pair, the curate cycle decides which layer is appropriate.

When Cabinet is not deployed, there is no second layer — `core/` is all the Bear has. This is acceptable for simple deployments and removes a category of decision-making from curate.

### 4. The Docket ↔ Cabinet boundary

Both Docket and Cabinet have human UIs and deal with persistent state. The boundary must be deliberate.

- **Docket** is operational. It tracks what is being done, what is done, what failed, what is scheduled. Its UI is a work queue and a project-planning surface.
- **Cabinet** is referential. It captures durable knowledge.

The relationship is that tasks produce knowledge: a retrospective writeup belongs in Cabinet; the task that produced it lives in Docket, with a link. Tasks-as-documentation is a Cabinet concern; tasks-as-work-tracking is a Docket concern.

When a deployment lacks Cabinet, retrospectives and similar referential output have nowhere natural to live. Deployments without Cabinet either accept that referential output is captured ad-hoc in `core/` (with the size constraints that implies) or accept that some patterns aren't supported.

When a deployment lacks Docket, there is no autonomous-work pipeline at all — the work agent has no tasks to execute. Such deployments are sensible for Bears that are purely conversational and don't need scheduled or event-triggered external action.

### 5. Tool-level role-based access

Role enforcement happens at the tool call, not at tool attachment. Every MCP server validates the caller's `(bear_id, role)` on every invocation and rejects calls the role isn't entitled to make. Tool absence (not attaching a tool to an agent) is defense-in-depth, not the primary check.

The curate agent's tool access deserves explicit articulation, since curate is otherwise tool-minimal:

- Curate has **broad access to `den`** — it is the agent that performs skill approvals, memory governance operations, and observation reviews; these are core to its role as integrator.
- Curate has **scoped access to `docket` and `cabinet`** — limited to the operations curate's role requires:
  - In Docket: approve/reject task intents, write derived task intents from observations, query task and run state. Curate does *not* write task intents on the user's behalf, schedule tasks, or override Docket's human-side workflows.
  - In Cabinet: contribute authoritative entries (curate-promoted knowledge), review proposed contributions from channel agents, and follow links between entries. Curate does *not* freely search Cabinet (it sees `core/` directly and reads peer branches via worktree); search is for agents that lack curate's privileged read access.
- Curate has **no other tool access** beyond what is described above. It does not call work-style external tools, it does not call IDE-side tools, it does not invoke arbitrary capability servers added in the future. New MCP servers do not implicitly grant curate access; explicit, scoped access must be granted per server.

Other roles' access:

- `write_task_intent` (in Docket) is callable by talk and pair, rejected for curate, work, and watch.
- `kb_search` (in Cabinet) is callable by talk, pair, and work; rejected for watch and curate.
- `kb_contribute` (proposed contribution; in Cabinet) is callable by talk and pair (with curate review of the proposed contribution).
- `den.skill.propose` (in `den`) is callable by roles allowed to request skill review.
- `den.skill.approve_proposal` (in `den`) is callable only by curate or an explicitly authorized skill-review lane.
- `den.memory.request_review` (in `den`) is callable by producer roles such as talk, pair, work, and watch for their own role-local memory.
- `write_observation` (in `den`) is callable only by watch.

This per-tool, per-role matrix lives in each MCP server's policy configuration. The skill manifest determines which tools are *attached* to each agent (defense in depth); the MCP server determines which calls are *authorized* (the real gate). Tools may be attached but unusable for a given role; that's an acceptable surface area cost.

### 6. Identity and auth

All three MCP servers share an identity model. Den issues per-agent MCP tokens at provisioning time, bound to `(bear_id, role)` and (for harness-backed agents) `(letta_agent_id)`. Each MCP server validates the token against Den's identity service on every call and uses the resolved identity for authorization and audit logging.

Implications:

- Token reissue is a Den responsibility, triggered by reconciliation when an agent is re-provisioned.
- The identity service is part of `den` (or alongside it as a small auxiliary service); the other MCP servers depend on it.
- Audit logs from all three servers share a common shape (`bear_id`, `role`, `letta_agent_id`, `tool_name`, `timestamp`, `outcome`) and can be aggregated for cross-server queries.

ACP-side tools (anything the IDE provides via ACP/MCP) are out of scope for this auth model — they live in the user's environment and are governed by the IDE's permission flow. Den's MCP servers and ACP client-provided MCP servers are independent worlds; the pair agent uses both, with different auth semantics for each.

### 7. The MCP server registry

Den maintains a registry of which MCP servers are configured for the current deployment. The registry has, per server, the URL, the version, the tools it advertises, and whether it is required or optional. Provisioning consults the registry: a manifest entry referencing a tool from a non-deployed server is silently skipped (with a log entry, not an error).

This pattern means:

- New MCP servers can be introduced without code changes — just registry entries plus manifest tools that reference them.
- Optional servers (currently `docket` and `cabinet`, potentially others) compose cleanly: deploy or don't, the rest of the system doesn't care.
- The "five agents" architecture and the MCP services architecture are decoupled — adding a server doesn't ripple into the agent ADR.

The `den` server is the only required entry in the registry. Provisioning fails if it is missing or unreachable; all other servers are optional from the architecture's perspective, even though specific deployments may treat `docket` as effectively required for their use cases.

### 8. Versioning

Each MCP server has a semantic version. The skill manifest pins which version of which server's tools each Bear is configured against (via the `content_hash` already in the manifest schema). Tool schema changes are breaking changes from the manifest's perspective, even if the tool name is unchanged.

Reconciliation checks server version compatibility: a Bear configured against `docket 1.4` cannot run against `docket 2.0` without explicit re-provisioning. This is mildly painful but the alternative (silent behavior changes when a server is upgraded) is worse for systems where curate's review decisions depend on tool schemas being stable.

Server upgrades are coordinated through Den: deploy the new version alongside the old, migrate Bears in waves with reconciliation, retire the old version. This is the same shape as the agent migration story in the multi-agent ADR.

### 9. Memory tools and the MemFS overlap

Some `den` tools touch memory: `request_memory_rollback`, `query_memory_history`, and `den.memory.request_review` for agents that want Reflection/`curate` to review role-local memory. These overlap conceptually with MemFS, which agents may also use natively.

The split:

- **MemFS** is for the agent's natural memory access — files loaded into context, browsable as filesystem operations through Letta's existing MemFS layer. This is plumbing.
- **`den` memory tools** are for cross-agent operations that require Den mediation — proposing writes, requesting rollbacks, querying historical states, recording provenance. This is governance.

A talk agent reading its own `core/` files goes through MemFS. A talk or pair agent that wants curation of role-local memory goes through `den.memory.request_review`, optionally with `suggested_action: promote_to_core`, `summarize_into_core`, `cabinet_update`, or `skill_review`. Den queues the request for Reflection/`curate` review. The two paths to memory have different semantics on purpose: one is in-context plumbing, the other is auditable governance.

## Consequences

### Positive

- One implementation of each capability, regardless of harness family.
- Role-based access is enforced server-side, robust against tool-attachment drift.
- Audit logging is centralized and consistent across capabilities.
- Optional capabilities (Docket, Cabinet, future services) compose cleanly via the registry.
- The agent architecture and the capability architecture evolve independently.
- Bears support both agent-driven and human-direct contributions to knowledge (via Cabinet's UI), tasks (via Docket's UI), and other domains as new servers are added.
- Curate's tool surface is small, explicit, and bounded: full access to `den`, scoped access to `docket` and `cabinet`, nothing else by default.

### Negative / Tradeoffs

- More services to deploy and operate (Den itself plus the `den` MCP server, optionally Docket, optionally Cabinet, plus the identity service).
- Versioning across multiple independently-deployed servers is a real cost.
- Token issuance and validation add latency to every Bear-internal tool call.
- The Cabinet/`core/` and Docket/Cabinet boundaries are conceptual and require ongoing discipline to maintain — there will be cases where it isn't obvious where something belongs.
- Deployments without Docket lose autonomous task execution entirely, even though the architecture treats it as optional. Operators choosing whether to deploy Docket need to understand this implication.
- Deployments without Cabinet have a degraded experience for referential knowledge; this is acceptable but should be documented for operators.

### Operational risks to monitor

- Skill manifest entries referencing tools from MCP servers that aren't deployed: should be silently skipped but worth alerting if the rate is high (suggests deployment misconfiguration).
- Token validation as a single point of failure — Den's identity service must be highly available or every capability tool call breaks.
- Cabinet's data drifting in scope toward operational concerns or Docket drifting toward referential knowledge; periodic boundary review.
- Cross-server version compatibility during upgrades — wave migrations need careful coordination.
- Audit log volume across multiple servers may be substantial; retention policy needs to be set deliberately.
- Curate's scoped access to Docket and Cabinet expanding informally over time — the explicit list of curate-callable tools per server should be reviewed when adding new tools to either.

## Alternatives considered

**Letta-native tool implementations per agent.** Each agent's tools implemented locally (in the harness for talk/work, in the API-direct adapter for pair/curate/watch). Rejected: implementations would live in five places, drift is a real risk, no central audit, role-based access becomes a per-implementation concern.

**A single monolithic Den MCP server.** All capabilities in one server. Considered. Simpler operationally but couples the rates of change of unrelated tools (Cabinet's evolution would force redeploys of unrelated Den-internal capability), and forces optional-deploy stories to be hacked in. Multiple servers with an explicit registry handles this cleanly.

**Per-capability MCP servers (one per tool family).** A `skills` server, a `memory` server, a `lifecycle` server, etc. Rejected as over-decomposition. The proposed split into `den` plus collaboration services aligns with deployment ownership and rates of change, which is a more useful axis than "one server per logical group of tools."

**Cabinet as a Letta agent rather than an MCP server.** Considered briefly: Cabinet could be modeled as another agent in the Bear's cluster, with its own branch and tool surface. Rejected because Cabinet is meant to have a primary human UI as a system of record, to be optionally deployed, and to be a sibling service shared across uses rather than per-Bear infrastructure. None of those map well to "agent in a Bear's cluster"; an MCP-fronted independent service is the right shape.

**Docket folded into `den`.** Considered. Tasks could be just another `den` capability domain rather than a distinct service. Rejected because (a) the project-management depth Docket grows toward would warp Den's data model, (b) Docket has a primary human UI with very different needs from Den's operator UI, and (c) treating it as a sibling collaboration service surfaces correctly that Bears are clients of Docket, not its sole owners. The naming convention reinforces this: `den` is Bear infrastructure, `docket` is a sibling service.

**ACP MCP servers for pair only.** Use the user's IDE's MCP support to surface capabilities to pair, while talk/work/curate/watch use Den-side implementations. Rejected: requires every Bear user to configure MCP servers in their IDE, which is friction; doesn't help the harness-backed agents; loses the central enforcement point. Den-side MCP servers configured uniformly across agents is the cleaner story, with ACP-client MCP servers used only for user-environment-specific tools the user actually wants in their IDE.

**Skip MCP entirely; use HTTP APIs.** Den and sibling services expose capabilities via plain HTTP; agents call them via generic HTTP-client tools. Rejected: loses the schema, discovery, and protocol-level capabilities of MCP; agents need richer-typed tool definitions to call capabilities reliably; MCP exists exactly to solve this problem.

## Naming notes

The names reflect the architectural distinction between Bear infrastructure and sibling collaboration services:

- **`den`** is the Bear-internal infrastructure server. The name matches the control plane (Den), since this MCP server is Den's tool surface to its agents.
- **`docket`** is the operational task and project-planning service. The name evokes a list of items to be addressed, with the formal connotation appropriate for a planning surface.
- **`cabinet`** is the durable referential knowledge service. The name evokes a place where things are stored and retrieved, with the implication of curated organization.

Neither `docket` nor `cabinet` carries a `den-` prefix because they are not Bear infrastructure — they are sibling services that Bears collaborate with, each with their own human UIs and their own systems of record. Future services follow this pattern: bare names for sibling services that have human-facing UIs and authoritative state of their own; `den-` prefix only for capabilities that are squarely Bear infrastructure (none currently planned beyond `den` itself).

## Open questions

These are not blockers for the architecture but warrant attention as it's built out:

1. **Multi-tenancy of Cabinet.** Cabinet is a sibling service that may be shared across Bears or scoped per Bear/team/deployment. The data model and access semantics depend on this.
2. **Cross-Bear queries via `den`.** Are there cases where Bear A's curate agent should be able to query Bear B's state? Probably no for security, but worth being explicit before someone asks.
3. **Tool result caching.** `kb_search` results are reasonable to cache; `query_task_status` probably is not. A per-tool cache policy may be needed.
4. **MCP server discovery for new agents.** When a new role is added (the hypothetical sixth agent), provisioning needs to know which tools from which servers it gets. The skill manifest's `applies_to_roles` already supports this, but tooling for "what would a new role's tool set look like" is worth thinking about.
5. **Federation across deployments.** If two deployments each run their own Bears, can they share Cabinet? Docket? This is a concern for the future, not for the initial architecture.
6. **Curate's scoped access surface area.** The list of Docket and Cabinet tools curate can call needs explicit articulation in `bear-spec.md` and review whenever new tools are added to either server. Without this discipline, curate's narrow access can drift wider over time without anyone noticing.

## References

- Multi-Agent Architecture ADR (companion document): describes the five-agent Bear architecture this ADR layers capabilities onto.
- MCP specification: <https://modelcontextprotocol.io/>
- ACP MCP integration patterns: <https://geminicli.com/docs/cli/acp-mode/>
- Cabinet design notes (forthcoming, internal)
- Docket design notes (forthcoming, internal)
