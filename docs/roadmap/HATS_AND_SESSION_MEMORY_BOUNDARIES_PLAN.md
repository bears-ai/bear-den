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

**Exit:** every applicable advertisement, direct effect, continuation, and outbound connection intersects the live hat grant with actor/source/resource/credential/safety ceilings; one-shot approvals cannot leak; supported persistent choices have truthful scope and audience. Credential-backed actions must also meet the mediation and exposure criteria below. This exit is **not complete**. Retiring configurable stance registration does not imply this resolver exists in full.

### Runtime-mediated credentials and external key management (planned)

**Direction approved by the project owner 2026-10-08; first bounded code/mock vertical slice delivered, external backend and broader cutover pending.** Models request a bounded operation, not access to its key. Credential-bearing tools must be wrapped by the trusted runtime/provider adapter. Encryption at rest, an opaque reference, a prompt warning, or output redaction alone does not establish this boundary.

#### Approved implementation direction

1. Use `secrecy` and `zeroize` for in-process secret handling, with explicit exposure confined to trusted adapter edges. The narrow `den-repository` adapter now implements this handling approach; legacy configuration, gateway and provider paths are not automatically migrated.
2. Keep existing owner-scoped Connections as the canonical identity, authorization and credential-reference layer. Do not create a parallel credential authority or expose secret-backend locators to models.
3. Delegate key custody and lifecycle to an established external KMS/secret service; do not build a Den KMS. Select one backend before production integration: AWS KMS or a managed secret store where cloud operations fit, or Vault/OpenBao where the operator accepts self-hosted service responsibilities. The deployment/backend choice remains open; approval of this direction does not select AWS, Vault or OpenBao or authorize a Compose change.
4. Deliver one credential-backed provider operation end to end first. Prove authorized outbound authentication works while credential material stays out of model requests, general tools, model-controlled processes and all persistence/projection paths; then expand to other tools and MCP/provider families.
5. Treat OS keyring integration as a separate local app/armature bootstrap option, not the initial Den backend or a prerequisite for the first vertical slice.

#### Boundary and ownership

- Extend the existing owner-scoped Connections path rather than create another independently writable credential store. Den owns connection identity, authorized resource bindings, grants and revocation; a selected secret backend owns secret material or key custody. A backend locator/version is a typed internal reference, not a model-controlled path or permission. A hat grant does not imply ownership of its human's Connection.
- Model-facing schemas expose only the operation's validated business arguments and, where necessary, an authorized logical resource/Connection handle. They must not accept or return credentials, credential-store paths, arbitrary authenticated URLs/headers, or shell snippets intended to use a key. Connection discovery reports availability and missing requirements, never secret values.
- At execution, a descriptor-owned adapter verifies the live actor, immutable source, hat action/resource grant, current Connection authorization, exact eligible Work run where applicable, and the operation's destination/account/resource ceiling. Only then may it resolve a credential for the exact outbound effect. Possessing a reference is not authorization. Replay, continuation and retries repeat the checks.
- Inject authentication only at the trusted HTTP/provider boundary, with bounded lifetimes and minimum scopes. Prefer short-lived provider credentials when supported. Prevent redirects, caller-supplied destination substitution and proxy configuration from forwarding authentication outside the authorized destination. Remote KMS/secret-store access needs its own operator-configured TLS, identity and egress policy, not a model tool grant.
- Do not place runtime-managed secret material, including short-lived tokens, in prompts, tool arguments/results, Bear memory, transcripts, compaction/replay, exports, model-readable files, command arguments, or the environment of model-controlled subprocesses. General filesystem/process/terminal tools must not expose the runtime's credential store or inherit its credential-bearing environment. An isolated credentialed provider/MCP process may be needed, but credentials must not reach a process that also permits arbitrary model-authored code or credential inspection. Audit existing managed credential files, MCP setup and subprocess inheritance; do not assume current `0600` files alone satisfy this requirement.
- Return allowlisted, structured business results and errors. Filter provider response fields/headers, echoed requests, stdout/stderr, tracing and diagnostics before any model or transcript projection. Redaction is defense in depth, not the primary access boundary. Record non-secret audit evidence for authorization, credential use/version, destination, failure and revocation; never record the material itself.
- Keep decrypted material in narrow secret-bearing types with explicit exposure only at adapter edges, minimal copying/lifetime, redacted debugging and zeroization on drop. This reduces accidental leaks, not disclosure by arbitrary code in the same process, crash dumps, pre-existing copies, or a compromised host. Inventory residual exposure and state those limits explicitly.

