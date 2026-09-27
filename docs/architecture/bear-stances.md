# Bear stances: chat, pair, curate, work, and watch

This document describes the five internal stances Bear Den uses. It is the core reference for stance names, stance responsibilities, cross-stance cooperation, and stance-facing product language. Other current architecture and guide docs should prefer linking here rather than restating the full stance model. A partial [session-scoped memory and hats cutover](../topics/bear-memory-hats.md) changes memory access for explicitly bound turns; unbound sessions still use the stance-local branches described here. See the [active plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) for remaining work rather than treating this conceptual reference as a claim that the new model is fully deployed.

A Bear should feel like one coherent assistant to a user. The preferred conceptual model is **stances, channels, and work surfaces**, not Spaces or separate provider-managed agents. Internally, Bear Den uses a multi-stance runtime. Each stance has a distinct job, trust contract, memory branch, and relationship to external systems.

Stances are the preferred conceptual vocabulary. They are useful for code, schemas, routing, provisioning, diagnostics, architecture discussion, and user-facing explanation when a boundary matters. The Bear should still identify as the Bear rather than as a separate stance runtime or sub-agent.

## Status and relationship to other docs

This is the durable conceptual source for the five Bear stances: what they are, why they exist, how they cooperate, and how we should talk about them.

For the post-Letta split between **trust**, **armature**, and **work surfaces** (especially `chat` vs `pair`), see [`interactive-stances-and-role-axes.md`](interactive-stances-and-role-axes.md).

The five stances are durable trust-and-memory contracts (`BearStance` in code; `BearProfile` remains a temporary compatibility alias). How a particular run is *supervised right now* (live, disconnected, autonomous, inspected, frozen) is a separate, orthogonal **governance** (`Governance` in code) on the run / workspace session. A Bear going offline mid-session is a governance transition, not a switch from `pair` to `work`. See [ADR-0039](../decisions/adr-0039-trust-profiles-and-governance.md).

Long-running continuation also has an objective axis: the **focused Job**. `work` normally requires a designated Docket Job and drives the next logical incomplete task for that Job under autonomous-continuation governance. `pair` normally has no focused Job, but can enter focused-Job behavior explicitly through Bear conversation or client command while keeping the `pair` trust stance. The loop-control details live in [ADR-0050](../decisions/adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md).

For how **work surfaces** relate to **conversations** (including “start a conversation with this repository”), see [`../guides/work-surfaces-and-conversations.md`](../guides/work-surfaces-and-conversations.md).

It is not the implementation spec for provisioning, prompt hashes, tool ids, runtime lifecycle, or database reconciliation. Those details live in the Den spec. It is also not the decision record explaining why the architecture was chosen. That rationale lives in ADRs.

| Document | Audience | Purpose |
|----------|----------|---------|
| [`bear-stances.md`](bear-stances.md) | Product, design, engineering, docs, marketing, support | Canonical conceptual model and shared language for the five internal stances. |
| `docs/decisions/*` | Engineering and architecture | Historical decision records and rationale. |
| Runtime/provisioning specs such as `den-bear-spec.md` | Engineering implementation | Den-owned runtime and provisioning behavior. |

When current docs disagree, treat this document as the source for cross-functional stance meaning and messaging, ADRs as the source for historical architectural rationale, and runtime specs as the source for implementation behavior.

## The core idea

A Bear is one assistant that can operate through five coordinated stances:

| Stance | Plain-English implementation name | Primary job | Common channels or invocation style |
|--------|-----------------------------------|-------------|-------------------------------------|
| `chat` | Conversational agent | Chat with people in chat channels and capture task intent. | Slack, web chat, Discord, future chat surfaces. |
| `pair` | Collaborative agent | Work alongside a person inside tools such as IDEs. | ACP clients, IDEs, Cowork, Figma plugins, future client tools. |
| `curate` | Internal integrator | Decide what becomes shared memory, shared capability, approved work, or reviewed observation. | Not directly user-facing. |
| `work` | Outbound executor | Carry out approved scheduled or event-triggered work against external systems. | Not conversational; invoked by Den task dispatch. |
| `watch` | Inbound observer | Receive external events and turn them into structured observations for review. | Webhooks, polling, queues, subscriptions, streams. |

