use serde_json::{json, Value};
use uuid::Uuid;

use den_core::{ids::HatId, DenError};

use super::{
    clock::now_rfc3339,
    records::{head_record_for_logical_path, BearMemoryStore},
};

#[cfg(test)]
#[path = "proposals/verified_hat_tests.rs"]
mod verified_hat_tests;

#[derive(Debug, sqlx::FromRow)]
struct ProposalSqlRow {
    proposal_id: String,
    sequence_no: i64,
    status: String,
    payload_json: String,
    created_at: String,
    source_memory_id: Option<String>,
    target_hat_id: Option<String>,
}

impl ProposalSqlRow {
    fn into_proposal(self) -> Result<SqliteMemoryProposal, DenError> {
        let verified_hat_source = match (self.source_memory_id, self.target_hat_id) {
            (None, None) | (Some(_), None) => None, // Older proposals may have a source but no verified hat.
            (Some(memory_id), Some(hat_id)) => Some(VerifiedHatProposalSource {
                memory_id: Uuid::parse_str(&memory_id)
                    .map_err(|_| DenError::System("invalid verified proposal source ID".into()))?,
                hat_id: hat_id
                    .parse::<HatId>()
                    .map_err(|_| DenError::System("invalid verified proposal hat ID".into()))?,
            }),
            (None, Some(_)) => {
                return Err(DenError::System(
                    "verified proposal hat has no source".into(),
                ))
            }
        };
        Ok(SqliteMemoryProposal {
            proposal_id: self.proposal_id,
            sequence_no: self.sequence_no,
            status: self.status,
            payload_json: serde_json::from_str(&self.payload_json).unwrap_or_else(|_| json!({})),
            created_at: self.created_at,
            verified_hat_source,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SqliteMemoryProposal {
    pub proposal_id: String,
    pub sequence_no: i64,
    pub status: String,
    pub payload_json: Value,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_hat_source: Option<VerifiedHatProposalSource>,
}

/// Only constructed after Den checks the current source record and canonical
/// conversation/Job binding. These SQLite columns, not proposal JSON or paths,
/// are the basis for any future publication decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerifiedHatProposalSource {
    pub memory_id: Uuid,
    pub hat_id: HatId,
}

pub async fn create_memory_proposal(
    store: &BearMemoryStore,
    suggested_action: &str,
    sensitivity: &str,
    requires_human: bool,
    payload: &Value,
) -> Result<SqliteMemoryProposal, DenError> {
    create_proposal_with_verified_source(
        store,
        suggested_action,
        sensitivity,
        requires_human,
        payload,
        None,
    )
    .await
}

pub async fn create_verified_hat_proposal(
    store: &BearMemoryStore,
    sensitivity: &str,
    requires_human: bool,
    payload: &Value,
    verified: VerifiedHatProposalSource,
) -> Result<SqliteMemoryProposal, DenError> {
    // Recheck canonical source columns at write time; a guessed UUID and a
    // model-supplied path alone never establish a verified proposal link.
    crate::hat_promotion::review_candidate(store, verified.memory_id).await?;
    create_proposal_with_verified_source(
        store,
        "propose_hat",
        sensitivity,
        requires_human,
        payload,
        Some(verified),
    )
    .await
}

async fn create_proposal_with_verified_source(
    store: &BearMemoryStore,
    suggested_action: &str,
    sensitivity: &str,
    requires_human: bool,
    payload: &Value,
    verified: Option<VerifiedHatProposalSource>,
) -> Result<SqliteMemoryProposal, DenError> {
    // A verified hat intake does not claim a target head yet. Legacy path-based
    // freshness and consolidation heuristics must not decide its audience.
    let enriched = if verified.is_some() {
        payload.clone()
    } else {
        let payload = enrich_payload_with_freshness(store, payload).await?;
        enrich_payload_with_consolidation_review(store, &payload).await?
    };
    let enriched = enrich_payload_with_archive_lane(&enriched, suggested_action);
    let enriched = enrich_payload_with_sensitivity_gate(&enriched, sensitivity);
    let payload = payload_with_dedupe_key(&enriched, suggested_action, sensitivity);
    let effective_requires_human = requires_human
        || payload
            .get("requires_human")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if verified.is_none() {
        if let Some(key) = payload
            .pointer("/dedupe/key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        {
            if let Some(existing) = pending_proposal_for_dedupe_key(store, key).await? {
                return Ok(existing);
            }
        }
    }

    let proposal_id = Uuid::new_v4().to_string();
    let sequence_no = store.next_sequence().await?;
    let created_at = now_rfc3339()?;
    sqlx::query(
        r"
        INSERT INTO memory_proposals (
            proposal_id, bear_id, sequence_no, source_memory_id, target_hat_id,
            suggested_action, sensitivity, requires_human, status, payload_json, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)
        ",
    )
    .bind(&proposal_id)
    .bind(store.bear_id().to_string())
    .bind(sequence_no)
    .bind(verified.map(|source| source.memory_id.to_string()))
    .bind(verified.map(|source| source.hat_id.to_string()))
    .bind(suggested_action)
    .bind(sensitivity)
    .bind(i32::from(effective_requires_human))
    .bind(payload.to_string())
    .bind(&created_at)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite create proposal failed: {e}")))?;
    Ok(SqliteMemoryProposal {
        proposal_id,
        sequence_no,
        status: "pending".to_string(),
        payload_json: payload,
        created_at,
        verified_hat_source: verified,
    })
}

fn enrich_payload_with_archive_lane(payload: &Value, suggested_action: &str) -> Value {
    if suggested_action != "archive_index" {
        return payload.clone();
    }

    let mut payload = ensure_object_payload(payload);
    let obj = payload.as_object_mut().expect("payload object initialized");
    obj.insert("requires_human".to_string(), json!(true));
    obj.insert(
        "archive_lane".to_string(),
        json!({
            "status": "review_required",
            "apply_mode": "proposal_only",
            // ponytail: archive harvest is represented as archive_index proposals only;
            // upgrade path is a dedicated archive-harvest runner with richer ambiguity scoring.
            "detector": "archive-index-proposal-v1"
        }),
    );
    payload
}

fn enrich_payload_with_sensitivity_gate(payload: &Value, sensitivity: &str) -> Value {
    if !matches!(
        sensitivity.trim(),
        "person" | "secret_risk" | "external_untrusted" | "unknown"
    ) {
        return payload.clone();
    }

    let mut payload = ensure_object_payload(payload);
    let obj = payload.as_object_mut().expect("payload object initialized");
    obj.insert("requires_human".to_string(), json!(true));
    obj.insert(
        "risk_review".to_string(),
        json!({
            "status": "review_required",
            "sensitivity": sensitivity.trim(),
            // ponytail: sensitivity routing is enum-based only; upgrade path is content scanning/classification.
            "detector": "sensitivity-enum-v1"
        }),
    );
    payload
}

async fn enrich_payload_with_freshness(
    store: &BearMemoryStore,
    payload: &Value,
) -> Result<Value, DenError> {
    let Some(target_ref) = payload
        .get("target_ref")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(payload.clone());
    };
    let Some(new_fingerprint) = payload_claim_fingerprint(payload) else {
        return Ok(payload.clone());
    };
    let Some(head) = head_record_for_logical_path(store, target_ref).await? else {
        return Ok(payload.clone());
    };
    let head_fingerprint = super::claim_fingerprint::memory_claim_fingerprint(&head.content_text);
    if head_fingerprint.is_empty() || head_fingerprint == new_fingerprint {
        return Ok(payload.clone());
    }

    let mut payload = ensure_object_payload(payload);
    let obj = payload.as_object_mut().expect("payload object initialized");
    obj.insert("requires_human".to_string(), json!(true));
    obj.insert(
        "freshness".to_string(),
        json!({
            "status": "potential_conflict",
            "review_required": true,
            "target_ref": target_ref,
            "current_memory_id": head.memory_id,
            "current_sequence_no": head.sequence_no,
            "current_claim_fingerprint": head_fingerprint,
            "proposed_claim_fingerprint": new_fingerprint,
            // ponytail: contradiction detection is deterministic inequality at the same target_ref;
            // upgrade path is semantic entailment/contradiction review proposals.
            "detector": "same-path-different-claim-v1"
        }),
    );
    Ok(payload)
}

async fn enrich_payload_with_consolidation_review(
    store: &BearMemoryStore,
    payload: &Value,
) -> Result<Value, DenError> {
    if payload.get("consolidation_review").is_some() {
        return Ok(payload.clone());
    }
    let Some(new_fingerprint) = payload_claim_fingerprint(payload) else {
        return Ok(payload.clone());
    };
    let target_ref = payload
        .get("target_ref")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let Some(candidate) =
        active_record_with_claim_fingerprint(store, &new_fingerprint, target_ref).await?
    else {
        return Ok(payload.clone());
    };

    let mut payload = ensure_object_payload(payload);
    let obj = payload.as_object_mut().expect("payload object initialized");
    obj.insert("requires_human".to_string(), json!(true));
    obj.insert(
        "consolidation_review".to_string(),
        json!({
            "status": "needs_human_review",
            "reason": "same_claim_different_path",
            "candidate_memory_id": candidate.memory_id,
            "candidate_logical_path": candidate.logical_path,
            "candidate_sequence_no": candidate.sequence_no,
            // ponytail: consolidation is exact normalized-claim matching over recent active records only;
            // upgrade path is model-assisted semantic duplicate/supersession proposals.
            "detector": "deterministic-claim-fingerprint-v1"
        }),
    );
    Ok(payload)
}

struct ConsolidationCandidate {
    memory_id: String,
    logical_path: Option<String>,
    sequence_no: i64,
}

#[derive(sqlx::FromRow)]
struct ConsolidationCandidateSqlRow {
    memory_id: String,
    sequence_no: i64,
    logical_path: Option<String>,
    content_text: String,
}

async fn active_record_with_claim_fingerprint(
    store: &BearMemoryStore,
    fingerprint: &str,
    target_ref: &str,
) -> Result<Option<ConsolidationCandidate>, DenError> {
    let rows = sqlx::query_as::<_, ConsolidationCandidateSqlRow>(
        r"
        SELECT memory_id, sequence_no, logical_path, content_text
        FROM memory_records
        WHERE bear_id = ?
          AND visibility = 'normal'
          AND invalid_at IS NULL
          AND COALESCE(json_extract(metadata_json, '$.lifecycle.status'), 'active') NOT IN ('archived', 'archive-candidate')
        ORDER BY sequence_no DESC
        LIMIT 100
        ",
    )
    .bind(store.bear_id().to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite consolidation lookup failed: {e}")))?;

    Ok(rows.into_iter().find_map(|row| {
        if row.logical_path.as_deref() == Some(target_ref) {
            return None;
        }
        (super::claim_fingerprint::memory_claim_fingerprint(&row.content_text) == fingerprint)
            .then_some(ConsolidationCandidate {
                memory_id: row.memory_id,
                logical_path: row.logical_path,
                sequence_no: row.sequence_no,
            })
    }))
}

fn ensure_object_payload(payload: &Value) -> Value {
    if payload.is_object() {
        payload.clone()
    } else {
        json!({ "value": payload })
    }
}

fn payload_claim_fingerprint(payload: &Value) -> Option<String> {
    payload
        .get("proposed_content")
        .and_then(Value::as_str)
        .or_else(|| payload.get("summary").and_then(Value::as_str))
        .map(super::claim_fingerprint::memory_claim_fingerprint)
        .filter(|value| !value.is_empty())
}

async fn pending_proposal_for_dedupe_key(
    store: &BearMemoryStore,
    dedupe_key: &str,
) -> Result<Option<SqliteMemoryProposal>, DenError> {
    let row = sqlx::query_as::<_, ProposalSqlRow>(
        r"
        SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
        FROM memory_proposals
        WHERE bear_id = ?
          AND status = 'pending'
          AND json_extract(payload_json, '$.dedupe.key') = ?
        ORDER BY sequence_no DESC
        LIMIT 1
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(dedupe_key)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite find duplicate proposal failed: {e}")))?;
    row.map(ProposalSqlRow::into_proposal).transpose()
}

fn payload_with_dedupe_key(payload: &Value, suggested_action: &str, sensitivity: &str) -> Value {
    let mut payload = payload.clone();
    let Some(key) = proposal_dedupe_key(&payload, suggested_action, sensitivity) else {
        return payload;
    };
    if !payload.is_object() {
        payload = json!({ "value": payload });
    }
    let obj = payload.as_object_mut().expect("payload object initialized");
    let dedupe = obj.entry("dedupe".to_string()).or_insert_with(|| json!({}));
    if !dedupe.is_object() {
        *dedupe = json!({});
    }
    let dedupe_obj = dedupe.as_object_mut().expect("dedupe object initialized");
    dedupe_obj.insert("key".to_string(), json!(key));
    dedupe_obj.insert("version".to_string(), json!("deterministic-v1"));
    payload
}

fn proposal_dedupe_key(
    payload: &Value,
    suggested_action: &str,
    sensitivity: &str,
) -> Option<String> {
    let target_ref = payload
        .get("target_ref")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let source_hash = payload
        .pointer("/refs/source_hash")
        .or_else(|| payload.pointer("/source_refs/source_hash"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let claim = payload
        .get("proposed_content")
        .and_then(Value::as_str)
        .or_else(|| payload.get("summary").and_then(Value::as_str))
        .map(super::claim_fingerprint::memory_claim_fingerprint)
        .filter(|value| !value.is_empty());
    let evidence = source_hash.map(str::to_string).or(claim)?;
    // ponytail: deterministic proposal dedupe only; upgrade path is semantic match proposals.
    Some(format!(
        "{}|{}|{}|{}",
        suggested_action.trim().to_ascii_lowercase(),
        sensitivity.trim().to_ascii_lowercase(),
        target_ref,
        evidence
    ))
}

pub async fn get_memory_proposal(
    store: &BearMemoryStore,
    proposal_id: &str,
) -> Result<Option<SqliteMemoryProposal>, DenError> {
    let row = sqlx::query_as::<_, ProposalSqlRow>(
        r"
        SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
        FROM memory_proposals
        WHERE bear_id = ? AND proposal_id = ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(proposal_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite get proposal failed: {e}")))?;
    row.map(ProposalSqlRow::into_proposal).transpose()
}

pub async fn list_memory_proposals(
    store: &BearMemoryStore,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<SqliteMemoryProposal>, DenError> {
    let rows = if let Some(status) = status {
        sqlx::query_as::<_, ProposalSqlRow>(
            r"
            SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
            FROM memory_proposals
            WHERE bear_id = ? AND status = ?
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id().to_string())
        .bind(status)
        .bind(limit)
        .fetch_all(store.pool())
        .await
    } else {
        sqlx::query_as::<_, ProposalSqlRow>(
            r"
            SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
            FROM memory_proposals
            WHERE bear_id = ?
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id().to_string())
        .bind(limit)
        .fetch_all(store.pool())
        .await
    }
    .map_err(|e| DenError::System(format!("sqlite list proposals failed: {e}")))?;
    rows.into_iter()
        .map(ProposalSqlRow::into_proposal)
        .collect()
}

pub async fn count_memory_proposals(
    store: &BearMemoryStore,
    status: Option<&str>,
) -> Result<i64, DenError> {
    let count = if let Some(status) = status {
        sqlx::query_scalar::<_, i64>(
            r"
            SELECT COUNT(*)
            FROM memory_proposals
            WHERE bear_id = ? AND status = ?
            ",
        )
        .bind(store.bear_id().to_string())
        .bind(status)
        .fetch_one(store.pool())
        .await
    } else {
        sqlx::query_scalar::<_, i64>(
            r"
            SELECT COUNT(*)
            FROM memory_proposals
            WHERE bear_id = ?
            ",
        )
        .bind(store.bear_id().to_string())
        .fetch_one(store.pool())
        .await
    }
    .map_err(|e| DenError::System(format!("sqlite count proposals failed: {e}")))?;
    Ok(count)
}

pub async fn list_reviewable_memory_proposals(
    store: &BearMemoryStore,
) -> Result<Vec<SqliteMemoryProposal>, DenError> {
    let rows = sqlx::query_as::<_, ProposalSqlRow>(
        r"
        SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
        FROM memory_proposals
        WHERE bear_id = ?
          AND status IN ('pending', 'needs_human_review')
        ORDER BY sequence_no DESC
        ",
    )
    .bind(store.bear_id().to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite list reviewable proposals failed: {e}")))?;
    rows.into_iter()
        .map(ProposalSqlRow::into_proposal)
        .collect()
}

pub async fn resolve_memory_proposal(
    store: &BearMemoryStore,
    proposal_id: &str,
    status: &str,
    review_payload: &Value,
) -> Result<SqliteMemoryProposal, DenError> {
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM memory_proposals WHERE bear_id = ? AND proposal_id = ?",
    )
    .bind(store.bear_id().to_string())
    .bind(proposal_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite fetch proposal for resolve failed: {e}")))?
    .ok_or_else(|| DenError::NotFound("proposal not found".to_string()))?;
    let mut payload: Value = serde_json::from_str(&existing).unwrap_or_else(|_| json!({}));
    if let Some(obj) = review_payload.as_object() {
        for (k, v) in obj {
            payload[k] = v.clone();
        }
    }
    let reviewed_at = now_rfc3339()?;
    sqlx::query(
        r"
        UPDATE memory_proposals
        SET status = ?, payload_json = ?, reviewed_at = ?
        WHERE bear_id = ? AND proposal_id = ?
        ",
    )
    .bind(status)
    .bind(payload.to_string())
    .bind(&reviewed_at)
    .bind(store.bear_id().to_string())
    .bind(proposal_id)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite resolve proposal failed: {e}")))?;
    let row = sqlx::query_as::<_, ProposalSqlRow>(
        r"
        SELECT proposal_id, sequence_no, status, payload_json, created_at, source_memory_id, target_hat_id
        FROM memory_proposals WHERE bear_id = ? AND proposal_id = ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(proposal_id)
    .fetch_one(store.pool())
    .await
    .map_err(|e| DenError::System(format!("sqlite fetch proposal failed: {e}")))?;
    row.into_proposal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::new_test_store;

    #[tokio::test]
    async fn pending_duplicate_proposal_reuses_existing_row() {
        let store = new_test_store().await;
        let first = create_memory_proposal(
            &store,
            "human_review",
            "normal",
            true,
            &json!({
                "target_ref": "core/decisions/runtime.md",
                "proposed_content": "Use SQLite as canonical memory.",
                "refs": { "source_hash": "sha256:abc" }
            }),
        )
        .await
        .unwrap();
        let second = create_memory_proposal(
            &store,
            "human_review",
            "normal",
            true,
            &json!({
                "target_ref": "core/decisions/runtime.md",
                "proposed_content": "Different wording should still dedupe by source hash.",
                "refs": { "source_hash": "sha256:abc" }
            }),
        )
        .await
        .unwrap();

        assert_eq!(second.proposal_id, first.proposal_id);
        let proposals: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM memory_proposals WHERE bear_id = ?")
                .bind(store.bear_id().to_string())
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(proposals, 1);
    }

    #[tokio::test]
    async fn reviewed_duplicate_claim_can_create_new_proposal() {
        let store = new_test_store().await;
        let first = create_memory_proposal(
            &store,
            "human_review",
            "normal",
            true,
            &json!({
                "target_ref": "core/decisions/runtime.md",
                "proposed_content": "Use SQLite as canonical memory."
            }),
        )
        .await
        .unwrap();
        resolve_memory_proposal(&store, &first.proposal_id, "accepted", &json!({}))
            .await
            .unwrap();

        let second = create_memory_proposal(
            &store,
            "human_review",
            "normal",
            true,
            &json!({
                "target_ref": "core/decisions/runtime.md",
                "proposed_content": "use sqlite as canonical memory"
            }),
        )
        .await
        .unwrap();

        assert_ne!(second.proposal_id, first.proposal_id);
        assert_eq!(
            first
                .payload_json
                .pointer("/dedupe/version")
                .and_then(Value::as_str),
            Some("deterministic-v1")
        );
    }

    #[tokio::test]
    async fn archive_index_proposals_are_review_only() {
        let store = new_test_store().await;

        let proposal = create_memory_proposal(
            &store,
            "archive_index",
            "normal",
            false,
            &json!({
                "summary": "Older notes suggest duplicate Den terminology guidance.",
                "source_paths": ["pair/summaries/old.md"],
                "refs": { "source_hash": "sha256:archive" }
            }),
        )
        .await
        .unwrap();

        assert_eq!(proposal.status, "pending");
        assert_eq!(
            proposal
                .payload_json
                .pointer("/archive_lane/status")
                .and_then(Value::as_str),
            Some("review_required")
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/archive_lane/apply_mode")
                .and_then(Value::as_str),
            Some("proposal_only")
        );
        assert_eq!(
            proposal
                .payload_json
                .get("requires_human")
                .and_then(Value::as_bool),
            Some(true)
        );
        let active_records: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_records WHERE bear_id = ? AND invalid_at IS NULL",
        )
        .bind(store.bear_id().to_string())
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(active_records, 0);
    }

    #[tokio::test]
    async fn risky_sensitivity_proposals_require_human_review() {
        let store = new_test_store().await;

        let proposal = create_memory_proposal(
            &store,
            "retain_profile_local",
            "person",
            false,
            &json!({
                "summary": "User prefers concise technical plans.",
                "source_paths": ["pair/notes/profile.md"]
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            proposal
                .payload_json
                .pointer("/risk_review/status")
                .and_then(Value::as_str),
            Some("review_required")
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/risk_review/sensitivity")
                .and_then(Value::as_str),
            Some("person")
        );
        assert_eq!(
            proposal
                .payload_json
                .get("requires_human")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn same_path_different_claim_gets_freshness_review_metadata() {
        let store = new_test_store().await;
        let path = "core/decisions/runtime.md";
        let existing = store
            .append_record(
                &crate::LogicalMemoryPath::from_logical_path(path),
                "decision",
                "curate",
                None,
                "Use SQLite as canonical memory.",
                &json!({}),
                "normal",
            )
            .await
            .unwrap();

        let proposal = create_memory_proposal(
            &store,
            "promote_to_core",
            "normal",
            false,
            &json!({
                "target_ref": path,
                "source_paths": ["pair/summaries/run.md"],
                "proposed_content": "Use Postgres as canonical memory."
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            proposal
                .payload_json
                .pointer("/freshness/status")
                .and_then(Value::as_str),
            Some("potential_conflict")
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/freshness/current_memory_id")
                .and_then(Value::as_str),
            Some(existing.memory_id.as_str())
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/freshness/review_required")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            proposal
                .payload_json
                .get("requires_human")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn same_claim_different_path_gets_consolidation_review_metadata() {
        let store = new_test_store().await;
        let existing = store
            .append_record(
                &crate::LogicalMemoryPath::from_logical_path("core/decisions/runtime.md"),
                "decision",
                "curate",
                None,
                "Use SQLite as canonical memory.",
                &json!({}),
                "normal",
            )
            .await
            .unwrap();

        let proposal = create_memory_proposal(
            &store,
            "promote_to_core",
            "normal",
            false,
            &json!({
                "target_ref": "core/policies/memory.md",
                "source_paths": ["pair/summaries/run.md"],
                "proposed_content": "use sqlite as canonical memory"
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            proposal
                .payload_json
                .pointer("/consolidation_review/status")
                .and_then(Value::as_str),
            Some("needs_human_review")
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/consolidation_review/reason")
                .and_then(Value::as_str),
            Some("same_claim_different_path")
        );
        assert_eq!(
            proposal
                .payload_json
                .pointer("/consolidation_review/candidate_memory_id")
                .and_then(Value::as_str),
            Some(existing.memory_id.as_str())
        );
        assert_eq!(
            proposal
                .payload_json
                .get("requires_human")
                .and_then(Value::as_bool),
            Some(true)
        );
    }
}
