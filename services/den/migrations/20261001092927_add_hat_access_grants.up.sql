-- Den owns durable, positive tool and network grants for a Bear-owned hat.
-- The table is inert until every tool and egress path enforces the same resolver.
CREATE TABLE bear_hat_access_grants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL,
    hat_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('tool', 'network')),
    action_key TEXT NOT NULL CHECK (length(action_key) BETWEEN 1 AND 200),
    target_kind TEXT NOT NULL,
    target_value TEXT NOT NULL CHECK (length(target_value) <= 2000),
    created_by_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ NULL,
    FOREIGN KEY (bear_id, hat_id) REFERENCES bear_hats(bear_id, id) ON DELETE CASCADE,
    CHECK (
        (kind = 'tool' AND target_kind IN (
            'hat', 'workspace', 'directory', 'command_exact_workspace', 'command_family_workspace'
        )) OR
        (kind = 'network' AND action_key = 'https' AND target_kind = 'host')
    ),
    CHECK (
        (target_kind = 'hat' AND target_value = '') OR
        (target_kind <> 'hat' AND btrim(target_value) <> '')
    )
);

CREATE UNIQUE INDEX bear_hat_access_grants_active_unique
    ON bear_hat_access_grants (bear_id, hat_id, kind, action_key, target_kind, target_value)
    WHERE revoked_at IS NULL;
CREATE INDEX bear_hat_access_grants_current_by_hat
    ON bear_hat_access_grants (bear_id, hat_id)
    WHERE revoked_at IS NULL;
