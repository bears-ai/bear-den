# Cabinet: shared knowledge

Cabinet is your Den's shared knowledge wiki. People and Bears read and edit the
same pages subject to current page access; every edit creates an immutable revision.
Human edits publish directly; policy can require review of Bear-written versions. There is one Cabinet per Den.

Cabinet is a **tree of pages** and nothing else — there are no separate folders
or collections. A page can hold content, child pages, or both. A "Mission" is
just a page describing a goal, with its plans and references as child pages;
a Docket job can point at that page when it needs the documentation for its
work. The web UI supports child pages, sibling order and inherited access. A Job's **Mission knowledge** section links a page and can save a private copy of its published version.

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

## Attachments and private copies

On a readable page, **Attachments** lists only files you may also read under their artifact policy. Page write authority lets you link an existing finalized `artifact_…` reference, choose its role and remove the link. Adding a link does not give other page readers access to a private file. **Download** rechecks both page and artifact permissions. JSON content is supported; finalized Garage file downloads require configured byte storage, are limited to 16 MiB, and verify size/hash without exposing storage URLs or keys. Downloads preserve uploaded filenames safely; JSON downloads use `.json`.

Open an attachment's name to **inspect the file**. Content leads; **Details** exposes its Bear owner, creator reference, audience, recorded type/size and creation/finalization times without dumping storage keys or raw provenance/metadata. JSON is pretty-printed; text, Markdown, diffs, HTML, scripts and SVG are shown only as escaped source. Text previews stop at a UTF-8 boundary within 256 KiB and clearly offer a complete-file download. PNG/JPEG/GIF/WebP and PDFs use independently authorized same-origin byte requests with MIME/signature checks, `no-store`, `nosniff`, no-referrer and a restrictive sandbox CSP. PDFs also sit in a sandboxed browser frame; if the browser cannot show them, use **Download file**. Unsupported or invalid preview formats keep their original download fallback. Missing storage/verified content and files over the 16 MiB transfer limit show unavailable states rather than active download buttons. Permission checks run before access and again after storage reads; the inspection page never grants access to a later image/PDF request.

