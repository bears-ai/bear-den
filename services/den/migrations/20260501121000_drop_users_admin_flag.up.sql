-- Align users table on the Phase 1 operator flag name.
--
-- `admin_flag` came from the original Trestle scaffold. `is_admin` is the
-- canonical Den operator flag used by the app and docs.

UPDATE users
SET is_admin = COALESCE(is_admin, false) OR COALESCE(admin_flag, false)
WHERE is_admin IS DISTINCT FROM (COALESCE(is_admin, false) OR COALESCE(admin_flag, false));

ALTER TABLE users
    ALTER COLUMN is_admin SET DEFAULT false,
    ALTER COLUMN is_admin SET NOT NULL;

ALTER TABLE users
    DROP COLUMN IF EXISTS admin_flag;
