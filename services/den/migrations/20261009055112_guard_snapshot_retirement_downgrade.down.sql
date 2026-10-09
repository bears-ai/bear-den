-- Refuse before earlier down scripts can weaken guards around surviving audit.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM artifact_links WHERE retention_released_at IS NOT NULL) THEN
        RAISE EXCEPTION 'snapshot retirement receipts exist; downgrade across snapshot hardening requires an explicit audit-preserving migration';
    END IF;
END $$;
