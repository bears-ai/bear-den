# Cabinet: Reading & Knowledge Pipeline — Architecture Decision Record

## Status: Accepted

## Date: 2026-04-10

---

## Context

We need a self-hosted reading and bookmarking pipeline that satisfies these requirements:

1. **Reading experience**: Paginated, distraction-free reading on a Boox Android e-ink tablet, plus Safari on macOS and iOS.
2. **Highlights and annotations**: Captured during reading across multiple tools and surfaces, consolidated into a single canonical store (Karakeep), with structured export (source URL, author, content as separate fields) for downstream agent and site-builder consumption. Must also support highlights from offline sources (books, podcasts, conversations) that have no web URL.
3. **Bookmark management**: A self-hosted bookmark manager that archives full page content, supports tagging (optionally AI-assisted), and serves as the canonical content store.
4. **Semantic search**: All archived bookmark content and highlights must be indexable in Qdrant for agent-driven semantic retrieval.
5. **Bidirectional status sync**: Articles marked "read later" in the bookmark manager flow into the reading engine; articles finished in the reading engine flow back as "read" in the bookmark manager.
6. **Data ownership**: All components self-hosted. No dependency on commercial SaaS for core functionality.

---

## Decision

### Chosen Components

| Role | Component | Why |
|---|---|---|
| **Reading engine / read-it-later** | **Wallabag** (self-hosted) | Mature FOSS read-it-later app. Has web UI with annotation support, native Android and iOS apps, browser extensions, and a well-documented REST API with OAuth2. Provides RSS feeds per article status (unread, starred, archived). |
| **E-ink reader** | **KOReader** (on Boox via Android APK) | Purpose-built for e-ink. Excellent pagination, deep typography controls. Has a built-in Wallabag plugin that syncs articles as ePubs and marks them archived on completion. Exports highlights to markdown, JSON, HTML, or Kindle clippings format. |
| **Bookmark manager / content archive / highlight store** | **Karakeep** (self-hosted, Docker) | Archives full pages via Monolith (single-file HTML). Full-text search via Meilisearch. REST API for bookmarks, lists, tags, highlights, assets. Publishes per-list RSS feeds. Optional AI tagging via Ollama. Good mobile apps (iOS, Android) and browser extensions. Serves as the **canonical store for all highlights** from every source — Cabinet collectors normalize and push highlights here via the Karakeep highlights API. |
| **Semantic search** | **Qdrant** | Vector database for embedding-based retrieval across archived content and highlights. |
| **Glue layer + source metadata** | **Cabinet** (custom) | Custom services for pipeline orchestration, highlight collection from all sources, embedding generation, Qdrant indexing, and **source metadata store** (author, work title, source type — structured fields that Karakeep doesn't model). See Integration Services and Source Metadata below. |

### Components Evaluated and Rejected

| Component | Reason for rejection |
|---|---|
| **Linkwarden** | Archives as PDF (harder to extract clean text for embeddings). Less mature API. No per-collection RSS export. Better suited for team archival than automated pipelines. |
| **Karakeep as reader** | Has a reader mode, but web-only — no e-ink optimization, no pagination, no native reading app. It is a bookmark manager, not a reading engine. |
| **Linkding** | Minimal bookmark manager. No archival, no AI tagging, no reader mode. |
| **Readwise Reader** | Commercial SaaS. Poor pagination on e-ink (confirmed by user). No self-hosting. |
| **Instapaper** | Commercial, closed source. GDPR availability concerns. No self-hosting. |
| **Readeck** | Promising (lightweight Go binary, good highlighting, OPDS server), but younger project with smaller community. No native Wallabag-equivalent plugin in KOReader. Worth revisiting as it matures — may eventually replace Wallabag in this stack. |

---

## Architecture

### Data Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        INTAKE                                   │
│                                                                 │
│  Browser extension ─┐                                           │
│  iOS/Android app ───┤──► Karakeep ──► "Read Later" list         │
│  Share sheet ───────┘      │              │                      │
│                            │              │ RSS feed             │
│                            ▼              ▼                      │
│                     Monolith archive   Cabinet: RSS→Wallabag     │
│                     Meilisearch index  (cron, polls RSS feed)    │
│                            │              │                      │
│                            │              ▼                      │
│                            │         Wallabag                    │
│                            │         (reading queue)             │
└────────────────────────────┼─────────────┼──────────────────────┘
                             │             │
┌────────────────────────────┼─────────────┼──────────────────────┐
│                        READING           │                      │
│                             │            │                      │
│  Boox tablet:              │            │                      │
│    KOReader ◄──Wallabag plugin──────────┘                      │
│      │ (syncs articles as ePub,                                 │
│      │  marks archived on finish)                               │
│      │                                                          │
│      ├──► Highlights (sidecar files)                            │
│      └──► Read status → Wallabag (archived)                     │
│                                                                 │
│  Safari macOS:                                                  │
│    Wallabag web UI (annotations via web interface)              │
│                                                                 │
│  Safari iOS:                                                    │
│    Wallabag iOS app (reading + offline, limited annotations)    │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                             │
┌────────────────────────────┼────────────────────────────────────┐
│                     STATUS WRITEBACK                            │
│                                                                 │
│  Cabinet: Wallabag→Karakeep sync (cron)                        │
│    - Polls Wallabag API for recently archived entries           │
│    - Matches by URL to Karakeep bookmarks                      │
│    - Tags as "read" / moves to "Finished" list in Karakeep     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                             │
┌─────────────────────────────────────────────────────────────────┐
│              HIGHLIGHT COLLECTION → KARAKEEP                    │
│                                                                 │
│  All highlights are normalized by Cabinet collectors and        │
│  written to Karakeep via POST /highlights. Karakeep is the     │
│  single canonical highlight store.                              │
│                                                                 │
│  ┌─────────────────┐   ┌──────────────────────────────────┐    │
│  │ KOReader (Boox)  │   │ cabinet-highlight-collector       │    │
│  │ sidecar .lua     ├──►│ Parses sidecar files, extracts   │    │
│  │ files via        │   │ text + note + source URL.         │    │
│  │ Syncthing/WebDAV │   │ Finds or creates Karakeep         │    │
│  └─────────────────┘   │ bookmark, POSTs highlight.        │    │
│                         └──────────────┬───────────────────┘    │
│  ┌─────────────────┐                  │                         │
│  │ Wallabag web UI  │   ┌─────────────┴────────────────────┐    │
│  │ annotations API  ├──►│ Polls /api/annotations/{entry},  │    │
│  │ (macOS Safari)   │   │ resolves URL, pushes to Karakeep │    │
│  └─────────────────┘   └──────────────┬───────────────────┘    │
│                                        │                        │
│  ┌─────────────────┐                  │     ┌──────────────┐   │
│  │ Safari / webview │   Apple          │     │              │   │
│  │ (macOS + iOS)    ├──►Shortcut ──────┼────►│   Karakeep   │   │
│  │ via Share Sheet  │   (selected text │     │  highlights  │   │
│  └─────────────────┘    + URL → POST)  │     │   API        │   │
│                                        │     │              │   │
│  ┌─────────────────┐                  │     │  POST        │   │
│  │ Offline sources  │   Cabinet UI/    │     │  /highlights │   │
│  │ (books, pods,    ├──►API manual ────┘     │              │   │
│  │  conversations)  │   entry                └──────────────┘   │
│  └─────────────────┘                                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
                             │
┌────────────────────────────┼────────────────────────────────────┐
│                     INDEXING (Cabinet)                          │
│                                                                 │
│  Bookmark content → Qdrant:                                     │
│    - Poll Karakeep API for new/updated bookmarks                │
│    - Extract clean text from Monolith HTML archive              │
│    - Chunk text                                                 │
│    - Generate embeddings                                        │
│    - Upsert to Qdrant with metadata                             │
│                                                                 │
│  Highlights → Qdrant:                                           │
│    - Poll Karakeep highlights API (GET /highlights)             │
│    - Single source: all highlights already consolidated here    │
│    - Generate embeddings per highlight                          │
│    - Upsert to Qdrant with metadata                             │
│                                                                 │
│  Join key: source URL (bookmark URL = highlight's bookmark URL) │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Highlight Collection: Sources and Mechanisms

Karakeep is the canonical store for all highlights. Cabinet collectors are responsible for gathering highlights from each source, normalizing them, and pushing them to Karakeep's highlight API. The table below specifies each source and its capture path.

| Source | Capture mechanism | Bespoke code required? |
|---|---|---|
| **KOReader** (Boox) | HighlightSync plugin exports highlights as JSON via WebDAV → `cabinet-highlight-collector` parses JSON, resolves source URL from ePub metadata, finds/creates Karakeep bookmark, POSTs highlight. Fallback: Syncthing + `.lua` sidecar parsing. | Yes — Cabinet service |
| **Wallabag web UI** (macOS Safari) | `cabinet-highlight-collector` polls Wallabag annotation API (`/api/annotations/{entry_id}`), resolves URL, finds/creates Karakeep bookmark, POSTs highlight | Yes — Cabinet service |
| **Safari macOS** | Apple Shortcut (macOS Service): select text → right-click → Shortcut extracts selected text + page URL + title → POSTs to Cabinet highlight intake API → Cabinet ensures bookmark exists in Karakeep, attaches highlight | No bespoke extension — Apple Shortcut (~6 actions) + Cabinet API endpoint |
| **Safari iOS / app webviews** | Apple Shortcut (Share Sheet): select text → Share → "Save Highlight" → Shortcut extracts selected text + page URL + title → POSTs to Cabinet highlight intake API → Cabinet ensures bookmark exists in Karakeep, attaches highlight | No bespoke extension — same Apple Shortcut + Cabinet API endpoint |
| **Offline sources** (books, podcasts, conversations) | Manual or agent-assisted entry via Cabinet UI/API. Source identified by synthetic URL scheme (e.g. `book://isbn/9780262046305`, `offline://podcast/episode-name`). Cabinet creates a Karakeep bookmark with the synthetic URL and attaches highlights. | Cabinet UI/API feature |

### Karakeep Highlight Model: Usage Notes

Karakeep's highlight schema includes `bookmarkId`, `startOffset`, `endOffset`, `text`, `note`, `color`, `id`, `userId`, and `createdAt`. The `startOffset`/`endOffset` fields reference character positions within Karakeep's own extracted content.

For highlights originating outside Karakeep's reader (i.e. all sources listed above), the offsets are meaningless — content extraction differs between tools, so positions won't align. Cabinet collectors set dummy offset values (e.g. `startOffset: 0`, `endOffset: 0`) and rely on the `text` field to carry the actual highlighted passage. The `note` field carries the user's annotation.

**Implication**: externally-sourced highlights will not render as in-page highlights if the article is opened in Karakeep's reader. They function as attached quotes — searchable, browsable, and associated with the correct bookmark, but not visually anchored in Karakeep's content view.

### Offline Source Bookmarks

For highlights from offline sources (physical books, podcasts, conversations, etc.), Cabinet creates a Karakeep bookmark using a synthetic URL scheme as a stable identifier. Recommended conventions:

- Books: `book://isbn/<isbn13>` or `book://title/<url-encoded-title>`
- Podcasts: `podcast://<show-name>/<episode-name>`
- Conversations: `offline://conversation/<date>-<topic>`
- Other: `offline://<type>/<identifier>`

Karakeep will fail to crawl these URLs (no web content to fetch), but the bookmark's title, note, and tags can be set via API. The synthetic URL serves as the join key between the bookmark and its highlights, and between Karakeep and Qdrant.

### Apple Shortcut Specification

The Safari/webview highlight capture uses a standard Apple Shortcut, not a bespoke browser extension. The Shortcut performs approximately 6 actions:

1. **Receive** input from Share Sheet (accepts Text)
2. **Get** URLs from Shortcut Input (extracts the page URL)
3. **Get** Name of URL (extracts the page title)
4. **Set Variable** for the selected text (from input)
5. **Get Contents of URL** — POST to Cabinet's highlight intake endpoint:
   - URL: `https://<cabinet-host>/api/highlights/ingest`
   - Method: POST
   - Headers: `Authorization: Bearer <token>`, `Content-Type: application/json`
   - Body: `{"text": "<selected text>", "source_url": "<page URL>", "source_title": "<page title>"}`
6. **Show Notification** confirming save (optional)

On macOS, the same Shortcut can be invoked as a Service from the right-click context menu when text is selected in Safari. On iOS, it appears in the Share Sheet.

Cabinet's `/api/highlights/ingest` endpoint handles the Karakeep interaction:
1. Searches Karakeep for an existing bookmark matching the `source_url`.
2. If none exists, creates a new bookmark via `POST /api/v1/bookmarks`.
3. Creates the highlight via `POST /api/v1/highlights` with the `bookmarkId`, `text`, and dummy offsets.

### Source Metadata: Cabinet as the Author/Work Authority

**Problem**: Karakeep's data model does not include a first-class `author` field on bookmarks. Its crawler extracts author metadata internally via Metascraper (title, description, author, published date, publisher), but this metadata is not exposed as a writable or queryable field in the API. You cannot set or update the author for a bookmark — especially not for offline sources where no crawling occurs.

For a commonplace book, author identity and work title are essential structured fields. A highlight from Seneca's *Letters from a Stoic* must carry both "Seneca" and "Letters from a Stoic" as distinct, queryable attributes — not just text in a note.

**Solution**: Cabinet maintains its own **source metadata table**, keyed by source URL (the same join key used throughout the architecture). This table stores structured fields that Karakeep doesn't model:

```
cabinet_source_metadata
├── source_url        (PRIMARY KEY, matches Karakeep bookmark URL)
├── source_author     (string, e.g. "Seneca")
├── source_work_title (string, e.g. "Letters from a Stoic" — distinct from the article/page title)
├── source_type       (enum: "article", "book", "podcast", "conversation", "paper", etc.)
├── isbn              (string, nullable)
├── publication_date  (date, nullable)
├── karakeep_id       (string, foreign reference to Karakeep bookmark ID)
├── created_at        (timestamp)
├── updated_at        (timestamp)
```

**How it gets populated**:

- **Web articles**: Cabinet's content indexer attempts to extract author from Karakeep's crawled metadata (Metascraper does extract it internally — Cabinet can read it from the bookmark's content response if `includeContent=true`). If found, it writes it to the metadata table automatically.
- **Offline sources**: The `cabinet-highlight-intake` API accepts `source_author` and `source_work_title` as optional fields. When creating a highlight for a book or podcast, the caller provides these, and Cabinet stores them.
- **Manual enrichment**: Cabinet exposes a `PATCH /api/sources/{source_url}` endpoint for updating metadata on any source after the fact — e.g., adding an author to a web article that Metascraper didn't extract.

**Karakeep integration for browsability**: When Cabinet writes metadata for a source, it also sets an `author:<name>` tag on the corresponding Karakeep bookmark. This makes author browsing and filtering work in the Karakeep UI and mobile apps without requiring Karakeep schema changes. The tag is the human-friendly index; Cabinet's metadata table is the machine-friendly structured record.

**Downstream consumers**: The Qdrant indexers (`cabinet-content-indexer` and `cabinet-highlight-indexer`) read from Cabinet's metadata table to populate `source_author`, `source_work_title`, and `source_type` in Qdrant payloads. Agents and the site builder query Qdrant and get clean, structured attribution on every result.

### Qdrant Schema

Two collections (or one collection with `content_type` discriminator):

#### Bookmark Chunks

```json
{
  "id": "<karakeep_bookmark_id>_<chunk_index>",
  "vector": [/* embedding */],
  "payload": {
    "content_type": "bookmark_chunk",
    "source_url": "https://example.com/article",
    "source_title": "Article Title",
    "source_author": "Author Name",
    "source_work_title": "Work or Publication Title",
    "source_type": "article",
    "text": "The chunk text...",
    "tags": ["machine-learning", "transformers"],
    "date_saved": "2026-04-10T12:00:00Z",
    "date_read": "2026-04-11T08:30:00Z",
    "karakeep_id": "abc123",
    "chunk_index": 3,
    "total_chunks": 12
  }
}
```

#### Highlights

```json
{
  "id": "<karakeep_highlight_id>",
  "vector": [/* embedding */],
  "payload": {
    "content_type": "highlight",
    "source_url": "https://example.com/article",
    "source_title": "Article Title",
    "source_author": "Author Name",
    "source_work_title": "Work or Publication Title",
    "source_type": "article",
    "text": "The highlighted passage...",
    "note": "User annotation, if any",
    "color": "yellow",
    "date_highlighted": "2026-04-11T08:15:00Z",
    "karakeep_bookmark_id": "abc123",
    "karakeep_highlight_id": "def456"
  }
}
```

The `source_author`, `source_work_title`, and `source_type` fields are populated from Cabinet's source metadata table, not from Karakeep. The `source_title` comes from the Karakeep bookmark title. The `source_url` is the join key across all systems.

---

## Integration Services (Cabinet)

Cabinet provides the following custom services to glue this pipeline together:

### 1. `cabinet-rss-bridge`

**Direction**: Karakeep → Wallabag

**Mechanism**: Polls the RSS feed of Karakeep's "Read Later" list. For each new entry, POSTs the URL to Wallabag's `/api/entries` endpoint.

**Schedule**: Cron, every 10–15 minutes.

**Deduplication**: Wallabag returns HTTP 200 (not 201) if an entry already exists for that URL. Safe to re-process.

**Existing tooling to consider**: [wallabag-importer](https://github.com/the-codeboy/wallabag-importer) — a Python tool that does exactly this (RSS → Wallabag via OAuth2 API). May be usable as-is or as a starting point.

### 2. `cabinet-status-sync`

**Direction**: Wallabag → Karakeep

**Mechanism**: Polls Wallabag's API for entries with `archive=1` updated since last sync. Matches each by URL against Karakeep's bookmark API. Updates the Karakeep bookmark: adds a `read` tag and/or moves to a "Finished" list.

**Schedule**: Cron, every 15–30 minutes.

**State**: Maintains a cursor (last sync timestamp) to avoid reprocessing.

**Estimated complexity**: ~60–80 lines of Python.

### 3. `cabinet-highlight-collector`

**Direction**: KOReader + Wallabag annotations → Karakeep

**Mechanism**: Collects highlights from non-interactive sources (i.e. sources that don't post directly to Cabinet's API) and pushes them to Karakeep.

**Sub-collectors**:

**3a. KOReader highlights**:
- **Primary method**: Use the KOReader HighlightSync plugin, which syncs highlights as JSON files via WebDAV or Dropbox. JSON is straightforward to parse and avoids the complexity of raw `.lua` sidecar files.
- **Fallback method**: Sync KOReader's `docsettings/` directory from the Boox to the server via Syncthing, then parse the `.lua` sidecar files directly. Use this if HighlightSync doesn't preserve the source URL from ePub metadata.
- Extracts the source URL from the Wallabag-generated ePub metadata.
- Finds or creates the corresponding Karakeep bookmark by URL.
- POSTs each highlight to Karakeep's highlights API with dummy offsets.

**3b. Wallabag web annotations**:
- Polls Wallabag's annotation API (`GET /api/annotations/{entry_id}`) for each entry.
- Extracts highlight text and notes.
- Resolves the source URL from the Wallabag entry.
- Finds or creates the corresponding Karakeep bookmark by URL.
- POSTs each highlight to Karakeep's highlights API with dummy offsets.

**Deduplication**: The collector maintains a set of already-synced highlight hashes (hash of source_url + text) to avoid duplicates on re-runs.

**Schedule**: Cron, every 15–30 minutes.

### 4. `cabinet-highlight-intake` (API endpoint)

**Direction**: Apple Shortcuts / manual entry → Karakeep

**Mechanism**: A lightweight HTTP API endpoint exposed by Cabinet. Receives highlight submissions from Apple Shortcuts (Safari/iOS share sheet) and the Cabinet UI for offline sources.

**Endpoint**: `POST /api/highlights/ingest`

**Request body**:
```json
{
  "text": "The highlighted passage",
  "note": "Optional annotation",
  "source_url": "https://example.com/article",
  "source_title": "Article Title",
  "source_author": "Optional — author name",
  "source_work_title": "Optional — work/publication title if distinct from page title",
  "source_type": "Optional — article|book|podcast|conversation|paper",
  "color": "yellow"
}
```

**Logic**:
1. Searches Karakeep for an existing bookmark matching `source_url`.
2. If none exists, creates a new bookmark via Karakeep's `POST /api/v1/bookmarks` with `type: "link"` and the given URL. (For synthetic URLs like `book://isbn/...`, Karakeep will fail to crawl but the bookmark record is created with the title from the request.)
3. Creates the highlight via Karakeep's `POST /api/v1/highlights` with `bookmarkId`, `text`, `note`, `color`, and dummy offsets.
4. If `source_author` or `source_work_title` is provided, upserts a record in Cabinet's source metadata table keyed by `source_url`. Also sets/updates an `author:<name>` tag on the Karakeep bookmark for browsability.
5. Returns success with the Karakeep highlight ID.

**Estimated complexity**: ~40–60 lines (a thin HTTP handler over Karakeep's API).

### 5. `cabinet-content-indexer`

**Direction**: Karakeep → Qdrant

**Mechanism**: Polls Karakeep API for new or updated bookmarks. For each:

1. Fetches the archived content (Monolith HTML or extracted text via Meilisearch).
2. Parses clean text, extracts metadata (title, URL) from the Karakeep bookmark.
3. Attempts to extract author from Karakeep's crawled content (Metascraper metadata, available when `includeContent=true`). If found and not already in Cabinet's metadata table, writes it there and sets an `author:<n>` tag on the Karakeep bookmark.
4. Looks up Cabinet's source metadata table for `source_author`, `source_work_title`, and `source_type`.
5. Chunks the text (strategy TBD — fixed-size with overlap, or semantic chunking).
6. Generates embeddings (model TBD — OpenAI `text-embedding-3-small`, or local via Ollama).
7. Upserts vectors + payload to Qdrant, including source metadata fields.

**Schedule**: Event-driven (webhook from Karakeep if available) or cron.

**Deletion**: When a bookmark is deleted from Karakeep, corresponding vectors should be removed from Qdrant by `karakeep_bookmark_id`.

### 6. `cabinet-highlight-indexer`

**Direction**: Karakeep highlights → Qdrant

**Mechanism**: Polls Karakeep's `GET /highlights` API for new highlights. For each:

1. Fetches highlight text and metadata.
2. Resolves the parent bookmark to get source URL, title, and tags.
3. Looks up Cabinet's source metadata table by source URL to get `source_author`, `source_work_title`, and `source_type`.
4. Generates embedding for the highlight text.
5. Upserts to Qdrant with `content_type: "highlight"`, including source metadata fields.

Because all highlights are already consolidated in Karakeep, this indexer has a single data source — it does not need to know about KOReader, Wallabag, Apple Shortcuts, or offline entry. It just reads from Karakeep.

**Schedule**: Cron, every 15–30 minutes (or event-driven if Karakeep supports webhooks).

---

## Known Limitations and Trade-offs

### Author metadata gap in Karakeep

Karakeep's data model does not expose author as a writable or queryable field on bookmarks. Its crawler extracts author via Metascraper internally, but this is not available through the bookmark update API. Cabinet compensates by maintaining its own source metadata table with structured author, work title, and source type fields. The `author:<n>` tag convention on Karakeep bookmarks provides browsability in the Karakeep UI, while Cabinet's metadata table provides structured queries for agents and the site builder. For web articles, Cabinet attempts to auto-extract author from Karakeep's crawled content; for offline sources, author must be provided explicitly at highlight creation time.

### Highlight offset mismatch

Karakeep's highlight model uses character offsets within its own extracted content. Highlights from external sources (KOReader, Wallabag, Safari, offline) cannot provide meaningful offsets. These highlights are stored with dummy offsets and will not render as in-page highlights in Karakeep's reader. They function as searchable, browsable attached quotes.

### Wallabag iOS annotation limitations

Wallabag's annotation feature does not work reliably in mobile Safari or the iOS app. For iOS highlighting, use the Apple Shortcut (Share Sheet) path, which bypasses Wallabag entirely and goes directly to Cabinet → Karakeep.

### Dual content extraction

Both Karakeep and Wallabag extract article content independently when a URL is saved. Karakeep's Monolith archive is the canonical source for Qdrant indexing. Wallabag's extraction is used only for the reading experience.

### KOReader Wallabag plugin scope

The plugin syncs read-status (marks articles as archived in Wallabag on completion). It does NOT sync highlights back to Wallabag. Highlights remain in KOReader's local sidecar files until collected by `cabinet-highlight-collector`.

### Highlight deduplication across sources

It is possible for the same passage to be highlighted in both KOReader and the Wallabag web UI. The `cabinet-highlight-collector` deduplicates by hashing `source_url + text`, so identical highlights from different sources produce only one Karakeep entry. Near-duplicates (e.g. slightly different selection boundaries for the same passage) are not deduplicated and will appear as separate highlights.

### URL canonicalization

The entire architecture uses source URL as the join key between bookmarks and highlights, and between Karakeep and Qdrant. URL normalization (trailing slashes, query parameters, protocol differences, URL shorteners) must be handled consistently. Cabinet should normalize URLs on ingestion — strip tracking parameters, normalize protocol to https, remove trailing slashes, and resolve redirects — to prevent the same article from appearing as multiple bookmarks.

---

## API Reference Summary

| Service | Auth | Key Endpoints |
|---|---|---|
| **Karakeep** | Bearer token (API key from Settings) | `GET/POST /api/v1/bookmarks`, `GET /api/v1/lists`, `PATCH /api/v1/bookmarks/:id`, `GET/POST /api/v1/highlights`, `GET /api/v1/highlights/:id`, RSS at `/api/v1/lists/:id/rss` |
| **Wallabag** | OAuth2 (client credentials) | `GET/POST /api/entries`, `GET /api/annotations/{entry}`, `PATCH /api/entries/{id}` (set `archive=1`) |
| **Qdrant** | API key or none (local) | Standard Qdrant REST/gRPC — `PUT /collections`, `PUT /points`, `POST /points/search` |
| **KOReader** | Filesystem (HighlightSync JSON via WebDAV, or sidecar `.lua` files) | No HTTP API — read highlight data from HighlightSync JSON exports (preferred) or from synced `docsettings/` directory (`.lua` tables, parseable with a Lua or regex-based parser). |
| **Cabinet** | Bearer token | `POST /api/highlights/ingest` — highlight intake from Apple Shortcuts and manual entry; `PATCH /api/sources/{source_url}` — update source metadata (author, work title, source type); `GET /api/sources/{source_url}` — read source metadata |

---

## Open Questions

### Resolved

3. **KOReader highlight collection method**: **Use the HighlightSync plugin** (JSON via WebDAV). This produces structured JSON files that are straightforward to parse, avoids the complexity of raw `.lua` sidecar files, and supports sync across devices via cloud storage. Fall back to Syncthing + `.lua` parsing if HighlightSync doesn't preserve the source URL from ePub metadata.

4. **Karakeep webhook support**: **No webhooks available.** Karakeep's architecture uses tRPC internally with background workers for async tasks, but does not expose outbound webhooks. All Cabinet indexers and collectors should use **polling with cursors** (last-sync timestamp or Karakeep's `cursor` pagination parameter).

### Requires Smoke Test (first hour of Karakeep deployment)

5. **Karakeep dummy offset validation**: The OpenAPI schema defines `startOffset` and `endOffset` as required `number` fields with no minimum constraint, so `0, 0` should pass schema validation. The `text` field is a nullable string — unclear whether Karakeep derives it from offsets or accepts it as provided. The community `karakeep_python_api` library includes an [Omnivore highlight importer](https://github.com/thiswillbeyourgithub/karakeep_python_api) with "intelligent position detection and bookmark matching," confirming that importing external highlights is a known use case. **Test**: POST a highlight with `startOffset: 0, endOffset: 0, text: "test passage"` to a bookmark and verify the text is stored correctly and appears in `GET /highlights`. Reference the Omnivore importer for the exact offset values it uses.

6. **Karakeep bookmark creation for synthetic URLs**: The bookmark creation API accepts `type: "link"` with a URL (typed as a string, no scheme validation visible in the spec). Karakeep will attempt to crawl the URL and fail silently for non-HTTP schemes. **Test**: POST a bookmark with `type: "link", url: "book://isbn/9780262046305"` and verify the record is created (even with a failed crawl). If Karakeep rejects non-HTTP URLs, the fallback is to use `type: "text"` bookmarks with the synthetic URL in the note field — but this means the join key becomes the Karakeep bookmark ID rather than the URL.

### Deferred (decide after real usage data)

1. **Chunking strategy** for bookmark content. **Default**: start with fixed-size chunks of ~500 tokens with 50-token overlap. This is simple, predictable, and works well for the typical article lengths in a read-later queue. Revisit with semantic chunking (by heading/paragraph) if agent query quality is poor.

2. **Embedding model**. **Default**: start with OpenAI `text-embedding-3-small` for quality and simplicity. Evaluate local alternatives (`nomic-embed-text` via Ollama, or `bge-small-en-v1.5`) once volume and latency requirements are clearer. The Qdrant schema is model-agnostic — switching models only requires re-indexing.
