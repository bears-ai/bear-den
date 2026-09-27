DROP INDEX bear_jobs_hat_id_idx;
ALTER TABLE bear_jobs DROP CONSTRAINT bear_jobs_bear_hat_fkey;
ALTER TABLE bear_jobs DROP COLUMN hat_id;

DROP INDEX conversations_hat_id_idx;
ALTER TABLE conversations DROP CONSTRAINT conversations_bear_hat_fkey;
ALTER TABLE conversations DROP COLUMN hat_id;

DROP TABLE bear_hat_work_surfaces;
DROP TABLE bear_hats;