On an active writable page, **Upload file** accepts one non-empty file up to 16 MiB without JavaScript. Choose a Bear you belong to (the artifact's canonical owner), the file's use, and optionally **Share with this Bear and its members**. Sharing is off by default: the uploaded file is private to you under current Bear membership. Sharing allows the selected Bear and its members to read the artifact; it does not override page access or automatically add file content to a Job/run. Uploads require the Den's existing `S3_ENDPOINT`, bucket, region and credential configuration; otherwise the page shows an unavailable state instead of a working-looking upload button.

Den reserves an unreadable pending artifact, sends the file through an internal signed PUT, reads it back and verifies exact size/SHA-256, then rechecks active-page write access and Bear membership before finalizing and linking in one transaction. No storage network call runs under Cabinet's database fence. Archive, page-access revocation or Bear-membership removal during transfer prevents publication. Transfer/integrity failures mark the still-pending registry row deleted and attempt blob cleanup; ambiguous database commit outcomes never trigger deletion of a finalized retained file. The recovery implementation adds a bounded automatic cleanup worker when file storage is configured. Storage writes revalidate the canonical pending lease and signed PUTs cannot outlive it; publication refuses closed/expired leases. After the 24-hour deadline plus a 20-minute write-safety grace, the worker retires unretained Cabinet-upload records under row locks, then deletes their canonical object keys outside database locks. Successful deletion is acknowledged in `artifacts.content_removed_at`; failed deletion or a crash before acknowledgement retries idempotently. Cabinet page/snapshot retention always excludes a record. This is implemented and tested in an isolated database, not yet deployed to the shared stack.

On a Job, choose an accessible Cabinet page under **Mission knowledge** and save the link. A Job owner or Bear admin can then **Save a private copy** of the published version. **Saved document evidence** offers downloads of readable copies with their captured titles. A copy stays unchanged after page edits or deletion, is private to the person who captured it under current Bear membership, and does not give the Bear or other Job readers access. Restricting the source page later does not revoke the already-captured private copy.

Linked attachments and captured copies retain their artifacts: expiration/GC and ordinary deletion cannot remove retained payloads. Detaching a page attachment releases that link's retention, but another page or snapshot link may still retain it. New uploads have a 24-hour ephemeral deadline that is ignored while retained; after the last retaining link is removed, they become GC candidates once that deadline passes. Deleting a page does not release retention. Snapshot retention has no ordinary web release control yet; purge requires operator remediation. Retention can also block deletion of the artifact's Bear through database cascades, so detach ordinary links or arrange operator remediation before deleting that Bear.

## Your uploads and recovery

**Your uploads** on the Cabinet index opens `/cabinet/uploads`. It shows your latest 64 upload records under current Bear membership, including unfinished, retained, cleanup-pending and removed states. It does not expose another member's uploads, even to a Bear admin, or reveal a source page ref/title you can no longer read. Historical visibility does not grant access to expired/deleted file bytes. **Retry cleanup** is available only for your eligible unretained records after the safety grace; the action rechecks ownership, membership and retention, and cannot force removal of a live/retained file. Without configured storage, cleanup and retry are explicitly unavailable.

The worker polls every minute in batches of ten, throttles automatic retries and preserves registry audit rows after deletion. It targets registered `cabinet_file` uploads only: other artifact types, unindexed bucket objects, objects whose registry row was removed by an operator/Bear cascade, old storage namespaces, versions/backups and Cabinet snapshot-retention release remain operator work. It deletes the current canonical key in the configured bucket; provider migration and bucket-version retention are not a secure-erasure promise.

## What Bears can do

Bears use the same knowledge store through the same facade:

| Tool | Verified contexts | What it does |
|---|---|---|
| `cabinet_search`, `cabinet_read`, `cabinet_history` | Chat, editor, browser task, eligible Work | find/read pages and inspect revisions; `cabinet_read` also reads shared text/JSON attachments |
| `cabinet_create`, `cabinet_update` | Chat, editor, browser task, with mutable governance | create an item, publish a revision |
| `cabinet_source_link` | Chat, editor, browser task, with mutable governance | attach or detach provenance (no revision published) |

A Bear's edits go through exactly the same facade, versioning, and conflict
rules as yours, and show up in history as Bear-authored with the acting
runtime context. Bears cannot delete through the facade. Archive/restore is currently a human web/facade operation, not an advertised model tool.

**Shared file reads (code implementation, not yet deployed):** a normal `cabinet_read` page response includes only attachments visible to the acting Bear, with `attachment_ref`, safe artifact summaries and `text_readable`. Attachment links are current page state, even when an old `version_ref` was requested. To read a file, pass the page's `cabinet_ref` and an `attachment_ref`; `offset_chars` defaults to 0 and `limit_chars` to 12000 (maximum 24000). The result includes the text, total characters and `next_offset_chars` for pagination. An attachment selector cannot be combined with a page version, and ranges require an attachment selector.

Only same-Bear, explicitly Bear-visible finalized files qualify; the human's private uploads and other Bears' files are not borrowed. Page policy, Cabinet enablement, link identity and artifact access are checked before/after I/O; the native caller's source authority is rechecked before delivery. Database JSON works without object storage. Configured byte storage supports declared UTF-8 text/JSON within the 16 MiB transfer ceiling and verifies full size/hash before serving a character-safe slice. Unsupported encodings, malformed UTF-8, PDFs/images/other binary formats and unavailable storage return errors rather than invented extraction. No object key, signed URL, digest or raw artifact metadata/provenance is returned. The existing provider/canonical tool names, descriptor audience and `cabinet.read` class remain unchanged; broader hat-grant resolver work remains separate. Files remain source data, not executable instructions or automatic memory/run context.

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
- Hierarchy, access/review, file upload/attach/detach and Job Mission/copy management remain web/facade operations. Attachment discovery and bounded text reading extend the existing `cabinet_read`, not a new provider tool.
- Cabinet recall, model-facing file uploads, binary document extraction and general/bucket-wide artifact GC remain pending. Upload/download/preview transfer tests use an S3 byte-store substitute, not live Garage. Browser-native image/PDF rendering has not been visually verified; signature checks are not a full decoder or malware scanner. Markdown intentionally remains source-only, avoiding embedded remote-content requests.
- The editor is a plain Markdown textarea; rendered views sanitize HTML.
