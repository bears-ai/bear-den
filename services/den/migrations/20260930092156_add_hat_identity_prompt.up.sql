-- One Bear-owned identity component per hat. Backfill existing hats from their
-- descriptive purpose; no stance prompt is copied into the hat.
ALTER TABLE bear_hats ADD COLUMN identity_prompt TEXT;
UPDATE bear_hats SET identity_prompt = purpose;
ALTER TABLE bear_hats ALTER COLUMN identity_prompt SET NOT NULL;
ALTER TABLE bear_hats ADD CONSTRAINT bear_hats_identity_prompt_length
    CHECK (length(btrim(identity_prompt)) BETWEEN 1 AND 4000);
