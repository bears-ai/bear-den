-- Review receipts are Den-owned policy evidence, not another copy of SQLite memory.
-- Each successful off → on transition for a populated hat records the exact
-- canonical SQLite snapshot the Bear admin reviewed.
CREATE TABLE bear_hat_work_reviews (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL,
    hat_id UUID NOT NULL,
    reviewed_by_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    memory_sha256 TEXT NOT NULL CHECK (memory_sha256 ~ '^[0-9a-f]{64}$'),
    record_count BIGINT NOT NULL CHECK (record_count > 0),
    rationale TEXT NOT NULL CHECK (btrim(rationale) <> ''),
    reviewed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (bear_id, hat_id) REFERENCES bear_hats (bear_id, id) ON DELETE RESTRICT
);
CREATE INDEX bear_hat_work_reviews_by_hat
    ON bear_hat_work_reviews (bear_id, hat_id, reviewed_at DESC);
