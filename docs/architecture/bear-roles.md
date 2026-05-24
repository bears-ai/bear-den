# Bear roles: talk, pair, curate, work, and watch

This document describes the five internal roles BEARS uses. It exists to align product, design, engineering, documentation, support, and marketing around one shared implementation and trust-boundary model.

A Bear should feel like one coherent assistant to a user. The preferred conceptual model is **roles, channels, and work surfaces**, not Spaces or separate provider-managed agents. Internally, BEARS uses a multi-role runtime. Each role has a distinct job, trust profile, memory branch, and relationship to external systems.

Roles are the preferred conceptual vocabulary. They are useful for code, schemas, routing, provisioning, diagnostics, architecture discussion, and user-facing explanation when a boundary matters. The Bear should still identify as the Bear rather than as a separate role runtime or sub-agent.

## Status and relationship to other docs

This is the durable conceptual source for the five Bear agent roles: what they are, why they exist, how they cooperate, and how we should talk about them.

It is not the implementation spec for provisioning, prompt hashes, tool ids, runtime lifecycle, or database reconciliation. Those details live in the Den spec. It is also not the decision record explaining why the architecture was chosen. That rationale lives in the ADR.

| Document | Audience | Purpose |
|----------|----------|---------|
| [`BEAR_AGENT_ROLES.md`](BEAR_AGENT_ROLES.md) | Product, design, engineering, docs, marketing, support | Canonical conceptual model and shared language for the five internal agent roles. |
| [`../architecture/adr/multi-agent-architecture.md`](../architecture/adr/multi-agent-architecture.md) | Engineering and architecture | Accepted decision record: why the multi-agent Bear architecture exists and what tradeoffs it makes. |
| [`../../services/den/docs/bear-spec.md`](../../services/den/docs/bear-spec.md) | Engineering implementation | Canonical Den-owned provisioning/runtime spec for prompts, tools, skills, tags, branches, and reconciliation. |

When these docs disagree, treat this document as the source for cross-functional meaning and messaging, the ADR as the source for architectural rationale, and the Den spec as the source for implementation behavior.

## The core idea

A Bear is one assistant that can operate through five coordinated roles:

| Role | Plain-English implementation name | Primary job | Common channels or invocation style |
|------|-----------------------------------|-------------|-------------------------------------|
| `talk` | Conversational agent | Talk with people in chat channels and capture task intent. | Slack, web chat, Discord, future chat surfaces. |
| `pair` | Collaborative agent | Work alongside a person inside tools such as IDEs. | ACP clients, IDEs, Cowork, Figma plugins, future client tools. |
| `curate` | Internal integrator | Decide what becomes shared memory, shared capability, approved work, or reviewed observation. | Not directly user-facing. |
| `work` | Outbound executor | Carry out approved scheduled or event-triggered work against external systems. | Not conversational; invoked by Den task dispatch. |
| `watch` | Inbound observer | Receive external events and turn them into structured observations for review. | Webhooks, polling, queues, subscriptions, streams. |

The split lets a Bear be conversational, collaborative, reflective, autonomous, and observant without giving every capability to one all-powerful runtime. The role is the operating mode and trust boundary; the channel is the concrete touchpoint; the work surface is the durable work context the Bear may be acting on.

## Why five roles?

The five-role model supports five product and safety goals at once:

1. **One coherent Bear, many contexts.** Users experience one assistant, while the system routes different contexts to the right internal role.
2. **Better concurrency.** Chat, IDE collaboration, background work, and inbound events can proceed without all traffic bottlenecking through one stateful agent.
3. **Cleaner memory.** Raw interactions stay in role-specific branches until `curate` promotes durable knowledge into shared `core/` memory.
4. **Safer autonomy.** No single role combines broad private data, outbound external communication, and unrestricted durable state mutation.
5. **Clearer product language.** Each role has a stable purpose that can guide UI, documentation, onboarding, data modeling, and marketing.

## Role summaries

### `talk`: conversational agent

`talk` is the Bear role people meet in chat. It handles synchronous conversation in Slack, web chat, Discord, and similar text-in/text-out channels.

`talk` should be understood as the Bear's conversational front door. It can answer questions, help users think through work, use appropriate channel tools, and write down task intents when a user asks for external or autonomous work.

`talk` does not directly perform arbitrary outbound autonomous work. If a user asks for something like “check this every morning,” “post this to another system,” or “monitor that service,” `talk` captures the intent in a structured form so `curate` and Den can review and route it.

