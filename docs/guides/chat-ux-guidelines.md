# Chat UX guidelines

This guide defines how Bear Den presents conversation and runtime activity across web chat, Pair over ACP, `work` sandbox projections, and future chat surfaces.

It complements the transcript and protocol contracts. It does not replace channel-specific rendering or permission rules.

## Model-context visibility invariant

Every non-secret item delivered to a model must have a durable, user-visible transcript projection. A projection may be compact by default, but model delivery must not be invisible.

Expanded detail must expose the exact delivered representation, or a stable reference that resolves to it, together with:

- source subsystem and triggering reason;
- delivery order and model invocation or run identity;
- whether the item was persisted, shown to the user, sent to the model, or used to derive another context artifact;
- redaction status.

A concise label, such as `Den: sent checkpoint challenge`, is a summary, not a substitute for the delivered content.

### Redaction

Credentials, third-party secrets, and security-sensitive values may be withheld. Redaction must itself be visible: identify that protected content was delivered, why its value is unavailable, and the delivery metadata. Never silently omit model-visible content.

## Event and card patterns

Runtime-originated content is a structured transcript event, not assistant-authored prose. Render it with a distinct Deep Chat custom role or equivalent card presentation and metadata. Assistant messages remain reserved for assistant answer content.

| Event kind | Compact card | Expanded detail |
|---|---|---|
| Model-context delivery | `Den: sent checkpoint challenge` | Exact payload, source, reason, invocation/run IDs, delivery order, redaction state |
| Tool activity | `Read src/main.rs` | Arguments, target, status, result, bounded raw input/output, permissions where relevant |
| Work-log projection | `Work: cargo test failed` | Summary/excerpt, stdout/stderr detail, artifact references, and projection/delivery status |
| Runtime lifecycle | `Den: compacted earlier context` | Policy/version, source range, derived artifact, and affected invocation |
| Protected-context delivery | `Den: supplied protected context` | Category and reason for withholding the value, plus delivery metadata |

Use target-first, stable labels and progressive disclosure. The one-line card should explain the user-meaningful event; raw payloads are diagnostics in the expanded view. Follow [ADR-0049](../decisions/adr-0049-acp-tool-call-and-permission-ux.md) for tool and permission semantics.

## Delivery and projection semantics

These facts are independent and must be represented independently when applicable:

1. visible to the user;
2. persisted in the transcript;
3. sent to the model;
4. used to derive a summary or other context artifact.

For example, a sandbox log can be displayed without being sent to a model, sent to a model without being displayed in full, or summarized into a separate model-context delivery. Cards must say which occurred rather than implying equivalence.

A context-delivery card must identify its delivery boundary. When retries, compaction, or context-window selection produce different prompts, users must be able to determine which invocation received which content.

Live and replayed projections must preserve canonical ordering. Context-delivery cards, tool cards, work-log cards, and assistant streaming content use the same per-conversation ordered projection path; concurrent producers must not reorder them.

## Renderer conventions

Deep Chat surfaces should use custom roles and `custom` data (or an equivalent typed event payload) for runtime cards. Preserve enough structured data to render the compact card and inspect the canonical record without parsing display text. At minimum, include event kind, source, model-visibility state, invocation/run reference, ordering reference, and redaction/expandability state.

Do not use model reasoning or private scratchpad as assistant text. This invariant is about context supplied by Den and connected runtimes, not disclosure of provider-private reasoning.

## First-chat setup and HTTP failure rendering

- Creating a hat does not bind a conversation. A fresh Bear's Chat view should explain how to choose that hat under **+ New chat**, create the owned conversation, and then send. Do not manufacture an executable default thread on empty or failed inventory, including on a zero-hat Bear.
- Metadata inspection is not admission or mutation: viewing model settings must not create an unbound conversation. Keep authorized history/model reads separate from send/configuration eligibility.
- Use the server's explicit send eligibility and canonical/display selection mapping. An exact authorized older history link must not be lost merely because it falls outside a recent inventory page; an unavailable link must not silently become another conversation.
- Chat API failures should be typed JSON with safe recovery text and a request reference matching the response header. General navigation-page HTML is not a chat error payload. Render status/recovery for HTML/proxy/non-JSON failures, never their raw bodies; do not echo serialized provider diagnostics or credential-bearing context.
- Error references and business messages must remain literal text, not executable links/Markdown. Sign-in redirects have their own handling. Clear only the relevant operation/selection's error after recovery; do not hide an unrelated failed mutation.
- Serialize model mutations and reread canonical state before accepting another choice. Fence stale create/list/history/model callbacks against selection and draft changes; do not remount a newer chat merely because an older request completes.

These rules describe the current branch's browser follow-on, not proof of deployment. See [current contract and validation limits](../topics/bear-memory-hats.md#browser-first-chat-and-error-boundary-follow-on-2026-10-09).

## Related contracts

- [Conversation persistence and archive model](../architecture/den-conversation-persistence-and-archive-model.md) defines the shared, append-only transcript source of truth.
- [BearWire JSON specification](../architecture/bearwire-json-spec.md) defines replayable runtime activity and message/reasoning separation.
- [ADR-0007: BearWire protocol](../decisions/adr-0007-bearwire-protocol.md) requires ordered live-surface projection.
- [ACP Runtime Contract](../architecture/acp-runtime-contract.md) defines ACP's adapter boundary.
- [Deep Chat styling](deep-chat-styling.md) records implementation-specific renderer conventions.
