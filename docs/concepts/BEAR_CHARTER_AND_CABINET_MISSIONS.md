# Bear charter and Cabinet Missions

A Bear's **charter** is its durable purpose: why this Bear exists and what responsibility boundary its memory, tools, routines, and agents serve.

A Cabinet **Mission** is different. It is a shared work and knowledge container that can involve multiple Bears and contain multiple projects.

## Summary

- A **Bear** is a persistent assistant identity with membership, policy, tools, role agents, and memory boundaries.
- A Bear has one **charter** as a characteristic of the Bear, not as a separate entity.
- Bear-specific knowledge is organized under the Bear through **Domains**, Projects, Routines, Tasks, Runs, and memory.
- **Domains** are durable areas of knowledge and responsibility within the Bear's scope.
- Cabinet **Missions** are shared work/knowledge containers and can involve zero, one, or many Bears.
- If a responsibility needs a different identity, memory boundary, policy, membership, or tool profile, create another Bear.

## Bear overview

```text
Bear
├── identity
│   ├── name
│   ├── slug
│   ├── description
│   └── charter: durable purpose / responsibility boundary
│
├── membership + policy
│   ├── users
│   ├── roles / permissions
│   ├── tool policy
│   └── privacy / trust boundary
│
├── role agents
│   ├── talk
│   ├── pair
│   ├── curate
│   ├── work
│   └── watch
│
├── Bear-scoped organization
│   ├── Domains
│   ├── Projects
│   ├── Routines
│   ├── Tasks
│   ├── Runs
│   └── Artifacts
│
├── Bear memory
│   ├── core/
│   ├── talk/
│   ├── pair/
│   ├── curate/
│   ├── work/
│   └── watch/
│
└── Cabinet links
    ├── People refs
    ├── Mission refs
    ├── Knowledge refs
    └── Cabinet page refs
```

The charter is shown as a field on the Bear because it is descriptive. The Bear itself is the scope.

## Examples

| Bear | Charter |
|------|---------|
| House Bear | Care for the house. |
| SaaS Builder Bear | Build and operate the SaaS product. |
| Executive Assistant Bear | Coordinate the executive's time, communications, and administrative flow. |
| CFO Bear | Maintain financial clarity, forecasting, and budget discipline. |
| PMO Bear | Maintain project portfolio visibility and delivery discipline. |

## Bear-scoped organization

Use these concepts for bear-specific work and knowledge:

| Concept | Meaning | Example |
|---------|---------|---------|
| Domain | Durable knowledge/responsibility area within the Bear's scope. | smart home, renovations, billing, infrastructure |
| Project | Bounded initiative with a desired outcome. | kitchen renovation, Stripe billing v1 |
| Routine | Recurring responsibility. | monthly maintenance review, weekly project digest |
| Task | Executable unit of work. | compare electrician quotes, add webhook tests |
| Run | One execution attempt. | work run 2026-05-09T10:32Z |

Distinct domains or skills do not imply distinct Bears or Cabinet Missions. For example, smart home, renovations, maintenance, appliances, and contractors can all be Domains for a House Bear whose charter is “Care for the house.”

## Bear memory overview

```text
Bear memory
├── core/
│   ├── charter.md
│   ├── domains.md
│   ├── projects.md
│   ├── current-focus.md
│   ├── decisions.md
│   ├── knowledge.md
│   └── policies.md
│
├── role-local memory
│   ├── talk/
│   ├── pair/
│   ├── curate/
│   ├── work/
│   └── watch/
│
└── Bear curated archive
    └── derived Letta Archive for semantic recall
```

`core/` is the Bear's canonical shared orientation. Letta Archives are derived semantic indexes over selected canonical content; they are not the source of truth.

For most Bears, the Bear curated archive is the semantic retrieval index for the Bear's charter and Domains.

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

## Cabinet overview

```text
Cabinet
├── People
│   ├── humans
│   ├── stakeholders
│   ├── contractors
│   └── collaborators
│
├── Missions
│   ├── shared work/knowledge containers
│   ├── may contain multiple projects
│   ├── may involve multiple Bears
│   └── may involve no Bear yet
│
└── Knowledge
    ├── procedures
    ├── policies
    ├── decisions
    ├── concepts
    ├── references
    └── runbooks
```

## Relationship between Bears and Cabinet Missions

```text
Bear 1 ─┐
        ├── Cabinet Mission A
Bear 2 ─┘

Bear 2 ─┐
        ├── Cabinet Mission B
Bear 3 ─┘

Bear 4 ──── no Cabinet Mission yet
Cabinet Mission C ──── no Bear assigned yet
```

The relationship is many-to-many. A Cabinet Mission is not the Bear's purpose; it is a shared Cabinet object that Bears can participate in.

## When to create another Bear

Create another Bear when the responsibility needs a separate:

- assistant identity,
- membership boundary,
- privacy boundary,
- tool/capability profile,
- policy profile,
- memory boundary,
- operational autonomy.

For example, an Executive Assistant Bear can collaborate with separate Comms/Scheduling, PMO, and CFO Bears instead of one Bear holding every responsibility.

## Archive implications

Cabinet Mission archives are optional. Create one when a Cabinet Mission needs semantic recall shared across Bears or role agents.

Do not create a generic “technical archive” by default. Technical knowledge usually belongs to a Bear Domain, Cabinet Mission, project, task, repo, or service.

## Product language

Prefer:

- “This Bear's charter is to care for the house.”
- “Smart home and maintenance are Domains for the House Bear.”
- “This Cabinet Mission involves the House Bear and the Finance Bear.”
- “Projects can live under a Bear or a Cabinet Mission.”
- “Bears can collaborate on a Mission.”

Avoid:

- “A Bear has many missions” as the default model.
- “A Cabinet Mission is the Bear's purpose.”
- “Different skills always mean different missions.”
- “Bear-specific work is under a Mission by default.”
- “Technical work needs its own archive by default.”
