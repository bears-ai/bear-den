DROP TABLE cabinet_reviews;
DROP INDEX cabinet_items_parent_position;
ALTER TABLE cabinet_items DROP COLUMN reviewer_user_ids, DROP COLUMN bear_members, DROP COLUMN user_members, DROP COLUMN page_policy, DROP COLUMN position, DROP COLUMN parent_item_id;
