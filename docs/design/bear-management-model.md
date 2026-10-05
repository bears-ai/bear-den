# Bear management: ownership-led information architecture

**Status:** Selected design direction — target web UI, not a statement of implemented or deployed behavior.
**Companion:** [Bear management UI design](bear-management-ui-design.md) — user journeys, screen behavior, and acceptance checks.
**Contracts and status:** [Bear memory and hats](../topics/bear-memory-hats.md), [Docket and task execution](../topics/docket.md), [Cabinet contract](../architecture/cabinet-contract.md), [Cabinet implementation plan](../roadmap/CABINET_IMPLEMENTATION_PLAN.md), [bear package](../guides/bear-package.md).

## The organizing idea

A Bear is something a person can understand, correct, govern, and eventually take elsewhere. Its **Purpose**, **Hats**, **Memory**, and **Skills** are presented as *Yours*: Bear-owned identity and knowledge. The host's credentials, live reach, work records, shared pages, and membership are presented as *This Den*. These are **soft headings in one visible navigation list**, not modes or separate settings menus. **Overview**, **Chat**, and **Backup & move** sit outside the split: they span it.

The grouping makes portability legible before export without stamping every row “portable” or hiding normal actions. It is a design intent, not a guarantee that every Bear-owned field already exports. The [package guide](../guides/bear-package.md) describes the target portability boundary; actual export/import must be verified against its implementation. Keep the second heading about ownership (“This Den”), not infrastructure (“server”).

A person must be able to finish every management journey in Den's **web UI**. A contextual link can move between Den web pages, but no action requires an editor, CLI, raw config, or third-party admin UI. The current browser chat is a channel, not a local-tool armature.

## Navigation and destinations

```text
Den header: Bears (create / switch) · Cabinet · Connections · Reviews
Selected Bear
  Overview
  Chat
  Yours
    Purpose
    Hats
    Memory
    Skills
  This Den
    Tools
    Connections
    What it can use
    Jobs
    Cabinet
    Activity
    People
  Backup & move
```

The Den header holds **shared** destinations. The Bear's Connections link shows which reusable accounts enable this Bear and opens the same Den-wide connection management; its Cabinet link opens the **same** Den-wide page tree, with relevant authorized pages in context. Neither is a Bear-owned copy. Reviews is a permission-filtered inbox of links to owning records, not a second place to store or decide approvals. The Bear switcher and a visible breadcrumb make the current scope clear after following a cross-link. On small screens the list may collapse visually, but its grouping, labels, and destinations remain available without an “advanced” mode.

| Destination | Visible question and primary action | Inspect further |
|-------------|-------------------------------------|-----------------|
| **Overview** | Is it working; what needs me? Open a recent chat, Job, or review. | Health, current hat/use, effective reach, and recent permitted activity. |
| **Chat** | Talk to this Bear; start or resume my conversation. | Hat binding, transcript, own notes, linked Jobs and reviewable memory provenance. |
| **Purpose** | Who is this Bear? Edit its name, slug, charter, and model choices. | Effective identity/steering preview and when changes take effect. A charter is a Bear property, not an entity. |
| **Hats** | What responsibilities can it take on? Create/edit a hat, select permitted surfaces, choose an IDE default, or change Work eligibility. | Hat identity preview, allowed uses, reviewed hat knowledge and the Work-audience review; a hat restricts rather than grants reach. |
| **Memory** | What does it know, and is that right? Browse/search, correct/forget, or request reviewed promotion. | Source, scope, dates, lineage, history, and whether search is derived rather than canonical. |
| **Skills** | What owned procedures does it use? Review, attach, edit, or remove where authorized. | Source, trust, applicable uses, and any capability supplied by a skill. |
| **Tools** | What can it do? Inspect origin and effective grants; authorize or revoke through the owning control. | Built-in Den tools, armature-local tools, and remote/MCP tools; exposure and enabling connection. |
| **Connections** | Which external accounts enable it? Attach/detach access; open Den-wide setup or revocation. | Provider, scope, status, affected Bears and grants; secret *name/status*, never secret value. |
| **What it can use** | Which concrete things can it read or change? Grant/revoke a repository, document, server, design, or internal reach as permitted. | Effective limits by hat/use, enabling connection or policy, and the impact of changes. “Work surface” stays internal; do not label the UI “Resources.” |
| **Jobs** | What work is planned or running? Create/edit, prioritize, dispatch, pause/cancel or resolve when authorized. | Docket task tree, criteria, assignment, approvals, runs, evidence, outputs and settlement; optional Cabinet Mission page. |
| **Cabinet** | What shared pages can I maintain? Open the Den-wide page tree to create/edit and, when supported, organize pages and Mission subtrees. | Versions, authors, sources, attachments, page membership/policy and conditional review. |
| **Activity** | What happened? Follow permitted conversation, Job/run and Cabinet-edit trails. | Links to their canonical records, rather than a flat duplicate log or another editing surface. |
| **People** | Who can use this Bear? Manage Bear membership and roles. | Effective Bear access; Cabinet page membership stays on the page. |
| **Backup & move** | How do I take this Bear elsewhere? Preview, export/download, import and re-attach. | What travels, what stays, and what needs review or remapping on the new Den. |

## Ownership, reach, and canonical state

**Bear-owned knowledge is not the same thing as a permission.** The charter describes the Bear's durable responsibility. Hats give a bound conversation or eligible Job a named responsibility and reviewed knowledge; they may restrict already-granted surfaces but do not grant tools, credentials, egress, local armature trust, or Work authority on their own. A Work-enabled hat requires the relevant identity/knowledge audience review; an authorized Job and current policy still govern each run. Prompt/identity previews show the compiled *effect* of changes without turning hat text or memory into authority. See [Bear memory and hats](../topics/bear-memory-hats.md) for current boundaries and remaining target work.

