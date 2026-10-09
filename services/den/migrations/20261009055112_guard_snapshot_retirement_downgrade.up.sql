-- Rollback barrier only: the preceding migration already installed the guards.
-- SQLx commits each down version independently. Register a later version so a
-- populated-receipt rollback refuses BEFORE removing the snapshot hardening.
-- No schema, artifact, citation, payload or receipt data is changed on apply.
SELECT 1;
