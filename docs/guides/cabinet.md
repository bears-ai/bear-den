# Cabinet: shared knowledge

Cabinet is your Den's shared knowledge wiki. People and Bears read and edit the
same pages subject to current page access; every edit creates an immutable revision.
Human edits publish directly; policy can require review of Bear-written versions. There is one Cabinet per Den.

Cabinet is a **tree of pages** and nothing else — there are no separate folders
or collections. A page can hold content, child pages, or both. A "Mission" is
just a page describing a goal, with its plans and references as child pages;
a Docket job can point at that page when it needs the documentation for its
work. The branch web UI now supports child pages, sibling order and inherited access; the Docket Mission reference/snapshot workflow remains target work.

Contract and design: [cabinet-contract.md](../architecture/cabinet-contract.md).
Plan and phase status: [CABINET_IMPLEMENTATION_PLAN.md](../roadmap/CABINET_IMPLEMENTATION_PLAN.md).

## Using the wiki (people)

Open **`/cabinet`** while logged in.

- **Browse and search** — the index searches titles and current content.
  Archived items are hidden by default (`Show archived items` toggles).
- **Create** — `/cabinet/new`: a title plus a Markdown body. The item is
  visible under its effective page/ancestor access. Use **New child page** on an existing page to inherit its restrictions atomically.
- **Edit** — every save publishes a new revision. If someone (or some Bear)
  published a newer revision while you were editing, your save is refused and
  the form comes back with your draft preserved and the conflict explained —
  review the latest revision, fold your change in, and save again. Nothing is
  ever merged silently and nothing is overwritten.
- **History** — every revision is immutable and permanently viewable at
  `/cabinet/{item}/history`, with author kind and timestamp.
- **Sources** — record where an item's knowledge came from (a URL, a book, an
  artifact, a conversation). Cabinet stores the link, not the linked content,
  and adding or removing one does not publish a revision.
- **Organization and access** — the page's details expose move/reorder controls, named people/Bears/reviewers, and Bear-write review policy. Ancestors narrow access; child settings cannot widen it. Access changes and moves require audience acknowledgement; cycles and depth overflow are refused. Policy administration belongs to an accessible page/ancestor owner or Den admin, not every reader or reviewer.
- **Review** — authorized humans open pending versions from Cabinet or Reviews, inspect proposed content/title, and approve/reject with a rationale. A stale published base blocks approval; no silent merge.
- **Archive / restore** — archive cascades to children only when the actor can write the subtree; restore restores only the named page. Every retained version still obeys current access.
- **Delete** — tombstones the item: it leaves Cabinet for everyone, while its
  revisions are retained so anything that already cited them keeps resolving.
  **Only people can delete.** A Bear that tries is refused by the server, so
  the most destructive thing a Bear can do to shared knowledge is archive it.
  Hard purge (removing the retained revisions) is an operator action, not a
  button here.

## What Bears can do

Bears use the same knowledge store through the same facade:

| Tool | Stances | What it does |
|---|---|---|
| `cabinet_search`, `cabinet_read`, `cabinet_history` | all | find, read (any revision), and inspect revision history |
| `cabinet_create`, `cabinet_update` | chat, pair, curate | create an item, publish a revision |
| `cabinet_source_link` | chat, pair, curate | attach or detach provenance (no revision published) |
| `cabinet_lifecycle` | curate only | archive or restore an item |

A Bear's edits go through exactly the same facade, versioning, and conflict
rules as yours, and show up in history as Bear-authored with the acting
stance. Bears cannot delete: the most destructive act available to one is a
reversible archive, and only `curate` can do even that.

Cabinet is deliberately separate from a Bear's private memory: memory tools
cannot write Cabinet, and Cabinet tools cannot write Bear memory.

## Permissions

- **People:** open-wiki access applies where no page or ancestor narrows membership. Unreadable pages are omitted from search/tree navigation, and known refs do not bypass access.
- **Bears:** gated per Bear by the `cabinet_enabled` flag on the Bear record
  (default on, like `work_enabled`). A disabled Bear does not see the Cabinet
  tools and the server independently refuses it access.
- **Scoped sharing:** named membership and policy live on pages and narrow down their subtree. A restricted page administrator must remain a member; reviewers gain review authority only while the page is readable. A Mission is an ordinary page, not a collection entity.

## Source links

An item can carry provenance links to material outside Cabinet: web URLs,
offline sources (synthetic schemes such as `book://isbn/…`), artifact refs,
or conversations. Links are provenance only — Cabinet never fetches or stores
the bytes behind them.

## Limitations

- Search is substring matching over titles and current content (no semantic
  recall yet; that is Phase 3, via the derived recall index).
- The existing model tool request schemas remain the root/search interfaces; hierarchy and access/review management are currently web/facade operations. Docket Mission annotations and immutable snapshot citations are not delivered by this slice.
- Attachments (files on pages, via artifact refs) are Phase 3.
- The editor is a plain Markdown textarea; rendered views sanitize HTML.
