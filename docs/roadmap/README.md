# Planning index

This is the entry point for *verified* active delivery plans. A plan is not a description of deployed behavior: read the [topic map](../README.md) for current behavior and the relevant decision records for rationale.

## Plans to reconcile

The older [planning hub](PLAN.md) contains useful priorities and delivery history, but its dated status claims have not yet been reconciled against the current implementation. Do not treat its entire plan catalog as active. The [BearWire v1 refinement draft](BEARWIRE_V1_PROTOCOL_REFINEMENT_ROADMAP.md) and [Docket implementation plan](DOCKET_IMPLEMENTATION_PLAN.md) are linked from their topic pages with explicit caveats. The [focused-execution stabilization plan](pair-execution-authority-and-debug-tracing-plan.md) is completed delivery history.

## Draft design and protocol work

- [Bears app client evolution](MACOS_BEARS_CLIENT_APP_PLAN.md) — agreed separation of human administration/Docket APIs, a conversation client and optional local armature management. UI design and AHP-versus-AG-UI evaluation remain open; implementation is gated on review and approval. Real-time voice/video and external agent interoperability are excluded.

- [Bear memory forgetting](BEAR_MEMORY_FORGETTING_PLAN.md) — draft skeleton requested 2026-10-23 for broader aging, expiry, retention and forgetting. No schedules or automation approved. Explicit owner-requested personal-body/derived-copy erasure remains required by the active user-associated memory plan; it is not deferred here.

## Active product delivery

- [Cabinet implementation](CABINET_IMPLEMENTATION_PLAN.md) — shared Den knowledge, page policy/review, artifacts and recall. [Phase 5](CABINET_IMPLEMENTATION_PLAN.md#phase-5--distinct-cabinet-surface-and-external-mcp) is in progress: code/test implementation includes the Cabinet-only entrypoint, distinct UI/cross-host links, page discussion/history and an initial authenticated read-only MCP adapter. The approved root Compose `cabinet` profile, production image and isolated local-HTTPS acceptance are validated; actual public-host rollout and native-browser/live-storage acceptance remain open. It does not extract Cabinet from Den. Bear administration remains Cabinet-aware and hands off live content details/manipulation. Current behavior and delivery caveats live in the [Cabinet guide](../guides/cabinet.md).

- [User-associated Bear memory](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md) — active: Phase 4B committed as `3d079b9d`, Phase 5A as `582505be9`; [Phase 5B provider-error containment](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-5b--provider-error-containment) committed as `52bdd3f7`; [Phase 5C-1 deny-first dependency/replay foundation](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-5c-1--deny-first-dependencyreplay-foundation) committed as `84f916d2`; [Phase 5C-2 value-free retained-reference freshness](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-5c-2--value-free-retained-reference-freshness) committed as `7ce23c6d`. The [2026-10-23 v1 policy amendment](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#approved-v1-policy-amendment-2026-10-23) approves preferred-name-only explicit consent/owner approval and explicit body/derived-copy deletion; potential future categories are documented separately. Provider policy remains undecided; automatic forgetting has its own draft. Typed content-free inference failures preserve retry/fallback policy and terminate SSE errors before trailing-frame diagnostics or success. Typed metadata now blocks marked transcript, session and artifact reuse before projection/derivation effects; DB regressions and strict workspace checks pass. Zero new test-only SQL/dependencies, one production query/cache replacement, no personal-context consumer or deployment. Value-free revalidation now checks current rights and retained binding/head freshness without reading the name (236 memory, 33 service, 22 web tests pass), with no further SQL/cache changes. Continue actual-input dependency propagation, authorized replay/secondary-output/derivation/in-flight and every-send/output gates before provider transport; Phase 5 as a whole, Phase 4 explicit-erasure/replay gates and ordinary personal recall remain incomplete; general retention/automatic forgetting is separate future work.

## Active contract and validation work

- [Hats and session memory boundaries](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) — validate the memory and authority contract and hat UX with one isolated workflow before any general migration or stance removal. Includes the [approved credential-mediation and external key-management direction](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#runtime-mediated-credentials-and-external-key-management-planned): runtime-wrapped credential use, `secrecy`/`zeroize`, existing Connections ownership, and one external backend/vertical slice before expansion. Backend selection and implementation remain pending. The current-system evidence and target are separated in the [Bear memory and hats topic](../topics/bear-memory-hats.md).

## Adding or advancing a plan

- Start from an existing [topic page](../README.md); link the plan from that page and back to it.
- New plans state **Status:** `Draft`, `Active`, `Completed`, or `Superseded` and **Topic:** a link to their home. Index them here in the same change. Archive completed plans only after the topic's current behavior has been reconciled.
- Separate target outcomes and exit criteria from implemented milestones. Completing an ADR or a plan does not by itself prove that its intended behavior is deployed.
