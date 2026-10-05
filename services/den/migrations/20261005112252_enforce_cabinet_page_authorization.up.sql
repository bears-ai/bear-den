ALTER TABLE cabinet_item_versions ADD COLUMN proposed_title TEXT;
CREATE FUNCTION cabinet_ancestors(target UUID) RETURNS TABLE(id UUID,parent_item_id UUID,lifecycle TEXT,page_policy JSONB,user_members INTEGER[],bear_members UUID[],reviewer_user_ids INTEGER[],created_by_user_id INTEGER,path UUID[],depth INTEGER) LANGUAGE SQL STABLE AS $$
WITH RECURSIVE ancestry AS (
 SELECT i.id,i.parent_item_id,i.lifecycle,i.page_policy,i.user_members,i.bear_members,i.reviewer_user_ids,i.created_by_user_id,ARRAY[i.id] AS path,1 AS depth FROM cabinet_items i WHERE i.id=target
 UNION ALL
 SELECT p.id,p.parent_item_id,p.lifecycle,p.page_policy,p.user_members,p.bear_members,p.reviewer_user_ids,p.created_by_user_id,a.path||p.id,a.depth+1 FROM ancestry a JOIN cabinet_items p ON p.id=a.parent_item_id WHERE NOT p.id=ANY(a.path) AND a.depth<32
) SELECT * FROM ancestry
$$;
CREATE FUNCTION cabinet_can_access(target UUID,human INTEGER,bear UUID,writing BOOLEAN) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
SELECT ((human IS NOT NULL) <> (bear IS NOT NULL)) AND
 ((human IS NOT NULL AND EXISTS(SELECT 1 FROM users WHERE id=human)) OR (bear IS NOT NULL AND EXISTS(SELECT 1 FROM bears WHERE id=bear AND cabinet_enabled))) AND
 COALESCE((SELECT bool_and(a.lifecycle <> 'deleted' AND (a.parent_item_id IS NULL OR (a.depth<32 AND NOT a.parent_item_id=ANY(a.path))) AND
 ((cardinality(a.user_members)=0 AND cardinality(a.bear_members)=0) OR (human IS NOT NULL AND human=ANY(a.user_members)) OR (bear IS NOT NULL AND bear=ANY(a.bear_members))) AND
 (NOT writing OR bear IS NULL OR COALESCE((a.page_policy->>'bears_may_write')::BOOLEAN,true))) FROM cabinet_ancestors(target) a),false)
$$;
CREATE FUNCTION cabinet_can_review(target UUID,human INTEGER) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
SELECT cabinet_can_access(target,human,NULL,false) AND (EXISTS(SELECT 1 FROM users WHERE id=human AND is_admin) OR EXISTS(SELECT 1 FROM cabinet_ancestors(target) a WHERE a.created_by_user_id=human OR human=ANY(a.reviewer_user_ids)))
$$;
CREATE FUNCTION cabinet_review_required(target UUID) RETURNS BOOLEAN LANGUAGE SQL STABLE AS $$
SELECT COALESCE((SELECT bool_or(COALESCE((page_policy->>'review_required')::BOOLEAN,false)) FROM cabinet_ancestors(target)),false)
$$;
