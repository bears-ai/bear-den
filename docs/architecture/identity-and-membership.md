# Identity and Membership

Identity defines who a user is and which Bear they are interacting with. Membership defines which users may access or administer a Bear.

## Summary

- Users and Bears are separate identities.
- A user may belong to many Bears.
- A Bear may have many users.
- Den enforces membership before routing requests to Bear agent roles.
- Human membership roles are different from internal Bear agent roles.

## Users

A user is a human or authorized account that can interact with Den and, through Den, with Bears.

Users may access Bears through:

- web chat,
- Slack or other chat surfaces,
- ACP clients and IDEs,
- admin UI,
- or future API clients.

The same user identity may appear across multiple surfaces, but Den is responsible for deciding whether that user may access a given Bear.

## Bears

A Bear is the assistant identity.

A Bear may be:

- personal to one user,
- shared by a team,
- scoped to a project,
- or administered by operators.

The exact product packaging may change, but the access model should remain clear: users do not automatically have access to every Bear.

## Membership

Membership is the relationship between a user and a Bear.

Membership answers:

- Can this user see the Bear?
- Can this user talk to the Bear?
- Can this user pair with the Bear in a client tool?
- Can this user administer the Bear?
- Can this user change capabilities, skills, integrations, or membership?

Den checks membership before allowing requests to proceed to the Bear's internal agents or control-plane operations.

## Membership roles

Membership roles are human access roles. They are not the same as Bear agent roles.

Possible membership roles may include:

| Membership role | Meaning |
|-----------------|---------|
| Owner | Full authority over the Bear, including membership and destructive administration. |
| Admin | Can manage configuration, capabilities, and operational settings. |
| Member | Can interact with the Bear through approved surfaces. |
| Read-only | Can inspect allowed Bear state but not make changes or initiate work. |
| Operator | Site-level or deployment-level authority, not necessarily a normal Bear member. |

The exact role set can evolve. The important distinction is that membership roles describe human permissions; Bear agent roles describe internal agent responsibilities.

## Personal and shared Bears

A personal Bear is primarily associated with one user. A shared Bear is associated with multiple users or a team.

Shared Bears need especially clear rules for:

- who can see conversations,
- who can approve work,
- who can install capabilities,
- who can view memory,
- who can manage subscriptions,
- and who can remove members.

Those rules should be enforced by Den rather than inferred by agents.

## Trusted context

Agents should receive trusted user and membership context from Den.

Clients and model messages must not be trusted to declare:

- user id,
- Bear id,
- membership role,
- approval status,
- or authorization scope.

Den constructs trusted context after authentication and membership checks, then passes only the needed scope downstream.

## Conversation, thread, and session

Bear Den distinguishes among **conversations**, **threads**, and **sessions**.

- A **conversation** is the durable, user-visible exchange between a human and a Bear. It is the default cross-surface term for chat history, titles, summaries, and contextual continuity.
- A **thread** is a channel-native reply structure, such as a Slack thread. Use this term only when the underlying surface explicitly has thread semantics.
- A **session** is a runtime interaction context, such as an ACP session, browser connection, or authenticated client binding. Sessions are operational and may be temporary; they can attach to existing conversations.

Prefer **conversation** over **thread** or **session** unless channel structure or runtime binding is specifically what matters.

### Guidance

Use **conversation** when the focus is:

- continuity over time,
- user-visible exchange history,
- titles or summaries,
- memory or task provenance,
- or the durable interaction record a Bear is participating in.

Use **thread** when the focus is:

- a channel-native reply structure,
- a Slack or Discord thread,
- reply nesting,
- or a transport/UI anchor specific to one messaging surface.

Use **session** when the focus is:

- a live runtime binding,
- client authentication or connection context,
- ACP or browser attachment state,
- tool scope,
- workspace binding,
- or execution lifecycle.

### Relationship

A helpful default model is:

- a **session** participates in or accesses a **conversation**,
- a **thread** may host or represent a **conversation**,
- a **conversation** may span multiple **sessions**,
- and a **conversation** may exist without any thread concept at all.

### Product language

Prefer:

- “The ACP session is attached to conversation `conv_123`.”
- “This Slack thread maps to a Bear conversation.”
- “Conversation summaries are durable; session metadata is ephemeral.”
- “A user may return to the same conversation in a new session.”

Avoid:

- “session” when you mean durable chat history,
- “thread” as the universal term for all Bear interactions,
- or “conversation” when you specifically mean a runtime binding or channel-native reply structure.

## Product language

Prefer:

- “This user is a member of that Bear.”
- “Den checks membership before routing the request.”
- “Membership roles control human access.”
- “Bear agent roles control internal responsibilities.”

Avoid:

- “A Bear role” when you mean a human membership role.
- “The agent decided the user was authorized.”
- “Anyone with a link can administer a Bear.”
- “User identity is supplied by the prompt.”

## Related docs

- [Bear Den and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Den architecture](../architecture/DEN_ARCHITECTURE.md)
- [Bear channel and ACP](../architecture/BEAR_CHANNEL_AND_ACP.md)