**Good shorthand:** “the role that talks with you in chat.”

**Primary responsibilities:**

- Hold synchronous conversations in chat-like surfaces.
- Use the Bear's shared `core/` knowledge and its own `talk/` memory.
- Capture external-effect requests as task intents.
- Propose durable skill changes instead of installing them directly.
- Keep the user's experience coherent: the user is talking to the Bear, not to a random sub-agent.

**Intentional limits:**

- No direct autonomous outbound work.
- No access to `pair`, `curate`, `work`, or `watch` branches.
- No unilateral promotion of memories into shared `core/`.

### `pair`: collaborative agent

`pair` is the Bear role for working side-by-side with a user inside a client tool. Its most important early surface is ACP-based IDE or tool integration.

`pair` differs from `talk` because it is embedded in an active working environment. It may see project context, user-approved tool results, editor state, design documents, or other client-side resources. External effects are mediated through the client and gated by the user's approval flow.

`pair` should feel like a collaborator sitting next to the user. It can help edit, reason, debug, design, and navigate the user's working context, while preserving the boundary that client tools are user-mediated.

**Good shorthand:** “the role that pairs with you inside your tools.”

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

`curate` is deliberately not user-facing. It exists so the Bear can learn, remember, approve, reject, summarize, and integrate without giving every outward-facing role broad authority over shared memory or autonomous action.

**Good shorthand:** “the role that decides what the Bear learns and what becomes durable.”

**Primary responsibilities:**

- Read across the Bear's role branches.
- Reflect on and reorganize accumulated activity.
- Promote durable knowledge into shared `core/` memory.
- Review task intents from `talk` and `pair`.
- Review observations from `watch`, potentially generating derived task intents.
- Review and promote results from `work`.
- Review skill proposals from any role.
- Choose role applicability for approved skills.
- Update the Bear skill manifest through Den.

**Intentional limits:**

- No outbound external communication tools.
- No direct write access to other agents' branches.
- Cross-branch mutations and external effects flow through Den-controlled tools.

### `work`: outbound executor

`work` is the Bear role for approved external action. It executes scheduled, event-triggered, or otherwise approved tasks against external systems.

`work` should not be treated as another conversational agent. Its job is structured execution: call the APIs, run the research, perform the scheduled check, create the summary, or interact with an integration according to a task definition that has already passed through review and policy.

`work` sees curated context rather than raw channel history. This is central to the safety model: the role that can act outward should not be directly exposed to every prompt injection or raw private exchange that arrived through chat, IDEs, or webhooks.

**Good shorthand:** “the role that does approved outbound work.”

**Primary responsibilities:**

- Execute approved tasks dispatched by Den.
- Use only the tools and scopes allowed by the task definition and run context.
- Read shared `core/` knowledge and its own `work/` memory.
- Write task results and execution notes to `work/`.
- Propose reusable execution procedures as skill proposals.

**Intentional limits:**

- No raw access to `talk`, `pair`, `curate`, or `watch` branches.
- No self-approval of tasks.
- No use of tools outside the approved task scope.
- No direct conversational surface.

### `watch`: inbound observer

`watch` is the Bear role for inbound external events. It receives webhooks, polling results, queue messages, subscription updates, and other external signals, then writes structured observations for `curate` to review.

`watch` is the inbound counterpart to `work`. Where `work` reaches outward to do approved tasks, `watch` listens inward for relevant events. It should not take outbound action on its own. An inbound event can inform the Bear, but it must pass through observation and review before it causes external action.

**Good shorthand:** “the role that listens for external events.”

**Primary responsibilities:**

- Receive subscription and event payloads from Den.
- Parse or summarize inbound events into structured observations.
- Write observations to its own `watch/` branch.
- Use shared `core/` context to interpret events where appropriate.
- Propose reusable subscription parsing or handling procedures as skill proposals.

**Intentional limits:**

- No outbound action capability.
- No direct access to `talk`, `pair`, `curate`, or `work` branches.
- No direct promotion of observations into shared memory.
- No direct conversion of events into external action without `curate` and Den mediation.

## How the roles cooperate

The five roles form a flow from raw interaction to durable memory and approved action:

