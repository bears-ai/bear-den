# Bear Charter and Cabinet Missions

A **Bear Charter** and a Cabinet **Mission** are related but distinct concepts.

## Summary

- A **Bear** is a persistent assistant identity with membership, policy, tools, role agents, and memory boundaries.
- A **Bear Charter** is the Bear's durable purpose and responsibility boundary: why this Bear exists.
- Bear-specific knowledge is organized **under the Bear's Charter**.
- **Domains** are the durable areas of knowledge and responsibility under a Bear's Charter.
- A Cabinet **Mission** is a shared work/knowledge container that may contain multiple projects and may involve multiple Bears.
- The Bear↔Mission relationship is many-to-many.
- The Charter is singular and is not an addressable data layer. Use `bear_id` for Bear-scoped records, not `charter_id`.
- If a responsibility needs a different identity, memory boundary, policy, membership, or tool profile, create another Bear rather than adding another Charter-like scope.

## Bear Charter

A Charter is the Bear's primary purpose.

Examples:

| Bear | Charter |
|------|---------|
| House Bear | Care for the house. |
| SaaS Builder Bear | Build and operate the SaaS product. |
| Executive Assistant Bear | Coordinate the executive's time, communications, and administrative flow. |
| CFO Bear | Maintain financial clarity, forecasting, and budget discipline. |
| PMO Bear | Maintain project portfolio visibility and delivery discipline. |

A Charter is singular. It belongs to the Bear and should shape the Bear's `core/` memory. Anything bear-specific should be described as living under this Charter, not under a Cabinet Mission.

Do not model Charter as a separate table, array, or foreign-key target unless a future need emerges. Bear-scoped concepts should use `bear_id`; the Charter is the meaning of that Bear scope.

Suggested `core/` files for Charter-oriented memory:

```text
core/charter.md
core/projects.md
core/current-focus.md
core/decisions.md
core/knowledge.md
core/policies.md
```

## Cabinet Mission

A Cabinet Mission is a shared work/knowledge container. It can contain:

- projects,
- decisions,
- docs,
- artifacts,
- people/stakeholders,
- links to Bears,
- links to tasks/routines,
- Cabinet pages and references.

A Cabinet Mission may map to:

- one Bear,
- multiple Bears,
- no Bear yet,
- multiple projects,
- multiple domains of work.

Examples:

| Cabinet Mission | Possible Bears involved |
|-----------------|-------------------------|
| Renovate the kitchen | House Bear, Comms/Scheduling Bear, Finance/CFO Bear |
| Launch SaaS billing | SaaS Builder Bear, CFO Bear, PMO Bear |
| Prepare board meeting | Executive Assistant Bear, CFO Bear, PMO Bear |

## Domains, projects, routines, and tasks

Inside a Bear's Charter, use more specific concepts. **Domains** are the main way to manage bear-specific knowledge under the Charter:

| Concept | Meaning | Example |
|---------|---------|---------|
| Domain | Durable knowledge/responsibility area under a Charter. | smart home, renovations, billing, infrastructure |
| Project | Bounded initiative with a desired outcome. | kitchen renovation, Stripe billing v1 |
| Routine | Recurring responsibility. | monthly maintenance review, weekly project digest |
| Task | Executable unit of work. | compare electrician quotes, add webhook tests |
| Run | One execution attempt. | work run 2026-05-09T10:32Z |

Distinct domains or skills do not imply distinct Charters or distinct Cabinet Missions. For example, smart home, renovations, maintenance, appliances, and contractors can all be Domains under a House Bear's Charter: “Care for the house.”

## When to create another Bear

Create another Bear when the responsibility needs a separate:

- assistant identity,
- membership boundary,
- privacy boundary,
- tool/capability profile,
- policy profile,
- memory boundary,
- operational autonomy.

For example, an Executive Assistant Bear may collaborate with separate Comms/Scheduling, PMO, and CFO Bears rather than becoming a single Bear with several unrelated responsibility scopes.

## Archive implications

For most Bears, the **Bear curated archive** is the semantic retrieval index for the Bear's Charter and its Domains.

Cabinet Mission archives are optional. Create one when a Cabinet Mission needs semantic recall shared across Bears or role agents.

Do not create a generic “technical archive” by default. Technical knowledge usually belongs under a Bear Charter Domain, a Cabinet Mission, a project, a task, a repo, or a service.

## Product language

Prefer:

- “This Bear's Charter is to care for the house.”
- “Smart home and maintenance are Domains under the House Bear's Charter.”
- “This Cabinet Mission involves the House Bear and the Finance Bear.”
- “Projects can live under a Bear Charter or a Cabinet Mission.”
- “Bears can collaborate on a Mission.”

Avoid:

- “A Bear has many missions” as the default model.
- “A Bear has many Charters.”
- “Use `charter_id` for Bear-scoped records.”
- “A Cabinet Mission is the Bear's purpose.”
- “Different skills always mean different missions.”
- “Bear-specific work is under a Mission by default.”
- “Technical work needs its own archive by default.”
