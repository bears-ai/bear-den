-- An explicitly authored, short cross-hat directory description. Do not backfill
-- from purpose or identity_prompt: those fields were not written for this audience.
ALTER TABLE bear_hats ADD COLUMN short_summary TEXT;
ALTER TABLE bear_hats ADD CONSTRAINT bear_hats_short_summary_bound
    CHECK (short_summary IS NULL OR (
        char_length(btrim(short_summary)) BETWEEN 1 AND 160
        AND position(E'\n' IN short_summary) = 0
        AND position(E'\r' IN short_summary) = 0
    ));
