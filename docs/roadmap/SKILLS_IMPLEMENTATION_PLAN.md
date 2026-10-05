# Skills implementation plan

> **Status (2026-06): canonical native-runtime plan.** This document consolidates the Skills roadmap for the Den-native runtime. It supersedes Letta/Letta Code-specific skill installation paths in [`PHASE1_BOOTSTRAP.md`](PHASE1_BOOTSTRAP.md), [`PHASE1_DECISIONS.md`](PHASE1_DECISIONS.md), and [`MULTI_ROLE_RUNTIME_IMPLEMENTATION_PLAN.md`](MULTI_ROLE_RUNTIME_IMPLEMENTATION_PLAN.md). Reuse those documents for product intent and historical rationale only.

Skills are Bear-scoped, reviewed procedure/capability packages that influence how a Bear works. Den owns catalog, approval, attachment, descriptor integration, reconciliation, and export. Runtime execution remains descriptor-owned: Den-hosted tools run in Den, armature-local tools run through BearWire/armatures, and channels do not inherit armature capabilities by default.

Related sources:

- [Roadmap hub](PLAN.md) — Phase 1 product debt and native-runtime priorities.
- [Pair tool discovery and scope policy](PAIR_TOOL_DISCOVERY_AND_SCOPE_POLICY.md) — skills plug into descriptor-based discovery, not prompt suffixes.
- [Memory curation plan](MEMORY_CURATION_PLAN.md) — `skill_review` is a possible curation/review handoff, not a memory promotion itself.
- [Reflection system plan](REFLECTION_SYSTEM_PLAN.md) — future `skill_review` and `skill_apply` lanes.
- [Bear package format](../guides/bear-package.md) — approved skills are portable Bear package artifacts.

## Documentation and model experience

Each delivered Skills phase must review and update [`MODEL_EXPERIENCE.md`](../../MODEL_EXPERIENCE.md) for model-facing skill discovery, proposal, approval, or availability changes. Create or update the relevant documentation in [`docs/guides`](../guides), including skill packaging, operator review, Bear attachment, reconciliation, and portability behavior.

---

## Implemented branch slice (2026-10-05)

Human-authored instruction-only catalog drafts, explicit publication review, immutable approved versions, Bear-admin pinned attachments/detachment, use selection and catalog disablement are implemented in `den-service::skills` and the Bear Skills web page. Drafts and hash-mismatched/disabled attachments are excluded from the context-compiled `bound_skills` component on subsequent verified Chat/Editor/Work turns. Version-2 bundles carry approved procedure content and stage imported procedures disabled. Legacy inactive manifest rows are not auto-activated.

This does not complete the plan: model-authored proposal tools and review lanes, executable/tool-bearing packages, declared capability requirements, internal Curate/Watch procedure integration, reconciliation reporting and larger artifact-backed trees remain target work. No new tool or credential grant is supplied by this slice.

## Goals

1. Provide an operator-managed Skills catalog for reusable procedures, policies, and capability bundles.
2. Attach approved skills to Bears with explicit role applicability: `chat`, `pair`, `curate`, `work`, and `watch`.
3. Let Bears propose new or improved skills, but require curate/reflection or human/operator approval before installation.
4. Integrate skills into model-facing tool/context discovery through descriptors and compiled role profiles, not ad hoc prompt suffixes.
5. Reconcile actual runtime projections against Den's canonical manifest and surface drift in operator UI.
6. Export approved skills as portable Bear package artifacts without copying secrets or host-local bindings.

## Non-goals

- Do not reintroduce Letta Code filesystem discovery or Letta API skill attachment as the canonical path.
- Do not store skills in Bear memory branches or under logical `core/` paths. Memory may reference or propose skills, but approved skill artifacts live in Den-managed catalog/artifact storage.
- Do not allow any runtime stance to self-install executable or tool-bearing skills without review.
- Do not use skills as a shortcut around tool descriptors, permission classes, BearWire routing, sandbox policy, or MCP attachment policy.
- Do not make routine runs automatically learn or install skills without an explicit skill proposal design.

## Concepts