The split lets a Bear be conversational, collaborative, reflective, autonomous, and observant without giving every capability to one all-powerful runtime. The stance is the operating mode and trust boundary; the channel is the concrete touchpoint; the work surface is the durable work context the Bear may be acting on.

## Why five stances?

The five-stance model supports five product and safety goals at once:

1. **One coherent Bear, many contexts.** Users experience one assistant, while the system routes different contexts to the right internal stance.
2. **Better concurrency.** Chat, IDE collaboration, background work, and inbound events can proceed without all traffic bottlenecking through one stateful agent.
3. **Cleaner memory.** Raw interactions stay in stance-specific branches until `curate` promotes durable knowledge into shared `core/` memory.
4. **Safer autonomy.** No single stance combines broad private data, outbound external communication, and unrestricted durable state mutation.
5. **Clearer product language.** Each stance has a stable purpose that can guide UI, documentation, onboarding, data modeling, and marketing.

## Stance summaries

### `chat`: conversational agent

`chat` is the Bear stance people meet in chat. It handles synchronous conversation in Slack, web chat, Discord, and similar text-in/text-out channels.

`chat` should be understood as the Bear's conversational front door. It can answer questions, help users think through work, use appropriate channel tools, and write down task intents when a user asks for external or autonomous work.

`chat` does not directly perform arbitrary outbound autonomous work. If a user asks for something like “check this every morning,” “post this to another system,” or “monitor that service,” `chat` captures the intent in a structured form so `curate` and Den can review and route it.

**Good shorthand:** “the stance that talks with you in chat.”

**Primary responsibilities:**

- Hold synchronous conversations in chat-like surfaces.
- Use the Bear's shared `core/` knowledge and its own `chat/` memory.
- Capture external-effect requests as task intents.
- Propose durable skill changes instead of installing them directly.
- Keep the user's experience coherent: the user is talking to the Bear, not to a random sub-agent.

**Intentional limits:**

- No direct autonomous outbound work.
- No access to `pair`, `curate`, `work`, or `watch` branches.
- No unilateral promotion of memories into shared `core/`.

### `pair`: collaborative agent

`pair` is the Bear stance for working side-by-side with a user inside a client tool. Its most important early surface is ACP-based IDE or tool integration.

`pair` differs from `chat` because it is embedded in an active working environment. It may see project context, user-approved tool results, editor state, design documents, or other client-side resources. External effects are mediated through the client and gated by the user's approval flow.

`pair` should feel like a collaborator sitting next to the user. It can help edit, reason, debug, design, and navigate the user's working context, while preserving the boundary that client tools are user-mediated.

**Good shorthand:** “the stance that pairs with you inside your tools.”

**Primary responsibilities:**

- Collaborate through ACP-speaking clients such as IDEs and future design/productivity tools.
- Use client-mediated tools with user approval where appropriate.
- Write durable notes to its own `pair/` branch.
- Capture external-effect requests as task intents.
- Propose durable skill changes instead of installing them directly.

**Intentional limits:**

- No direct access to chat-channel memory branches.
- No autonomous outbound work outside the client-mediated permission model.
- No unilateral promotion of memories into shared `core/`.

### `curate`: internal integrator

`curate` is the Bear's internal integrator. It reads across the Bear's branches, reflects on and reorganizes accumulated activity, promotes durable knowledge into shared `core/`, reviews task intents and watch observations, promotes work results, and governs skill learning.

It is the primary semantic authority for what becomes shared Bear memory or shared Bear capability. Den enforces and installs those decisions.