#### First bounded effect — repository head (code/mock tested; not deployed)

`repository_head` (`den.repository.head`, `repository.head.read`) accepts only a managed `work_surface_id` and returns only that UUID plus a validated configured-branch commit SHA. The trusted adapter builds a fixed GitHub API request, resolves/validates an internal versioned external-credential lease, pins public DNS/TLS, disables redirects/proxy/retries, and bounds connect/total/body size. Its `SecretString` and owned zeroizing buffers remain outside model args/results, files, processes and MCP; authentication is exposed only at the HTTP header edge. Header/TLS library copies, host compromise and other preexisting paths remain explicit limitations.

Canonical Connections add mutually exclusive external-reference material, not a new secret store. References are inactive until a trusted external resolver is integrated; production defaults to typed `credential_unavailable` with **no ciphertext/environment fallback**. External material cannot be exported as managed Git credentials or silently treated as anonymous/legacy setup. Queued/live/paused Work blocks replacement/detachment. Exact repository grants bind surface, validated coordinates/branch, Connection identity/revision and versioned reference; changes invalidate approval. One service resolver intersects current actor/source/hat, Bear/hat resource, exact action/target/host/block, owner/current-manager and exact Work Job/run authority before lookup and before/after HTTP. Shared hat membership does not lend another human's Connection.

Evidence: authenticated mock HTTP, denied-call counters, revocation/change/ref-substitution and endpoint/response-echo tests, migration clean rollback/reapply and populated refusal, plus successful native Chat Completions **and** Responses execution → automatic canonical persistence → fresh next-turn replay with SHA/tool exchange and no canary/internal refs. Genuine in-process hosted-tool EOF now hands the batch to its owning executor rather than prematurely abandoning it; model-visible tool exchanges use the shared hidden-user projection factories. Historical diagnostic constructors/rows are not promoted. The known diagnostic-client history regression remains separate.

Dependency scope: new `secrecy 0.10.3` plus existing `zeroize 1.9` live in the adapter; root/service uses for fixtures are dev-only. Existing HTTP/serde/crypto dependencies are reused, and no cloud SDK, key-management service, provider deployment or Compose change is selected. `scripts/sqlx.sh migrate add` invokes installed SQLx directly for local version generation, without starting infrastructure; prepare/apply still use the isolated-database wrapper and all-target cache.

**Remaining production gate:** choose and integrate an established backend with workload identity/TLS/egress, lease/version/rotation/revocation/recovery and credential-migration evidence. Then perform live GitHub acceptance. No production-ready credential custody is claimed from the injected test interface.

**Residual source inventory:** Config/telemetry legacy credential strings and Debug/clone paths; legacy Bifrost plaintext reads; decrypted managed repository credentials, provider files, Git helper subprocess/environment delivery; armature/MCP environment inheritance; unrelated authenticated provider errors/results. These are not claimed fixed by this one HTTP adapter. Inventory is source evidence, not proof that each path currently leaks; broader isolation and exposure-matrix acceptance remain open.

#### Reuse established components; do not build a KMS

The existing `aes-gcm` crate supplies RustCrypto authenticated-encryption primitives; it does not supply key custody, access policy, rotation, recovery or secret distribution. Retain vetted cryptographic primitives where needed, but do not invent a key-management service, key hierarchy or rotation protocol in Den.

Extend the implemented narrow `secrecy`/`zeroize` boundary to other credential paths only with independent authorization/exposure evidence, keeping dependencies scoped and inspecting feature/build impact. Evaluate one operationally suitable external backend first; the service and client adapter are not yet selected. The following distinguishes the selected handling libraries from backend/bootstrap candidates:

