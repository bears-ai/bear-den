PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA busy_timeout=5000;

CREATE TABLE IF NOT EXISTS bear_sequence (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    next_sequence INTEGER NOT NULL DEFAULT 1
);
INSERT OR IGNORE INTO bear_sequence (id, next_sequence) VALUES (1, 1);

CREATE TABLE IF NOT EXISTS memory_records (
    memory_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    scope_type TEXT NOT NULL CHECK (scope_type IN ('profile_local', 'source_local', 'hat', 'shared')),
    scope_profile TEXT NULL,
    scope_source_kind TEXT NULL,
    scope_source_id TEXT NULL,
    scope_hat_id TEXT NULL,
    kind TEXT NOT NULL,
    author_profile TEXT NOT NULL,
    author_agent_id TEXT NULL,
    created_at TEXT NOT NULL,
    content_text TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    supersedes_memory_id TEXT NULL,
    visibility TEXT NOT NULL DEFAULT 'normal',
    logical_path TEXT NULL,
    work_surface_ref TEXT NULL,
    -- Bi-temporal event time (ADR-0041 / DERIVED_RECALL Phase 3.5): when the asserted fact became
    -- true or ceased to be true. created_at remains transaction time. Recall reads
    -- COALESCE(valid_from, created_at). invalid_at is forward-looking (set on supersession).
    valid_from TEXT NULL,
    invalid_at TEXT NULL,
    salience TEXT NOT NULL DEFAULT 'normal' CHECK (salience IN ('low', 'normal', 'high', 'critical')),
    CHECK (
        (scope_type = 'source_local' AND scope_source_kind IN ('conversation', 'work_run', 'intake')
         AND scope_source_id IS NOT NULL AND scope_hat_id IS NULL AND scope_profile IS NULL)
        OR (scope_type = 'hat' AND scope_hat_id IS NOT NULL AND scope_source_kind IS NULL
            AND scope_source_id IS NULL AND scope_profile IS NULL)
        OR (scope_type IN ('profile_local', 'shared') AND scope_source_kind IS NULL
            AND scope_source_id IS NULL AND scope_hat_id IS NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_memory_records_bear_sequence
    ON memory_records (bear_id, sequence_no);
CREATE INDEX IF NOT EXISTS idx_memory_records_logical_path
    ON memory_records (bear_id, logical_path);

-- Bear entity layer (ADR-0042 §2): the Bear's portable awareness/resolution of entities.
CREATE TABLE IF NOT EXISTS entities (
    entity_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    type TEXT NOT NULL,
    display_name TEXT NULL,
    resolution TEXT NOT NULL DEFAULT 'observed',
    trust TEXT NOT NULL DEFAULT 'inferred',
    canonical_ref TEXT NULL,
    superseded_by_entity_id TEXT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entities_bear_sequence ON entities (bear_id, sequence_no);
CREATE INDEX IF NOT EXISTS idx_entities_bear_type ON entities (bear_id, type);

CREATE TABLE IF NOT EXISTS entity_handles (
    handle_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    handle_type TEXT NOT NULL,
    handle_value TEXT NOT NULL,
    source TEXT NULL,
    trust TEXT NOT NULL DEFAULT 'inferred',
    state TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entity_handles_entity ON entity_handles (bear_id, entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_handles_lookup
    ON entity_handles (bear_id, handle_type, handle_value);

-- Memory–entity relation layer (ADR-0042 §7): two descriptor-routed tables, same shape.
-- Descriptive relations: filter/boost only, broad write access.
CREATE TABLE IF NOT EXISTS memory_relations (
    link_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    src_memory_id TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    relation TEXT NOT NULL,
    qualifiers_json TEXT NOT NULL DEFAULT '{}',
    author_profile TEXT NOT NULL,
    author_agent_id TEXT NULL,
    confidence TEXT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    supersedes_link_id TEXT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memory_relations_src ON memory_relations (bear_id, src_memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_relations_entity ON memory_relations (bear_id, entity_id);

-- Access-bearing relations: the ONLY table the recall gate consults (append-only audit surface).
CREATE TABLE IF NOT EXISTS memory_access_rules (
    link_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    src_memory_id TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    relation TEXT NOT NULL,
    qualifiers_json TEXT NOT NULL DEFAULT '{}',
    author_profile TEXT NOT NULL,
    author_agent_id TEXT NULL,
    confidence TEXT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    supersedes_link_id TEXT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memory_access_rules_src ON memory_access_rules (bear_id, src_memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_access_rules_entity ON memory_access_rules (bear_id, entity_id);

-- Cross-cutting read view: descriptive ∪ access-bearing, tagged with class.
CREATE VIEW IF NOT EXISTS memory_links AS
    SELECT link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
           qualifiers_json, author_profile, author_agent_id, confidence, state,
           supersedes_link_id, created_at, 'descriptive' AS class
    FROM memory_relations
    UNION ALL
    SELECT link_id, bear_id, sequence_no, src_memory_id, entity_id, relation,
           qualifiers_json, author_profile, author_agent_id, confidence, state,
           supersedes_link_id, created_at, 'access_bearing' AS class
    FROM memory_access_rules;

CREATE TABLE IF NOT EXISTS memory_promotions (
    promotion_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    source_memory_id TEXT NOT NULL,
    target_memory_id TEXT NULL,
    review_agent_id TEXT NULL,
    action TEXT NOT NULL,
    created_at TEXT NOT NULL,
    notes TEXT NULL
);

CREATE TABLE IF NOT EXISTS memory_proposals (
    proposal_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    source_memory_id TEXT NULL,
    suggested_action TEXT NOT NULL,
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    requires_human INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    reviewed_at TEXT NULL
);

CREATE TABLE IF NOT EXISTS memory_observations (
    observation_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    summary TEXT NOT NULL,
    salience TEXT NOT NULL DEFAULT 'normal',
    payload_ref TEXT NULL,
    source_json TEXT NOT NULL DEFAULT '{}',
    logical_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending_review',
    proposal_id TEXT NULL,
    created_at TEXT NOT NULL,
    reviewed_at TEXT NULL
);

CREATE TABLE IF NOT EXISTS memory_harvest_marks (
    mark_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    source_hash TEXT NULL,
    harvested_at TEXT NOT NULL,
    run_id TEXT NULL,
    proposal_ids_json TEXT NOT NULL DEFAULT '[]'
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_harvest_marks_source
    ON memory_harvest_marks (bear_id, source_kind, source_ref);

CREATE TABLE IF NOT EXISTS reflection_run_outcomes (
    run_id TEXT PRIMARY KEY,
    bear_id TEXT NOT NULL,
    sequence_no INTEGER NOT NULL,
    lane TEXT NOT NULL,
    trigger TEXT NOT NULL,
    status TEXT NOT NULL,
    input_summary TEXT NULL,
    output_summary TEXT NULL,
    proposal_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    completed_at TEXT NULL
);
