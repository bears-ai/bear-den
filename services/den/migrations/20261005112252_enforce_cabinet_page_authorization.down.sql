DROP FUNCTION cabinet_review_required(UUID);
DROP FUNCTION cabinet_can_review(UUID,INTEGER);
DROP FUNCTION cabinet_can_access(UUID,INTEGER,UUID,BOOLEAN);
DROP FUNCTION cabinet_ancestors(UUID);
ALTER TABLE cabinet_item_versions DROP COLUMN proposed_title;