| Concept | Meaning |
|---|---|
| Skill catalog entry | Org- or operator-approved skill source, metadata, version, integrity hash, default applicability, required capabilities, and package/export metadata. |
| Bear skill manifest | Per-Bear source of truth for approved skills and role applicability. This replaces legacy per-agent installation state. |
| Skill proposal | Durable request to add, update, fork, or remove a skill. Proposals may come from `chat`, `pair`, `work`, `watch`, `curate`, humans, or import tooling. |
| Skill projection | Runtime-specific compiled output from the manifest: prompt fragments, descriptor hints, tool roster requirements, sandbox policies, MCP requirements, and artifact mounts. |
| Skill reconciliation | Den process that compares desired manifest/projection state with compiled role profiles, artifact availability, descriptors, and runtime bindings. |

## Native Runtime Model

Den is the system of record. Skills affect runtime through Den-owned compilation and descriptors:

1. The operator or approval lane updates the catalog and a Bear's skill manifest.
2. Den computes a role-relevant manifest slice for each stance.
3. The role profile compiler folds that slice into durable prompt fragments, descriptor metadata, allowed tool requirements, and policy references.
4. The native runtime exposes model-facing capabilities through the existing descriptor resolver.
5. BearWire armatures execute only armature-local tools that descriptors and policy permit. Channel adapters get channel-appropriate projections and do not gain local workspace tools from a skill alone.
6. Reconciliation reports drift and refreshes compiled profiles or artifacts. It does not mutate live in-flight turns.

Skills can describe procedures that use tools, but tool availability is still controlled by descriptors and policy. A skill requiring `fs_edit_file` is applicable only where an armature-local edit tool is present and approved for that stance/session. A web-channel `chat` turn may receive the procedural instruction but not the local filesystem tool.

## Storage Model

Use Den Postgres for catalog, Bear attachments, proposal queues, review state, and operator audit. Use Garage/artifact storage for skill trees when content is larger than inline metadata. Use per-Bear SQLite memory only for memories and curation references to skills.

Initial logical tables:

```text
skill_catalog_entries
  id
  name
  description
  version
  source_kind              -- inline | url | artifact | imported_package
  source_ref
  content_hash
  default_roles            -- chat | pair | curate | work | watch
  required_capabilities
  optional_capabilities
  risk_class               -- instruction_only | mutating_tools | external_effects | code_execution
  status                   -- draft | approved | deprecated | disabled
  created_by
  created_at
  updated_at

bear_skill_manifest
  id
  bear_id
  skill_catalog_entry_id
  version
  content_hash
  applies_to_roles
  enabled
  approval_id
  installed_by
  installed_at
  last_reconciled_at

bear_skill_proposals
  id
  bear_id
  proposer_kind            -- role | human | import | system
  proposer_ref
  proposal_kind            -- add | update | fork | remove | applicability_change
  payload_json
  proposed_roles
  content_hash
  status                   -- pending_review | approved | rejected | needs_human | superseded
  reviewed_by
  reviewed_at
  rejection_reason
  resulting_manifest_id

skill_reconciliation_runs
  id
  bear_id
  role
  desired_hash
  observed_hash
  status                   -- ok | drifted | repaired | failed
  details_json
  created_at
```

These names are logical. Reuse existing migrations if compatible, but avoid carrying forward `letta_agent_id`, MemFS, or filesystem installation semantics.

## Descriptor Integration

Skills must plug into descriptor-based discovery:

- Model-facing skill references should be concise and scoped. A skill can add guidance to a role profile and can annotate tools with when-to-use hints, but it should not duplicate the full role prompt in every descriptor.
- Required capabilities resolve through descriptor metadata, not scattered string allowlists.
- Provider-safe tool names remain concise action names such as `session_info`, `memory_search`, `memory_write_entry`, `web_fetch`, and `fs_edit_file`.
- Canonical internal names remain scoped, for example `den.skill.propose`, `den.skill.approve_proposal`, and `den.skill.reject_proposal`.
- Display metadata for skill-related tool calls belongs with descriptors/resolvers so UI labels are consistent across Den, BearWire, ACP, and future channels.

Initial model-facing tools:

