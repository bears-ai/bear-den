# Capabilities and Skills

Capabilities describe what a Bear is allowed to do. Tools are the concrete actions available to agents. Skills are reusable procedures or knowledge packages that help a Bear use its tools and memory well.

## Summary

- A capability is a product-level permission or ability.
- A tool is an executable action exposed to one or more Bear agent roles.
- A skill is reusable know-how installed for selected roles.
- In BEARS, durable skills are a **special class of Bear memory artifact**.
- Den owns the canonical capability and skill configuration.
- Durable skill learning happens through Reflection proposal and review, not raw self-installation.

## Capabilities

A capability is something a Bear can do from the user's or administrator's perspective.

Examples:

- use GitHub,
- post to Slack,
- read a project repository,
- create background tasks,
- monitor a webhook,
- use company conventions while coding.

Capabilities should be described in product language. They may map to tools, skills, policies, credentials, or external integrations underneath.

## Tools

A tool is a concrete action an agent can call.

Examples:

- read or write allowed memory paths,
- write a task intent,
- approve a task intent,
- post to an integration,
- write an observation,
- relay a client-side ACP tool call.

Tools are role-scoped. A tool that is safe for `work` may be unsafe for `talk`; a tool that is appropriate for `curate` may be inappropriate for `watch`.

## Skills

A skill is reusable know-how.

Skills can encode:

- coding conventions,
- integration procedures,
- recurring workflows,
- reflection patterns,
- subscription parsing guidance,
- or team-specific practices.

In BEARS, durable skills should be modeled as a **special kind of memory artifact**: they are part of what the Bear knows how to do in a reusable way, but they are governed differently from ordinary notes or summaries because they also need role assignment, dependency declarations, review state, and runtime materialization.

That means skills are **not merely anonymous local runtime files**, but they are also **not identical to every other memory object**. They are governed memory with execution-oriented metadata.

### Canonical skill format in BEARS

The canonical durable representation of a Bear skill is a bundle under the Bear MemFS `skills/` namespace:

```text
skills/
  adr-drafting/
    SKILL.md
    bears.yaml
```

- `SKILL.md` holds the portable skill content.
- `bears.yaml` holds BEARS-specific metadata such as lifecycle, review state, role applicability, provenance, dependencies, sharing policy, and sync/materialization state.

The `skills/` namespace is flat at the skill-id level. BEARS should not encode role or lifecycle semantics in nested path hierarchies.

## Role applicability

Not every role should receive every skill or tool.

| Role | Typical capability shape |
|------|--------------------------|
| `talk` | Conversation, task-intent capture, general user help. |
| `pair` | Client-mediated collaboration and coding/design assistance. |
| `curate` | Reflection, review, memory integration, skill approval. |
| `work` | Approved outbound execution with scoped integration tools. |
| `watch` | Inbound event interpretation and observation writing. |

Role applicability keeps the Bear useful without giving every internal agent every power.

For skills, role applicability should be metadata-driven rather than path-driven. A skill may be applicable to one, many, or all roles, and that assignment belongs in BEARS metadata and Den policy.

## Skill proposals

Agents do not install durable skills directly. Skill learning belongs to the **adaptation** side of BEARS Reflection system, but durable skill governance still overlaps strongly with memory governance because skills are a special memory artifact.

The normal skill-learning flow is:

1. A role or Reflection lane identifies a reusable procedure, convention, failure mode, or checklist.
2. The role or lane submits a skill proposal through Den or a BEARS-governed review path.
3. `curate` or a future skill-review lane reviews the proposal.
4. The reviewer chooses whether to approve it, which roles it applies to, and whether its dependency metadata is adequate.
5. Den updates the Bear skill record only when policy allows.
6. Den provisions or reconciles affected runtime views.

High-risk adaptation, such as changing role prompts, tool permissions, global execution strategy, code-backed tools, or deployment/runtime configuration, should initially require human approval.

This gives the Bear a way to learn without letting any one role mutate shared capability unchecked.

## What Den owns

Den owns the canonical records for:

- which capabilities a Bear has,
- which tools each role may use,
- which skills are installed,
- which roles a skill applies to,
- which dependency and governance metadata apply to a skill,
- and whether runtime state matches the intended configuration.

Agents may request capability changes. Den enforces and installs them.

## Product language

Prefer:

- “This Bear has the GitHub capability.”
- “This skill applies to `talk` and `pair`.”
- “The Bear proposed a new skill for review.”
- “Den provisions tools and skills according to policy.”
- “This skill is stored in Bear memory and materialized into the runtime.”

Avoid:

- “Every agent can use every tool.”
- “Skills are just arbitrary local files.”
- “The Bear installed a durable skill without review.”
- “A capability is only a tool id.”

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Reflection system](REFLECTION_SYSTEM.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Dynamic skills, reflection subagents, and bear configuration ADR](../architecture/adr/dynamic-skills-subagents.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Reflection System ADR](../architecture/adr/reflection-system.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