1. A person talks with `talk` or works with `pair`.
2. `talk` or `pair` answers directly when the request fits the synchronous surface.
3. If the request implies durable learning, the role writes notes or proposes a skill.
4. If the request implies external or autonomous work, the role writes a task intent.
5. `watch` may independently receive external events and write observations.
6. `curate` reviews memories, task intents, observations, skill proposals, and work results.
7. `curate` promotes durable knowledge into `core/` and uses Den-controlled tools to approve or reject cross-role changes.
8. Den dispatches approved external tasks to `work`.
9. `work` executes within its approved scope and writes results.
10. `curate` reviews those results and promotes durable learnings back into `core/`.

In short:

- `talk` and `pair` are the synchronous user-facing roles.
- `watch` is the inbound external-events role.
- `curate` is the semantic integration and review role.
- `work` is the approved outbound execution role.

## Trust model in product language

A Bear is powerful because it can remember, collaborate, observe, and act. The role split keeps those powers from concentrating in one place.

| Role | Private/raw context | External communication | Durable state |
|------|---------------------|------------------------|---------------|
| `talk` | Chat/channel context | Conversation only | Own branch |
| `pair` | Client/session context | Client-mediated and user-gated | Own branch |
| `curate` | Broad Bear context | None | Own branch and shared `core/` |
| `work` | Curated context only | Outbound approved work | Own branch |
| `watch` | Inbound payloads and curated context | Inbound only | Own branch |

This lets us say, accurately, that Bears can support autonomy while keeping raw inputs, memory integration, and external action separated by role and policy.

## Messaging guidance

### Preferred language

Use role/channel/work-surface language for ordinary explanation:

- “A Bear feels like one assistant, and Den routes different kinds of work through different roles.”
- “Each role has a clear job and a clear trust boundary.”
- “The `talk` role is where the Bear talks with people in chat-like channels.”
- “The `pair` role is where the Bear works alongside a person in a client or workspace.”
- “The `curate` role reviews what becomes shared memory.”
- “The `work` role performs approved external tasks.”
- “The `watch` role receives external events and records observations.”

Use implementation detail carefully:

- “Den projects the Bear into a runtime for the appropriate role.”
- “The `curate` role reviews and integrates durable knowledge.”
- “The `talk` and `pair` roles are the two synchronous user-facing roles.”

### Avoid

Avoid language that implies:

- A Bear is five unrelated bots.
- A Bear should introduce itself as a role agent.
- Roles are separate assistant identities.
- Every role can do every task.
- Chat surfaces directly execute arbitrary autonomous work.
- The event listener can take outbound action on its own.
- Shared memory is a dumping ground for every raw interaction.
- `curate` is merely a summarizer; it is the semantic integration and review authority.
- “Space” is the primary conceptual layer users need to understand.

### User-facing naming

The role names `talk`, `pair`, `curate`, `work`, and `watch` are the preferred stable vocabulary.

In normal user-facing behavior, a Bear should identify itself as the Bear rather than volunteering its internal role label. The internal role split is primarily an implementation and trust-boundary model, not the default self-description users should hear.

This means:

- `talk` and `pair` should normally speak in the voice of the Bear, not as “the talk role,” “the pair role,” or a separate agent.
- Internal role names should be exposed mainly in BEARS-building, operator, debugging, or other explicitly architectural contexts.
- Product surfaces may still use friendlier activity labels such as “chat,” “pairing,” or “background work,” but should avoid making the user feel like they are talking to five separate assistants.
- When a boundary explanation is necessary for honesty or safety, the system may briefly describe the relevant internal distinction without centering it as the assistant's identity.

For example:

| Role | Possible user-facing language |
|------|-------------------------------|
| `talk` | Chat, conversation, ask your Bear |
| `pair` | Collaborate in your IDE, work together, pairing |
| `curate` | Memory review, learning, integration |
| `work` | Background work, approved tasks, automations |
| `watch` | Monitoring, subscriptions, event listening |

## Design and data-model implications

The five roles should shape product and data design:

- User-facing conversation history belongs primarily to `talk` or `pair` channels, not to `work` or `watch`.
- Background tasks should be represented as reviewed work, not as hidden chat side effects.
- Subscription events should become observations before they become actions.
- Durable shared memory should be explainable as something `curate` promoted, not something every role writes freely.
- Skill learning should be proposal-and-review based, with role applicability chosen deliberately.
- UI should preserve the feeling of one Bear while making role-specific status understandable when needed.

## Future roles

A sixth role should not be added merely because a new feature exists. A new role is justified only when it has a distinct combination of:

- user or system surface,
- trust boundary,
- memory access pattern,
- external communication posture,
- runtime/tooling needs,
- and product meaning.

Until then, new capabilities should usually attach to one of the existing five roles.
