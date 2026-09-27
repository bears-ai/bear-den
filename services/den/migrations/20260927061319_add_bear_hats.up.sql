-- Hats are Bear-owned, user-configured responsibility/capability limits. Existing
-- conversations and Jobs retain NULL until explicitly bound; no implicit migration
-- of profile-local memory into a hat is permitted.
CREATE TABLE bear_hats (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (btrim(name) <> ''),
    purpose TEXT NOT NULL CHECK (btrim(purpose) <> ''),
    work_enabled BOOLEAN NOT NULL DEFAULT false,
    created_by_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (bear_id, id)
);
CREATE UNIQUE INDEX bear_hats_bear_name_unique ON bear_hats (bear_id, lower(name));

-- An allowed surface must already be assigned to the Bear. This is an
-- attenuation of the existing Bear/surface grant, not a second grant.
CREATE TABLE bear_hat_work_surfaces (
    bear_id UUID NOT NULL,
    hat_id UUID NOT NULL,
    surface_id UUID NOT NULL,
    PRIMARY KEY (hat_id, surface_id),
    FOREIGN KEY (bear_id, hat_id) REFERENCES bear_hats (bear_id, id) ON DELETE CASCADE,
    FOREIGN KEY (surface_id, bear_id) REFERENCES work_surface_bears (surface_id, bear_id) ON DELETE RESTRICT
);

ALTER TABLE conversations ADD COLUMN hat_id UUID NULL;
ALTER TABLE conversations ADD CONSTRAINT conversations_bear_hat_fkey
    FOREIGN KEY (bear_id, hat_id) REFERENCES bear_hats (bear_id, id) ON DELETE RESTRICT;
CREATE INDEX conversations_hat_id_idx ON conversations (hat_id) WHERE hat_id IS NOT NULL;

ALTER TABLE bear_jobs ADD COLUMN hat_id UUID NULL;
ALTER TABLE bear_jobs ADD CONSTRAINT bear_jobs_bear_hat_fkey
    FOREIGN KEY (bear_id, hat_id) REFERENCES bear_hats (bear_id, id) ON DELETE RESTRICT;
CREATE INDEX bear_jobs_hat_id_idx ON bear_jobs (hat_id) WHERE hat_id IS NOT NULL;
