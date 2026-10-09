# Planning index

This is the entry point for *verified* active delivery plans. A plan is not a description of deployed behavior: read the [topic map](../README.md) for current behavior and the relevant decision records for rationale.

## Plans to reconcile

The older [planning hub](PLAN.md) contains useful priorities and delivery history, but its dated status claims have not yet been reconciled against the current implementation. Do not treat its entire plan catalog as active. The [BearWire v1 refinement draft](BEARWIRE_V1_PROTOCOL_REFINEMENT_ROADMAP.md) and [Docket implementation plan](DOCKET_IMPLEMENTATION_PLAN.md) are linked from their topic pages with explicit caveats. The [focused-execution stabilization plan](pair-execution-authority-and-debug-tracing-plan.md) is completed delivery history.

## Draft design and protocol work

- [Bears app client evolution](MACOS_BEARS_CLIENT_APP_PLAN.md) — agreed separation of human administration/Docket APIs, a conversation client and optional local armature management. UI design and AHP-versus-AG-UI evaluation remain open; implementation is gated on review and approval. Real-time voice/video and external agent interoperability are excluded.

## Active contract and validation work

- [Hats and session memory boundaries](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) — validate the memory and authority contract and hat UX with one isolated workflow before any general migration or stance removal. Includes the [approved credential-mediation and external key-management direction](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#runtime-mediated-credentials-and-external-key-management-planned): runtime-wrapped credential use, `secrecy`/`zeroize`, existing Connections ownership, and one external backend/vertical slice before expansion. Backend selection and implementation remain pending. The current-system evidence and target are separated in the [Bear memory and hats topic](../topics/bear-memory-hats.md).

## Adding or advancing a plan

- Start from an existing [topic page](../README.md); link the plan from that page and back to it.
- New plans state **Status:** `Draft`, `Active`, `Completed`, or `Superseded` and **Topic:** a link to their home. Index them here in the same change. Archive completed plans only after the topic's current behavior has been reconciled.
- Separate target outcomes and exit criteria from implemented milestones. Completing an ADR or a plan does not by itself prove that its intended behavior is deployed.
