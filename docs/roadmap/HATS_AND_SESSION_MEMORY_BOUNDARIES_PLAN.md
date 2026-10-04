# Hats and session memory boundaries

**Status:** Active — current working-tree WIP, not shipped.

**Stance retirement:** Implemented and locally deployed in `78f1bb97`: no selectable stance, profile registry prerequisite, profile model/loop override, role-selected identity prompt, ordinary profile-memory branch, or label-only model-tool authority remains. `RuntimeContextLabel` preserves derived/audit encodings only. Gates 0A and broader rollout evidence remain open for the separately identified permission/resource and live-provider work.

**Topic:** [Bear memory and hats](../topics/bear-memory-hats.md).

Ordinary source admission, hat identity/memory, Bear-wide settings, and stance-registry retirement are implemented in the current WIP. Every ordinary conversation, Job, and run needs a real Bear-owned hat, including zero-hat Bears. Old unbound history remains owner/admin read-only; ownerless history is admin-only. No default hat, source ownership, imported grant, or promotion is fabricated. Pending IDE sessions materialize a durable `den-conv-*` only after hat admission. Inference, continuations, result recording, and direct dispatch recheck the live canonical actor/source/hat and exact eligible Work run. These boundaries do **not** complete hat-action/resource filtering. The implementation session reports passing workspace all-target checks and 121 root tests; these are WIP checks, not latest-image evidence. The latest Docker image has not been rebuilt, and earlier smoke evidence does not validate these changes.

## Gate 0 — Verify the contract before widening runtime behavior

Authority inputs are typed: authenticated human/current membership; immutable canonical conversation or exact Job/run source; real Bear-owned hat; verified channel/armature origin and current availability; governance; resource/credential ceilings; grants and exact approval obligations. Model text, labels, prompts, and client descriptors cannot create them.

Ordinary memory is own-source private notes + authorized selected-hat knowledge + Bear `core/`. Other sources' raw notes and historical profile-local memory are denied through tools, direct reads, projection, and recall. Internal curation and observation use dedicated verified system sources, not a default hat or generic ordinary session.

| Verified source | Boundary |
| --- | --- |
| Channel conversation | Canonical owner/hat admission; Den-hosted tools, no local armature tools. |
| Browser task session | Server-derived owned session-task authority; no local armature tools. |
| Armature conversation | Same canonical memory owner; local tools additionally need a connected trusted client, governance, resource bounds, and obligations. |
| Disconnected continuation | Same immutable source; unavailable client tools cannot be borrowed. |
| Authorized Work run | Exact live uncancelled run, eligible Job hat, assigned surfaces; own-run notes only. |
| Curation/observation | Narrow source-verified worker/intake operations; no generic ordinary model-tool route. |

**Exit pending:** a reviewed route/effect denial matrix across admission, next model request, replay, revocation, and all effects, not only a capability-table test. Historical ownership/collisions and same-creator artifact isolation remain separately audited.

## Gate 0A — Hat-owned tool and network permissions (partial cutover; exit pending)

Den owns canonical `(bear_id, hat_id)` positive action and bounded target grants. Actor membership, credential scope, verified origin/client availability, governance, descriptor denials, work-surface ceilings, exact Job/run, and platform safety can only narrow them. Hat prose, surface host lists, and cached local approvals do not create grants.

| Decision | Meaning |
| --- | --- |
| **Just this time** | One exact action/target obligation, consumed once; no remembered grant or time-limited Bear-wide permission. |
| **Always for [hat name]** | Admin-authorized Den-owned action/target grant with a truthful future-conversation/eligible-Job audience. A persisted winning obligation and grant must be fenced together; no losing/replayed result can grant authority. |

**Current partial coverage:**

