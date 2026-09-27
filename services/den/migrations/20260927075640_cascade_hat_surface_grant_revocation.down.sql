ALTER TABLE bear_hat_work_surfaces
    DROP CONSTRAINT hat_surface_bear_grant_fkey,
    ADD CONSTRAINT bear_hat_work_surfaces_surface_id_bear_id_fkey
        FOREIGN KEY (surface_id, bear_id)
        REFERENCES work_surface_bears (surface_id, bear_id) ON DELETE RESTRICT;
