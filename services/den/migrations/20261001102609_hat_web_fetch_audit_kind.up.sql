ALTER TABLE bear_web_fetches
    DROP CONSTRAINT IF EXISTS bear_web_fetches_approval_kind_check;

ALTER TABLE bear_web_fetches
    ADD CONSTRAINT bear_web_fetches_approval_kind_check
    CHECK (approval_kind IN (
        'preferred', 'allowed', 'user_url', 'user_host',
        'allow_once', 'approved_once', 'hat_host',
        'denied', 'not_required', 'requires_approval'
    ));
