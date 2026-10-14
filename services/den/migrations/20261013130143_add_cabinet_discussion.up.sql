-- Discussion is durable page-local audit content, not a published page version.
CREATE TABLE cabinet_discussion_entries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    discussion_ref TEXT NOT NULL UNIQUE
        CHECK (discussion_ref ~ '^cabinet_discussion_[0-9a-f]{32}$'),
    item_id UUID NOT NULL REFERENCES cabinet_items(id) ON DELETE RESTRICT,
    content TEXT NOT NULL
        CHECK (char_length(content) BETWEEN 1 AND 16000 AND btrim(content) <> ''),
    authored_by JSONB NOT NULL,
    authored_by_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    authored_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT cabinet_discussion_human_author CHECK (
        jsonb_typeof(authored_by) = 'object'
        AND authored_by->>'actor_kind' IS NOT DISTINCT FROM 'user'
        AND authored_by->'user_id' IS NOT DISTINCT FROM to_jsonb(authored_by_user_id)
    )
);

CREATE INDEX cabinet_discussion_entries_page_recent
    ON cabinet_discussion_entries(item_id, authored_at DESC, discussion_ref DESC);

COMMENT ON TABLE cabinet_discussion_entries IS
    'Append-only human discussion. Page tombstones and author account deletion must not erase entries.';
COMMENT ON COLUMN cabinet_discussion_entries.authored_by IS
    'Verbatim ActorScope provenance; immutable together with content, author query key and timestamp.';

CREATE FUNCTION protect_cabinet_discussion_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Cabinet discussion entries are immutable' USING ERRCODE = '23514';
END;
$$;

CREATE TRIGGER cabinet_discussion_entries_immutable
    BEFORE UPDATE OR DELETE ON cabinet_discussion_entries
    FOR EACH ROW EXECUTE FUNCTION protect_cabinet_discussion_entry();