- Den `web_fetch`/supported `web_search` check live source ownership/membership, hat action/destination grants, and Bear blocks. Fetch's exact one-shot URL/continuation remains separate; admin ACP can persist a current-hat exact HTTPS-host grant. Native search filtering covers the supported provider. DNS-vetted pinning, no redirects, and no ambient proxy apply.
- Supported read-only editor filesystem descriptors use fresh Den checks for the current owner/hat/exact root before reuse and before effect. An eligible admin can persist the narrow exact-root ACP choice atomically with its winning permission result. Work, writes, commands, MCP, and fallback prompts do not inherit it; symlink checks are not race-free filesystem enforcement.
- Eligible Work sandbox provisioning intersects current hat hosts with the assigned surface ceiling. Upgraded relays check Den for every new connection using the run token, exact live run, and provisioned ceiling; revoked authority cancels/token-revokes runs and retries teardown. Already-open streams can last up to 60 seconds. Real provider Job/revocation behavior remains unverified.
- There is no ordinary Legacy memory/network/Cargo fallback. Retained historical approval/helper branches cannot admit an unbound source. The unrestricted Cargo helper and automatic Cargo preparation are denied for hat-bound Work; Rust Jobs need a separately grant-enforced dependency cache.
- In-process dispatch, typed descriptor audiences, and connected-armature filtering are present; caller-context `/internal/den-tools/invoke` is retired. They establish source/capability boundaries, not complete per-effect hat grants.

**Next:** implement the common live action/resource `HatGrantResolver` across broader Den effects and descriptor catalogs, provider-owned upstream/git/credential operations, and other client-tool families. Extend bounded `Always for [hat]` choices only with typed target scopes and atomic obligation/grant handling. Do not describe stored grants or origin-filtered catalogs as full effect authorization. Cabinet and other Den effects still need grant intersection.

Test two hats/two humans, revoked membership/hat/Job/Connection, exact Work-run substitution, roots/surfaces changing, disconnect, replay, redirects/DNS/private-IP escape, stale client descriptors, and advertisement versus direct effect. Keep one canonical permission owner; no independent client/gateway allow cache.

**Exit:** every applicable advertisement, direct effect, continuation, and outbound connection intersects the live hat grant with actor/source/resource/credential/safety ceilings; one-shot approvals cannot leak; supported persistent choices have truthful scope and audience. This exit is **not complete**. Retiring configurable stance registration does not imply this resolver exists in full.

## Gate 1 — Set the product contract and build a prototype

The setup card should explain a named responsibility, identity, resources, bounded actions/network, private own-source notes, curated hat knowledge, and Work off/on. Hat knowledge is shared with authorized wearers, including eligible future Jobs; automatic sharing starts off. A populated hat requires full historical memory and identity review before enabling Work. Hat text is data, never permission.

**Exit pending:** prototype claims match implemented effects and truthful approvals. Use existing Bear management/memory pages, not duplicate state. Private-hat audience policy and broader action choices remain product decisions.

## Gate 2 — Prove one complete isolated workflow

Use a test Bear with a real hat, two separate human conversations (one armature-backed), private note, explicitly reviewed/curated hat knowledge, and eligible Job-bound Work. Inspect persistence **and the next model request**. Verify the same hat identity across channel, editor, and Work without copying another hat's full identity or legacy contracts.

Den Postgres owns hats, bindings, grants, transcripts, and Docket; per-Bear SQLite owns memory/publication provenance. Recall indexes only eligible current core/hat records, never source/profile-local records even with zero hats. Stale legacy derived-point cleanup is retried before embedding; canonical old SQLite remains intact. Profile-string recall APIs are removed. Reads still recheck canonical scope/lifecycle/access and reconstruct text, including poisoned/stale payloads. No ordinary run sees another source's raw notes even under the same hat.

**Exit pending:** a real end-to-end allowance/denial/revocation run, including live model curation and sandbox egress. Focused and synthetic-index tests are narrower evidence, not this exit.

## Gate 3 — Inventory and migrate without widening access

