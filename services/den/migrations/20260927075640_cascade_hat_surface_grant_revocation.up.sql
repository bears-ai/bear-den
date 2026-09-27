-- Revoking a Bear's surface grant must also remove a hat's restriction on
-- that surface, rather than allowing a hat to block the revocation.
ALTER TABLE bear_hat_work_surfaces
    DROP CONSTRAINT bear_hat_work_surfaces_surface_id_bear_id_fkey,
    ADD CONSTRAINT hat_surface_bear_grant_fkey
        FOREIGN KEY (surface_id, bear_id)
        REFERENCES work_surface_bears (surface_id, bear_id) ON DELETE CASCADE;
