use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
struct ToolOutputArtifactSelectRow {
    id: Uuid,
    tool_call_id: String,
    tool_name: Option<String>,
    source: String,
    content_text: Option<String>,
    content_json: Option<Value>,
    metadata: Value,
}

use den_core::{DenError, RuntimeContextLabel};
use den_service::artifacts::{
    self, ArtifactStorageKind, ArtifactVisibility, AttachConversationArtifactInput,
    FinalizeArtifactInput, ReserveArtifactInput,
};

#[derive(Debug, Clone)]
pub struct ToolOutputArtifactInput {
    pub bear_id: Uuid,
    pub user_id: Option<i32>,
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub tool_call_id: String,
    pub tool_name: Option<String>,
    pub source: &'static str,
    pub content_text: Option<String>,
    pub content_json: Option<Value>,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct ToolOutputArtifactRecord {
    pub id: Uuid,
    /// Legacy session-scoped handle retained for `den.tool_output.read`.
    pub artifact_ref: String,
    /// Registry-backed artifact citation for durable evidence surfaces.
    pub durable_artifact_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolOutputArtifactRead {
    pub id: Uuid,
    pub artifact_ref: String,
    pub tool_call_id: String,
    pub tool_name: Option<String>,
    pub source: String,
    pub content: String,
    pub offset: usize,
    pub limit_chars: usize,
    pub total_chars: usize,
    pub truncated: bool,
    pub metadata: Value,
}

fn artifact_id_from_ref(artifact_ref: &str) -> Result<Uuid, DenError> {
    let raw = artifact_ref
        .strip_prefix("tool-output://")
        .unwrap_or(artifact_ref)
        .trim();
    Uuid::parse_str(raw)
        .map_err(|err| DenError::ValidationError(format!("invalid artifact_ref: {err}")))
}

pub async fn create_tool_output_artifact(
    pool: &PgPool,
    input: ToolOutputArtifactInput,
) -> Result<ToolOutputArtifactRecord, DenError> {
    let content_bytes = input
        .content_text
        .as_ref()
        .map(|text| text.len() as i64)
        .or_else(|| {
            input
                .content_json
                .as_ref()
                .map(|value| value.to_string().len() as i64)
        })
        .unwrap_or(0);
    let metadata = if input.metadata.is_object() {
        input.metadata.clone()
    } else {
        json!({ "value": input.metadata })
    };
    let id = sqlx::query_scalar!(
        r"
        INSERT INTO tool_output_artifacts (
            bear_id, user_id, session_id, conversation_id, run_id, tool_call_id,
            tool_name, source, content_text, content_json, metadata, content_bytes
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        ",
        input.bear_id,
        input.user_id,
        input.session_id,
        input.conversation_id,
        input.run_id,
        input.tool_call_id,
        input.tool_name,
        input.source,
        input.content_text,
        input.content_json,
        metadata,
        content_bytes
    )
    .fetch_one(pool)
    .await
    .map_err(|err| DenError::Database(format!("insert tool output artifact: {err}")))?;
    let durable_artifact_ref = create_durable_tool_output_citation(pool, id, &input)
        .await
        .ok();
    Ok(ToolOutputArtifactRecord {
        id,
        artifact_ref: format!("tool-output://{id}"),
        durable_artifact_ref,
    })
}

async fn create_durable_tool_output_citation(
    pool: &PgPool,
    legacy_id: Uuid,
    input: &ToolOutputArtifactInput,
) -> Result<String, DenError> {
    let artifact = artifacts::reserve_artifact(
        pool,
        ReserveArtifactInput {
            bear_id: input.bear_id,
            created_by_user_id: input.user_id,
            owner_profile: RuntimeContextLabel::ArmatureConversation,
            kind: "tool_output".to_string(),
            title: input.tool_name.clone(),
            summary: Some("Truncated tool output retained for continuation".to_string()),
            content_type: Some("application/json".to_string()),
            storage_kind: ArtifactStorageKind::DbText,
            visibility: ArtifactVisibility::PrivateToProfile,
            provenance: json!({
                "creating_stance": "pair",
                "source": input.source,
                "tool_call_id": input.tool_call_id,
                "tool_name": input.tool_name,
                "session_id": input.session_id,
                "run_id": input.run_id,
            }),
            metadata: json!({ "legacy_tool_output_id": legacy_id }),
            expires_at: None,
        },
    )
    .await?;
    let artifact = artifacts::finalize_metadata_only_artifact(
        pool,
        FinalizeArtifactInput {
            artifact_ref: artifact.artifact_ref.clone(),
            bear_id: input.bear_id,
            storage_key: None,
            content_bytes: input
                .content_text
                .as_ref()
                .map(|content| content.len() as i64)
                .or_else(|| {
                    input
                        .content_json
                        .as_ref()
                        .map(|content| content.to_string().len() as i64)
                }),
            content_sha256: None,
            metadata: json!({ "legacy_tool_output_id": legacy_id }),
        },
    )
    .await?;
    if let Some(conversation_id) = &input.conversation_id {
        artifacts::attach_conversation_artifact(
            pool,
            AttachConversationArtifactInput {
                artifact_ref: artifact.artifact_ref.clone(),
                bear_id: input.bear_id,
                conversation_id: conversation_id.clone(),
                role: "output".to_string(),
                metadata: json!({}),
                created_by_user_id: input.user_id,
            },
        )
        .await?;
    }
    Ok(artifact.artifact_ref)
}

pub async fn read_tool_output_artifact(
    pool: &PgPool,
    bear_id: Uuid,
    session_id: &str,
    artifact_ref: &str,
    offset: usize,
    limit_chars: usize,
) -> Result<ToolOutputArtifactRead, DenError> {
    let id = artifact_id_from_ref(artifact_ref)?;
    let row = sqlx::query_as!(
        ToolOutputArtifactSelectRow,
        r"
        SELECT id, tool_call_id, tool_name, source, content_text, content_json, metadata
        FROM tool_output_artifacts
        WHERE id = $1 AND bear_id = $2 AND session_id = $3
        ",
        id,
        bear_id,
        session_id
    )
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("read tool output artifact: {err}")))?
    .ok_or_else(|| {
        DenError::NotFound("tool output artifact not found for this session".to_string())
    })?;

    let content = row
        .content_text
        .or_else(|| {
            row.content_json.map(|value| {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
            })
        })
        .unwrap_or_default();
    let total_chars = content.chars().count();
    let limit_chars = limit_chars.clamp(1, 24_000);
    let start = offset.min(total_chars);
    let slice = content
        .chars()
        .skip(start)
        .take(limit_chars)
        .collect::<String>();
    let truncated = start + slice.chars().count() < total_chars;
    Ok(ToolOutputArtifactRead {
        id: row.id,
        artifact_ref: format!("tool-output://{}", row.id),
        tool_call_id: row.tool_call_id,
        tool_name: row.tool_name,
        source: row.source,
        content: slice,
        offset: start,
        limit_chars,
        total_chars,
        truncated,
        metadata: row.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_output_artifact_ref() {
        let id = Uuid::new_v4();
        assert_eq!(
            artifact_id_from_ref(&format!("tool-output://{id}")).unwrap(),
            id
        );
        assert_eq!(artifact_id_from_ref(&id.to_string()).unwrap(), id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn writes_and_reads_tool_output_artifact_slice(pool: PgPool) {
        let suffix = Uuid::new_v4();
        let bear_id = sqlx::query_scalar!(
            "INSERT INTO bears (slug, name, description) VALUES ($1, $2, $3) RETURNING id",
            format!("artifact-bear-{}", suffix.simple()),
            "Artifact Bear",
            "artifact test bear"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let artifact = create_tool_output_artifact(
            &pool,
            ToolOutputArtifactInput {
                bear_id,
                user_id: None,
                session_id: "session-1".to_string(),
                conversation_id: Some("conv-1".to_string()),
                run_id: Some("run-1".to_string()),
                tool_call_id: "call-1".to_string(),
                tool_name: Some("search".to_string()),
                source: "den_hosted",
                content_text: Some("abcdef".to_string()),
                content_json: None,
                metadata: json!({ "test": true }),
            },
        )
        .await
        .unwrap();

        let read =
            read_tool_output_artifact(&pool, bear_id, "session-1", &artifact.artifact_ref, 2, 3)
                .await
                .unwrap();

        assert_eq!(read.content, "cde");
        assert_eq!(read.total_chars, 6);
        assert!(read.truncated);
        assert_eq!(read.tool_call_id, "call-1");

        let durable_ref = artifact
            .durable_artifact_ref
            .as_deref()
            .expect("durable citation");
        let citations = artifacts::list_conversation_artifact_citations(
            &pool,
            bear_id,
            "conv-1",
            artifacts::ArtifactAccessContext {
                bear_id,
                user_id: None,
            },
        )
        .await
        .unwrap();
        let citation = citations
            .iter()
            .find(|citation| citation.artifact_ref == durable_ref)
            .expect("anonymous durable citation is retained but redacted");
        assert!(!citation.readable);
        assert_eq!(citation.kind, "unavailable");
        assert!(citation.title.is_none());
        assert!(citation.summary.is_none());
        assert!(citation.content_type.is_none());
        assert!(citation.content_bytes.is_none());
        let rendered = serde_json::to_value(citation).unwrap();
        assert!(rendered.get("storage_key").is_none());
        assert!(rendered.get("content_sha256").is_none());
        assert!(rendered.get("provenance").is_none());
    }
}
