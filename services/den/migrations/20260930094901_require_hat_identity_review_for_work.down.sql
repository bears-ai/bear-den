-- Historical Work grants cannot be reconstructed safely after a new audience
-- review. Keep them disabled on rollback; a Bear admin can re-enable explicitly.
ALTER TABLE bear_hat_work_reviews DROP COLUMN identity_sha256;
