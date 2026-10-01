-- Historical Bear-wide approvals cannot be attributed to a specific hat.
UPDATE bear_web_approvals a SET revoked_at = now()
WHERE a.revoked_at IS NULL
  AND EXISTS (SELECT 1 FROM bear_hats h WHERE h.bear_id = a.bear_id);