| Component | Role and limits |
| --- | --- |
| [`secrecy`](https://docs.rs/secrecy/latest/secrecy/) + [`zeroize`](https://docs.rs/zeroize/latest/zeroize/) | **Selected in-process approach.** Explicit secret access, prevention of accidental Debug/serialization leaks, and memory cleanup. Not storage, access authorization, a KMS, or protection from a model-controlled process. The new narrow adapter directly uses `secrecy 0.10.3` and existing `zeroize 1.9`; these wrappers do not claim to migrate legacy paths. |
| [`aws-sdk-kms`](https://docs.rs/aws-sdk-kms/latest/aws_sdk_kms/) | Official Rust client for AWS-managed key custody/encryption. KMS is not an arbitrary credential catalog; Connections still need a storage/reference model, or an established managed secret store. Assess IAM/workload identity, audit, region, latency, availability, cost and toolchain compatibility. |
| [`vaultrs`](https://docs.rs/vaultrs/latest/vaultrs/) with Vault/OpenBao | Community Rust client for an established external service's KV, Transit and lease APIs. Prefer service-managed encryption or secret storage over custom key lifecycle code. Verify the selected server/version/auth/Transit operations with integration tests; client maturity/support is not the same as the service's maturity. Self-hosting adds unseal/bootstrap, storage/backup, TLS, audit, availability and operational obligations. |
| [Rust Keyring ecosystem](https://docs.rs/keyring/latest/keyring/) | Candidate for local app/armature bootstrap credentials using platform stores. Not a distributed Den KMS or a blanket fit for headless Linux containers. Choose explicit platform backends rather than enabling every store. |

Separate master-key rotation/rewrapping from rotation and revocation of the upstream API credential. Plan versioned references, fail-closed backend outages, bounded credential caching/leases, workload-auth bootstrap, backup/restore and recovery, and explicit migration of existing encrypted and legacy plaintext records. Preserve one canonical owner and do not silently fall back to an environment key or plaintext when the selected backend is unavailable. Service adoption may introduce network/data-sharing costs and a new operational dependency; keep deployment compatible with the single root Compose contract and obtain approval before any Compose changes.

#### Delivery and acceptance

- [ ] Inventory all key entry, storage, decryption, provider/MCP delivery, subprocess and projection paths, including legacy Bifrost plaintext reads and managed repository credential files. Establish what is already isolated and which paths can expose material to a model.
- [ ] Introduce typed internal credential references and `secrecy`/`zeroize` handling at the narrow owning boundaries; audit Debug/serialization, cloning and plaintext lifetimes. Verify dependency/features and toolchain compatibility without broadening core layers unnecessarily.
- [x] Wrap one credential-backed action end to end using typed Connection references, live authorization and trusted destination-bound injection. The new repository-head action has no raw-key/ambient-environment path; authenticated mock/native persistence and next-model tests contain no credential bytes. Production custody/live acceptance remains the separate unchecked backend gate below.
- [ ] Select and integrate one established secret/key backend with tested versioning, rotation/revocation, outages and recovery, plus an explicit migration for existing credentials. Do not widen production dependencies or require multiple backends before the first vertical slice is proven.
- [ ] Expand the wrapper boundary to the remaining credentialed tools and provider/MCP families; link evidence back to this gate rather than claim coverage from a generic secret helper.

**Exit evidence:** canary secrets remain absent from initial and next-turn model requests, tool schemas/arguments/results, replay/compaction, memory/recall, exports, logs and error projections. Adversarial tests cover a provider echoing credentials; redirects/destination/account substitution; model attempts to read files, dump environment/process state or execute code in a credentialed process; two humans/two hats/Connections; revoked ownership/membership/Connection/key; stale refs, retries and replay; cross-record ciphertext/ref substitution; backend failure and key/credential rotation. Verify actual outbound authentication works without delivering its material to the model. These checks target runtime-managed credentials; they do not guarantee arbitrary user-pasted content contains no secrets or protect against compromise of the trusted runtime/host.

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