`curate` is deliberately not user-facing. It exists so the Bear can learn, remember, approve, reject, summarize, and integrate without giving every outward-facing stance broad authority over shared memory or autonomous action.

**Good shorthand:** “the stance that decides what the Bear learns and what becomes durable.”

**Primary responsibilities:**

- Read across the Bear's stance branches.
- Reflect on and reorganize accumulated activity.
- Promote durable knowledge into shared `core/` memory.
- Review task intents from `chat` and `pair`.
- Review observations from `watch`, potentially generating derived task intents.
- Review and promote results from `work`.
- Review skill proposals from any stance.
- Choose stance applicability for approved skills.
- Update the Bear skill manifest through Den.

**Intentional limits:**

- No outbound external communication tools.
- No direct write access to other stances' branches.
- Cross-branch mutations and external effects flow through Den-controlled tools.

### `work`: outbound executor

`work` is the Bear stance for approved external action. It executes scheduled, event-triggered, or otherwise approved tasks against external systems.

`work` should not be treated as another conversational agent. Its job is structured execution: call the APIs, run the research, perform the scheduled check, create the summary, or interact with an integration according to a task definition that has already passed through review and policy.

`work` sees curated context rather than raw channel history. This is central to the safety model: the stance that can act outward should not be directly exposed to every prompt injection or raw private exchange that arrived through chat, IDEs, or webhooks.

**Good shorthand:** “the stance that does approved outbound work.”

**Primary responsibilities:**

- Execute approved tasks dispatched by Den.
- Use only the tools and scopes allowed by the task definition and run context.
- Read shared `core/` knowledge and its own `work/` memory.
- Write task results and execution notes to `work/`.
- Propose reusable execution procedures as skill proposals.

**Intentional limits:**

- No raw access to `chat`, `pair`, `review`, or `watch` branches.
- No self-approval of tasks.
- No use of tools outside the approved task scope.
- No direct conversational surface.

### `watch`: inbound observer

`watch` is the Bear stance for inbound external events. It receives webhooks, polling results, queue messages, subscription updates, and other external signals, then writes structured observations for `curate` to review.

`watch` is the inbound counterpart to `work`. Where `work` reaches outward to do approved tasks, `watch` listens inward for relevant events. It should not take outbound action on its own. An inbound event can inform the Bear, but it must pass through observation and curation before it causes external action.

**Good shorthand:** “the stance that listens for external events.”

**Primary responsibilities:**

- Receive subscription and event payloads from Den.
- Parse or summarize inbound events into structured observations.
- Write observations to its own `watch/` branch.
- Use shared `core/` context to interpret events where appropriate.
- Propose reusable subscription parsing or handling procedures as skill proposals.

**Intentional limits:**

- No outbound action capability.
- No direct access to `chat`, `pair`, `review`, or `work` branches.
- No direct promotion of observations into shared memory.
- No direct conversion of events into external action without `curate` and Den mediation.

## How the roles cooperate

The five stances form a flow from raw interaction to durable memory and approved action:

1. A person talks with `chat` or works with `pair`.
2. `chat` or `pair` answers directly when the request fits the synchronous surface.
3. If the request implies durable learning, the stance writes notes or proposes a skill.
4. If the request implies external or autonomous work, the stance writes a task intent.
5. `watch` may independently receive external events and write observations.
6. `curate` reviews memories, task intents, observations, skill proposals, and work results.
7. `curate` promotes durable knowledge into `core/` and uses Den-controlled tools to approve or reject cross-stance changes.
8. Den dispatches approved external tasks to `work`.
9. `work` executes within its approved scope and writes results.
10. `curate` reviews those results and promotes durable learnings back into `core/`.

In short:

- `chat` and `pair` are the synchronous user-facing stances.
- `watch` is the inbound external-events stance.
- `curate` is the semantic integration and review stance.
- `work` is the approved outbound execution stance.

## Trust model in product language

A Bear is powerful because it can remember, collaborate, observe, and act. The stance split keeps those powers from concentrating in one place.