**Two axes describe live reach:** what it can *do* (tools, with built-in/local/remote origin) and what it can *act on or read* (concrete connected things, the open web under policy, or internal Cabinet access). A Connection authenticates access to an external provider and may both enable tools and expose specific things. A reviewed Skill directs use of these capabilities; it does not silently install executable power. Show effective reach where a person grants it, including how the Bear grant, hat restriction, runtime/armature context, and governing policy combine. Warn about risky combinations of untrusted input, private data and outbound action as **review prompts**, never as a safety guarantee. Den keeps policy and named secret references; the host owns secrets and external processes. Revocation takes effect at the actual authorization boundary, not only in the display.

| Canonical owner | Record and relationship | Portability boundary |
|-----------------|-------------------------|----------------------|
| Bear identity and canonical memory | Purpose, hats/identity, reviewed hat and shared memory, source-local notes, and their provenance. | Bear-owned cognition/configuration is the *target* for package export; source privacy and review still apply on import. |
| Den capability and host wiring | Tool descriptors/grants, account Connections, host secrets, concrete resources and effective access. | Document portable **intent** separately from host bindings; re-authorize/re-attach on import. A copied grant never provides a live credential. |
| Den conversation storage | Human transcripts and client/session history, each with an owner and visibility boundary. | Not Bear cognition; not in a cognition package. |
| Docket | Jobs, tasks, runs, criteria, assignments, approvals, evidence and settlement. | Den work state; not Bear memory and not in a cognition package. Conversation task lists are projections of Docket, not another task store. |
| Cabinet | One Den-wide tree of pages and immutable versions, with page membership/policy and source links. | Shared knowledge, not per-Bear memory or part of a Bear package. Artifact payloads have their own owner; Cabinet links them. |
| Bear/Den membership | Who can administer or use a Bear, and who can read/edit a Cabinet subtree. | Host-side, re-established on import; Bear membership never substitutes for page membership. |

A Cabinet **Mission is a page** whose child pages gather shared material; it is not a Mission container or another id space. A Docket Job may name that page by `cabinet_ref`. Cabinet stores no Job/task/run identity. A page may show related Jobs only through a permission-filtered Docket lookup, not a Cabinet-maintained Job list. Page creation/editing publishes immutable versions by default; page hierarchy, inherited restrictions, optional Bear-write review and artifact attachments depend on the [Cabinet plan's](../roadmap/CABINET_IMPLEMENTATION_PLAN.md) later phases. Do not imply those controls are live before the backend enforces them.

## Inspection paths, reviews, and access

- **Conversation → Job/run → result:** a transcript links to work it initiated; a Job links to its tasks, run events, approvals, evidence and output. A conversation's task list reflects its Docket objective where one exists.
- **Conversation/source → memory → correction:** show which reviewed memories were formed, their source and effective audience. A memory can link back to a readable source; if the viewer cannot read the source, preserve the privacy boundary rather than exposing it in a label or link. Source-local notes are not automatically shared, even under the same hat.
- **Grant → effect → action:** a tool or typed card links to its enabling Connection and effective hat/use; an action links to the authorized tool, Job or Cabinet version where available. Provide a direct route to revoke/correct, not only inspect.
- **Job ↔ Mission page:** the Job owns the reference; the Cabinet page owns content, versions and access. Crossing the link checks both sides' permissions. The local implementation also offers **Save a private copy**: exact published-document evidence appears on Job detail with a scoped download, independently authorized by the artifact registry. A saved copy is not a grant to Work and survives later source-page changes/deletion; explain that distinction behind optional details. See the [verified Docket behavior](../topics/docket.md#mission-knowledge-and-saved-evidence).
- **Reviews:** the inbox combines only the actor's eligible decisions. Memory/skill proposals, hat Work-audience review, Job approvals and Cabinet pending versions keep their respective canonical owner and policy. Routine Cabinet edits publish directly unless a later page policy explicitly requires review.

Ordinary Bear members see their own conversations and permitted Jobs and shared/hat memory; admins may have broader inspection, but admin-only source-local notes, diagnostics and configuration must not leak into member summaries or search. Cabinet search and tree navigation exclude unreadable pages. Access is checked again on each detail page and action, including known IDs and cross-links; a visible link alone is not authority.

## What moves and what stays

**Yours** denotes the Bear's identity, curated knowledge and owned procedures, not an assertion that every row currently exports or that private notes become shared. **This Den** includes history, Docket and Cabinet state, host wiring, membership and secrets. Some Bear-specific grant *intent* may travel as configuration but must be remapped and authorized against destination accounts and policies. On export, explicitly list included material and exclusions; offer reviewed curation of recent knowledge rather than claiming transcripts travel. On import, validate versions and show model remapping, access re-authorization and re-attachment as actionable web steps. Derived recall is rebuildable and never the canonical export.

## Delivery and truthfulness

This document defines a **complete target**, not an implementation plan or release status. No full Cabinet tree/policy/review UI, reusable connection flow, skill catalog, or export flow should be described as working simply because it appears here. Current/target distinctions live in the [Bear memory and hats topic](../topics/bear-memory-hats.md), [Docket topic](../topics/docket.md), [Cabinet implementation plan](../roadmap/CABINET_IMPLEMENTATION_PLAN.md), and their linked contracts/plans. As a capability lands, update those maintained sources and the corresponding screen in the [UI design](bear-management-ui-design.md) together; keep missing operations visibly unavailable rather than supplying a read-only imitation of a working control.