| Canonical | Provider-safe | Roles | Purpose |
|---|---|---|---|
| `den.skill.propose` | `skill_propose` | `chat`, `pair`, `curate`, `work`, `watch` | Propose a new skill or update; never installs directly. |
| `den.skill.list` | `skill_list` | `chat`, `pair`, `curate`, `work`, `watch` | Read the current role-relevant approved skills and their applicability. |
| `den.skill.read` | `skill_read` | `chat`, `pair`, `curate`, `work`, `watch` | Read one approved skill available to the current role. |
| `den.skill.approve_proposal` | `skill_approve_proposal` | `curate` or dedicated `skill_review` lane | Approve a proposal and update the Bear manifest. |
| `den.skill.reject_proposal` | `skill_reject_proposal` | `curate` or dedicated `skill_review` lane | Reject a proposal with a reason. |

Approval tools are not part of ordinary `chat`, `pair`, `work`, or `watch` rosters. Operator UI may expose equivalent human actions without going through model-facing tools.

## Proposal And Review Flow

1. A role, human, import, or reflection lane creates a skill proposal.
2. Den validates payload size, source integrity, declared risk class, capability requirements, proposed applicability, and provenance.
3. Pending proposals appear in operator UI and the Reflection `skill_review` queue.
4. `curate` or a dedicated `skill_review` lane evaluates the proposal with bounded context. It can request human review for high-risk or ambiguous proposals.
5. Approval writes or updates the catalog entry as needed, updates `bear_skill_manifest`, records audit metadata, and schedules reconciliation for affected roles.
6. Rejection records a reason and leaves runtime projections unchanged.

Memory curation may hand off to skill review by using `suggested_action: skill_review`, but memory proposals and skill proposals remain separate queues. A memory extraction that looks like a reusable procedure should become a skill proposal, not a promoted `core/` memory entry containing executable instructions.

## Role Applicability

Default applicability should be conservative and explicit:

| Role | Skill examples | Notes |
|---|---|---|
| `chat` | User-facing explanation style, intake workflows, domain Q&A procedures | No local workspace or outbound side effects unless the channel separately exposes approved tools. |
| `pair` | Repo conventions, debugging playbooks, code review checklists, editor workflows | May reference armature-local tools when BearWire session policy exposes them. |
| `curate` | Memory review heuristics, consolidation procedures, skill-review rubric | Broad memory read, narrow write authority. No external-effect shortcuts. |
| `work` | Task execution recipes, release procedures, incident runbooks | Requires sandbox and Docket run context for external effects. |
| `watch` | Event parsing, triage heuristics, subscription summarization | Inbound observation only; cannot dispatch work or post outbound messages. |

Some skills apply to all roles, for example org writing standards or privacy classification rules. Capability-bearing skills should usually apply only to the roles that can safely use the required tools.

## Reconciliation

Reconciliation validates desired vs observed state without depending on Letta installation surfaces:

- Catalog content hash matches stored artifact bytes or source snapshot.
- Bear manifest entries point to approved, enabled catalog versions.
- Role profile compiled hash includes the role-relevant manifest slice.
- Descriptor resolver can satisfy each required capability for the target role/surface, or marks the skill partially unavailable with an actionable reason.
- Artifact paths referenced by Bear package export exist and match hashes.
- Deprecated or disabled skills no longer appear in new compiled role profiles.

Reconciliation should be idempotent. It may refresh compiled configs and artifact caches, but it must not modify in-flight turn state. Runtime changes apply to subsequent turns or after a role-profile reload boundary.

## Operator UX

Initial operator console slices:

1. Catalog list/detail: name, version, status, source, hash, risk class, required capabilities, and default role applicability.
2. Bear Skills tab: current manifest, enabled/disabled state, per-role applicability, reconciliation status, and drift/errors.
3. Proposal queue: pending/approved/rejected proposals with proposer, diff/summary, risk class, review notes, and approve/reject controls.
4. Import/export affordances: include approved skill artifacts in Bear packages and validate incoming package skills before enabling them.

Operators should be able to attach existing approved skills directly. Agent-proposed skills require review before becoming approved catalog entries or Bear manifest entries.

## Package Export And Import