| Stance | Private/raw context | External communication | Durable state |
|--------|---------------------|------------------------|---------------|
| `chat` | Chat/channel context | Conversation only | Own branch |
| `pair` | Client/session context | Client-mediated and user-gated | Own branch |
| `review` | Broad Bear context | None | Own branch and shared `core/` |
| `work` | Reviewed context only | Outbound approved work | Own branch |
| `watch` | Inbound payloads and curated context | Inbound only | Own branch |

This lets us say, accurately, that Bears can support autonomy while keeping raw inputs, memory integration, and external action separated by stance and policy.

## Messaging guidance

### Preferred language

Use stance/channel/work-surface language for ordinary explanation:

- “A Bear feels like one assistant, and Den routes different kinds of work through different stances.”
- “Each stance has a clear job and a clear trust boundary.”
- “The `chat` stance is where the Bear talks with people in chat-like channels.”
- “The `pair` stance is where the Bear works alongside a person in a client or workspace.”
- “The `curate` stance reviews what becomes shared memory.”
- “The `work` stance performs approved external tasks.”
- “The `watch` stance receives external events and records observations.”

Use implementation detail carefully:

- “Den projects the Bear into a runtime for the appropriate stance.”
- “The `curate` stance reviews and integrates durable knowledge.”
- “The `chat` and `pair` roles are the two synchronous user-facing roles.”

### Avoid

Avoid language that implies:

- A Bear is five unrelated bots.
- A Bear should introduce itself as a stance agent.
- Stances are separate assistant identities.
- Every stance can do every task.
- Chat surfaces directly execute arbitrary autonomous work.
- The event listener can take outbound action on its own.
- Shared memory is a dumping ground for every raw interaction.
- `curate` is merely a summarizer; it is the semantic integration and review authority.
- “Space” is the primary conceptual layer users need to understand.

### User-facing naming

The stance names `chat`, `pair`, `curate`, `work`, and `watch` are the preferred stable vocabulary.

In normal user-facing behavior, a Bear should identify itself as the Bear rather than volunteering its internal stance label. The internal stance split is primarily an implementation and trust-boundary model, not the default self-description users should hear.

This means:

- `chat` and `pair` should normally speak in the voice of the Bear, not as “the chat stance,” “the pair stance,” or a separate agent.
- Internal stance names should be exposed mainly in Bear Den-building, operator, debugging, or other explicitly architectural contexts.
- Product surfaces may still use friendlier activity labels such as “chat,” “pairing,” or “background work,” but should avoid making the user feel like they are talking to five separate assistants.
- When a boundary explanation is necessary for honesty or safety, the system may briefly describe the relevant internal distinction without centering it as the assistant's identity.

For example:

| Role | Possible user-facing language |
|------|-------------------------------|
| `chat` | Chat, conversation, ask your Bear |
| `pair` | Collaborate in your IDE, work together, pairing |
| `review` | Memory review, learning, integration |
| `work` | Background work, approved tasks, automations |
| `watch` | Monitoring, subscriptions, event listening |

## Design and data-model implications

The five stances should shape product and data design:

- User-facing conversation history belongs primarily to `chat` or `pair` channels, not to `work` or `watch`.
- Background tasks should be represented as reviewed work, not as hidden chat side effects.
- Subscription events should become observations before they become actions.
- Durable shared memory should be explainable as something `curate` promoted, not something every stance writes freely.
- Skill learning should be proposal-and-review based, with stance applicability chosen deliberately.
- UI should preserve the feeling of one Bear while making stance-specific status understandable when needed.

## Future roles

A sixth stance should not be added merely because a new feature exists. A new stance is justified only when it has a distinct combination of:

- user or system surface,
- trust boundary,
- memory access pattern,
- external communication posture,
- runtime/tooling needs,
- and product meaning.

Until then, new capabilities should usually attach to one of the existing five stances.