- Preserve legacy profile-local records, settings, contracts, imported paths, and audit encodings as restricted historical inspection data. Never infer a source owner from a path or session-like ID, create an autogenerated hat, or import old grants into a default.
- Explicit admin review may cite eligible unknown-owner legacy provenance while authoring new hat content; originals remain historical and unowned. Hat→`core/` publication is a separate explicit audience decision. No automatic import/promotion.
- Audit historical NULL owners, session-ID collisions, same-creator artifact isolation, and remaining session-only event joins/list filtering. Reconcile derived indexes without widening ordinary reads.

**Exit pending:** restore/import tests preserve provenance and deny ordinary legacy paths with recall enabled or disabled; retention/export/deletion policy is explicit.

## Gate 4 — Product surface and architecture decision

**Current WIP:**

- Bear-admin stance detail/configuration/provisioning is retired; `/models` is Bear-wide. Initialization only opens memory, ensures runtime plan, and compiles managed config; it never creates/refreshes a profile registry. Admin per-profile model and profile-registration routes are gone; named Rust type aliases are removed. Profile model/loop rows do not override live selection/control.
- Managed compilation produces bound Bear-wide base and platform interaction modes only, independent of legacy role contracts/metadata. Canonical hat identity is selected at turn time with source-version checks; no production inference selects a role prompt or role contract. Defaults, loop control, continuation, and compaction use verified origin/governance, not stale profile settings.
- Ordinary browser/IDE/Job sources require real hats even with zero hats. Pending IDE materialization follows admission; history inspection does not authorize a turn. Live canonical source admission precedes all production inference and continuations; result recording and direct tools recheck current actor/source/hat and exact Work eligibility. No ordinary Legacy memory/network/Cargo lane remains.
- Pair Plan and shared work-surface scaffold model tools are retired; historical rows/records remain inspection data. Current session-task/Docket state is independent.
- Curate briefing is direct source-verified tool-free inference with repository Markdown instructions, checked against its live Reflection-run/conversation before inference and delivery. Worker briefings consume direct verified `MemorySource`, not role prompts/contracts. Generic system starts/tools/results/continuations are denied.
- Rust `TrustProfile` / `BearProfile` / `BearStance` aliases are removed: derived runtime-context metadata (`RuntimeContextLabel`) has five source-kind labels and retains legacy schema/audit encoding. It is not a configurable stance or authority input. Browser-task origin remains distinct even though its historical encoding is `pair`.

**Remaining:** reconcile documented contracts with executable route/next-request denial evidence, finish Gate 0A's broader grants and persistent choices, improve source-specific history/curation navigation, and audit legacy ownership. Do not reintroduce a no-hat compatibility execution lane. Keep ADR rationale/history; change only implementation notes. Update public shipment claims only after rebuilding and verifying the actual image.

**Exit pending:** an admin can predict identity, memory sharing, and effect authority without configurable stance vocabulary; complete effect/denial and next-request regressions pass; rebuilt-image and real vertical-slice evidence support rollout. Metadata retirement alone is not this exit.

## Implementation landmarks (not complete ownership lists)

- Memory: `services/den/crates/den-memory/`; source/hat binding and identity: `services/den/crates/den-service/src/bears/hats/`.
- Compilation/settings/init: `services/den/crates/den-service/src/bears/managed_blocks.rs`, `context_composition.rs`, `db.rs`, `provision.rs`.
- Admission/continuation/dispatch: `services/den/crates/den-runtime/`, `services/den/crates/den-core/src/effective_policy.rs`, `tools/identity/mod.rs`, `services/den/crates/den-bearwire/`, `services/den/crates/den-docket/`.
- UI contract: `services/den/crates/den-web/src/ROUTES.md`; prompt prose: `services/den/prompts/`.

Check documentation links, then rebuild and test the stack/real vertical slice before claiming shipment. See the [maintained topic](../topics/bear-memory-hats.md) for current claims and validation limits.