Approved skills are portable Bear package artifacts per [`../guides/bear-package.md`](../guides/bear-package.md):

- Export `manifest.yaml` references to approved skills and place skill trees under `artifacts/skills/`.
- Include content hashes and compatibility metadata.
- Do not export secrets, OAuth tokens, webhook keys, host-local tool bindings, or MCP server credentials.
- On import, create disabled or pending-review catalog entries unless the operator chooses to trust the package source.
- Recompile role profiles and rebuild descriptors on the destination host. Do not copy runtime binding ids.

## Implementation Phases

### Phase 0 — Inventory and migration stance

1. Inventory existing `bear_skills_manifest`, `bear_skill_proposals`, admin UI, descriptor, and package-export code.
2. Decide whether existing tables can be migrated in place or should be replaced by new native-aligned tables.
3. Mark Letta-era skill install/materialization code as deprecated or remove it if unused.

Acceptance:

- A short implementation note lists kept, migrated, and deleted legacy surfaces.
- No active plan depends on Letta Code skill directories, Letta skill APIs, or MemFS skill paths.

### Phase 1 — Catalog and manifest foundation

1. Add or align catalog, manifest, proposal, and reconciliation schema.
2. Implement admin APIs for catalog CRUD and Bear attach/detach/applicability changes.
3. Add audit fields and content-hash validation.
4. Add operator UI for catalog and Bear manifest read/write.

Acceptance:

- Operators can approve a catalog skill and attach it to a Bear with role applicability.
- Manifest state is queryable by Bear and role.
- Attachments cannot reference unapproved or hash-mismatched content.

### Phase 2 — Runtime projection and descriptors

1. Include role-relevant skill manifest slices in role-profile compilation.
2. Add descriptor metadata for `skill_list`, `skill_read`, and `skill_propose`.
3. Resolve skill required capabilities through descriptor metadata and current surface policy.
4. Surface unavailable skill capabilities through `session_info` or equivalent status, not hidden prompt text.

Acceptance:

- A native `pair` turn can discover approved role-relevant skills through descriptor-owned tools.
- A channel `chat` turn does not receive armature-local tools merely because a skill mentions them.
- Descriptor tests cover provider names, internal names, display metadata, and role applicability.

### Phase 3 — Proposal and review lifecycle

1. Implement `den.skill.propose` with validation and provenance.
2. Implement human/operator approve/reject.
3. Implement `curate` or Reflection `skill_review` approve/reject tools.
4. Connect memory curation `skill_review` handoffs to skill proposals without merging proposal stores.

Acceptance:

- Runtime roles can create pending proposals but cannot install skills directly.
- Approval updates catalog/manifest and schedules reconciliation.
- Rejection preserves audit trail and leaves role profiles unchanged.

### Phase 4 — Reconciliation and drift visibility

1. Compute desired hashes for catalog artifacts, Bear manifests, and role-profile skill slices.
2. Add reconciliation runs and status reporting.
3. Refresh compiled role profiles after manifest changes.
4. Show drift and repair failures in operator UI.

Acceptance:

- Reconciliation is idempotent and safe to rerun.
- Removing or disabling a skill removes it from subsequent compiled role profiles.
- Operators can see which role/surface is missing a required capability.

### Phase 5 — Package portability

1. Export approved skill artifacts and manifest references in Bear packages.
2. Validate imported skills by hash and compatibility metadata.
3. Import skills as disabled or pending review unless the package source is trusted.
4. Recompile role profiles on import.

Acceptance:

- A cognition export includes approved skills without secrets or host-local bindings.
- Import can recreate catalog/manifest candidates and requires operator trust before enabling risky skills.

## Open Questions

1. Should approved catalog skill artifacts live primarily in Postgres, Garage, or filesystem-backed Den storage for local development?
2. Should `skill_list` and `skill_read` be model-facing in all roles from the first slice, or initially only `pair` and `curate`?
3. What risk threshold requires human approval even if `curate` recommends approval?
4. Should MCP attachment requirements be modeled inside skills or remain separate catalog attachments linked by capability references?
5. What compatibility version should package-import enforce for executable or tool-bearing skills?
