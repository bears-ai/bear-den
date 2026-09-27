# Bear Den documentation

Start with the [public project introduction](../README.md). To develop or troubleshoot, enter through a topic below, then follow its current contract, guide, plan, and ADR links. [Contributor workflow](../CONTRIBUTING.md) and [agent conventions](../AGENTS.md) live at the repository root.

## Topics (maintained entry points)

| Topic | Maintained by | What works now, what is intended, and where to read deeper |
|------|------|------|
| [BearWire and ACP](topics/bearwire-acp.md) | Den BearWire + Armature code owners | Runs, armatures, client obligations, and approvals |
| [Docket and task execution](topics/docket.md) | Den Docket code owners | Jobs, session tasks, and focused/background work |
| [Bear memory and hats](topics/bear-memory-hats.md) | Den memory, runtime-policy, and Bear management code owners | Current memory boundaries and the proposed session/hat model |

Other topics will move onto this spine incrementally. Until then, use the existing [architecture index](architecture/README.md), [developer and operator guides](guides/README.md), [decisions](decisions/README.md), [planning index](roadmap/README.md), and [public explanation](website/what.md). The older [planning hub](roadmap/PLAN.md) contains dated claims and is **not** an authoritative snapshot of deployed behavior. Service-specific runbooks may stay beside their services.

## Reading and writing rules

- **Current** is a claim about a verified implementation or supported deployment. Topic pages identify scope, verification date, and code/test/release evidence. A recent date alone is not evidence.
- **Target** describes an intended change and links to a plan. A draft specification or accepted ADR does not imply implementation. Plans describe the *delta* and exit criteria, not today's behavior.
- **Decisions** explain why: preserve ADR history, link superseding records, and treat `Accepted` separately from implementation status.
- **History** (completed plans and archives) is useful for archaeology, not for onboarding or live behavior. Label it and link back to the maintained topic.
- When editing a topic, update its current behavior only after verifying evidence; move delivered plan outcomes into the current description and mark the plan completed separately. Keep public claims consistent with the verified topic without copying implementation details.

Before adding a document, find its topic home. Prefer improving that page or a linked guide/plan to adding another overview. A new topic is linked here in the same change; a new plan is linked from its topic **and** [the planning index](roadmap/README.md); a new ADR links to its topic and the [decisions index](decisions/README.md). Avoid another catalog of all documents.

During each release review, the code-area maintainer checks the topics affected by that release: verify current claims against code and a relevant test or deployment, reconcile plan exit criteria and public claims, and update evidence/date only when that verification happened. Periodically review untouched, dated topics for stale claims; do not refresh dates automatically. The PR template asks contributors to explain documentation impact even when no edit is needed.

The fast guard is `python3 scripts/check-docs.py --base HEAD` (CI supplies its base revision). It checks *inline* local Markdown links and heading anchors, provenance on new topics, real topic links and indexing for new plans/ADRs, and new ADR-ID collisions. `--all` displays the legacy link backlog for deliberate cleanup. It does not check reference-style links, external URLs, or whether a current-state claim is true; code-area reviewers verify those against implementation evidence. CI runs the same guard and warns when mapped code changes lack a topic update; existing broken links are grandfathered, not endorsed.

## Cloning and automation

This repo is a light monorepo: documentation and `services/*` deploy artifacts share one Git history.

- Shallow clone if you only need a subset:

  ```bash
  git clone --depth 1 <repo-url>
  ```

- Sparse checkout is optional when machines should only materialize selected paths:

  ```bash
  git clone --filter=blob:none <repo-url> bears-deploy
  cd bears-deploy
  git sparse-checkout init --cone
  git sparse-checkout set services/bifrost docs README.md AGENTS.md
  ```

## Assistant-oriented material

Tooling notes for coding agents live at the repository root in **[AGENTS.md](../AGENTS.md)**. Agent instructions are not a substitute for the maintained topic pages or the verified current-system contract.
