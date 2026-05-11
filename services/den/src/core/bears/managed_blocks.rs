use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

use crate::errors::CustomError;

use super::{context_composition, Bear, BearAgentRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemBlockKind {
    PromptText,
    ToolInstruction,
    PolicyText,
}

impl SystemBlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptText => "prompt_text",
            Self::ToolInstruction => "tool_instruction",
            Self::PolicyText => "policy_text",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemBlockScope {
    Global,
    Space,
    Template,
}

impl SystemBlockScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Space => "space",
            Self::Template => "template",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearBlockBindingMode {
    Inherit,
    Custom,
}

impl BearBlockBindingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SystemBlockRow {
    pub key: String,
    pub kind: String,
    pub scope: String,
    pub status: String,
    pub current_published_version_id: Option<i64>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SystemBlockVersionRow {
    pub id: i64,
    pub block_key: String,
    pub version_number: i32,
    pub content: String,
    pub change_summary: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearBlockBindingRow {
    pub bear_id: Uuid,
    pub block_key: String,
    pub mode: String,
    pub custom_content: Option<String>,
    pub forked_from_version_id: Option<i64>,
    pub last_reviewed_version_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedSystemBlock {
    pub key: &'static str,
    pub kind: SystemBlockKind,
    pub scope: SystemBlockScope,
    pub content: String,
    pub change_summary: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedManagedBlock {
    pub key: String,
    pub kind: String,
    pub scope: String,
    pub source_mode: String,
    pub effective_content: String,
    pub effective_content_hash: String,
    pub system_version_id: Option<i64>,
    pub system_version_number: Option<i32>,
    pub forked_from_version_id: Option<i64>,
    pub last_reviewed_version_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedManagedBlockSet {
    pub bear_id: Uuid,
    pub blocks: Vec<ResolvedManagedBlock>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearCompiledConfigRow {
    pub bear_id: Uuid,
    pub compiled_version: i32,
    pub resolved_blocks_json: Json<serde_json::Value>,
    pub rendered_prompts_json: Json<serde_json::Value>,
    pub rendered_prompt_hashes_json: Json<serde_json::Value>,
    pub tool_guidance_hashes_json: Json<serde_json::Value>,
    pub config_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledBearConfig {
    pub bear_id: Uuid,
    pub compiled_version: i32,
    pub resolved_blocks: ResolvedManagedBlockSet,
    pub rendered_prompts: serde_json::Value,
    pub rendered_prompt_hashes: serde_json::Value,
    pub tool_guidance_hashes: serde_json::Value,
    pub config_hash: String,
}

pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn managed_space_block_key(role: BearAgentRole) -> &'static str {
    match role {
        BearAgentRole::Talk => "space_instruction.talk",
        BearAgentRole::Pair => "space_instruction.pair",
        BearAgentRole::Curate => "space_instruction.curate",
        BearAgentRole::Work => "space_instruction.work",
        BearAgentRole::Watch => "space_instruction.watch",
    }
}

pub fn system_block_seed_data() -> Vec<SeedSystemBlock> {
    let defaults = context_composition::default_role_contracts_for_bear("the Bear");
    vec![
        SeedSystemBlock {
            key: "den_baseline",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Global,
            content: context_composition::den_baseline().to_string(),
            change_summary: "Seed current Den baseline prompt text.",
        },
        SeedSystemBlock {
            key: "space_instruction.talk",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.talk,
            change_summary: "Seed current talk space instruction text.",
        },
        SeedSystemBlock {
            key: "space_instruction.pair",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.pair,
            change_summary: "Seed current pair space instruction text.",
        },
        SeedSystemBlock {
            key: "space_instruction.curate",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.curate,
            change_summary: "Seed current curate space instruction text.",
        },
        SeedSystemBlock {
            key: "space_instruction.work",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.work,
            change_summary: "Seed current work space instruction text.",
        },
        SeedSystemBlock {
            key: "space_instruction.watch",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.watch,
            change_summary: "Seed current watch space instruction text.",
        },
    ]
}

pub async fn seed_system_blocks(pool: &PgPool) -> Result<(), CustomError> {
    for block in system_block_seed_data() {
        sqlx::query(
            r#"
            INSERT INTO system_blocks (key, kind, scope, status)
            VALUES ($1, $2, $3, 'published')
            ON CONFLICT (key) DO NOTHING
            "#,
        )
        .bind(block.key)
        .bind(block.kind.as_str())
        .bind(block.scope.as_str())
        .execute(pool)
        .await?;

        let hash = content_hash(&block.content);
        let existing: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT id
            FROM system_block_versions
            WHERE block_key = $1 AND content_hash = $2
            "#,
        )
        .bind(block.key)
        .bind(&hash)
        .fetch_optional(pool)
        .await?;

        let version_id = if let Some((id,)) = existing {
            id
        } else {
            let next_version: (i32,) = sqlx::query_as(
                r#"
                SELECT COALESCE(MAX(version_number), 0)::integer + 1
                FROM system_block_versions
                WHERE block_key = $1
                "#,
            )
            .bind(block.key)
            .fetch_one(pool)
            .await?;

            let inserted: (i64,) = sqlx::query_as(
                r#"
                INSERT INTO system_block_versions (
                    block_key, version_number, content, change_summary, content_hash
                )
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                "#,
            )
            .bind(block.key)
            .bind(next_version.0)
            .bind(&block.content)
            .bind(block.change_summary)
            .bind(&hash)
            .fetch_one(pool)
            .await?;
            inserted.0
        };

        sqlx::query(
            r#"
            UPDATE system_blocks
            SET status = 'published',
                current_published_version_id = $2,
                updated_at = now()
            WHERE key = $1
            "#,
        )
        .bind(block.key)
        .bind(version_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn list_system_blocks(pool: &PgPool) -> Result<Vec<SystemBlockRow>, CustomError> {
    sqlx::query_as::<_, SystemBlockRow>(
        r#"
        SELECT key, kind, scope, status, current_published_version_id
        FROM system_blocks
        ORDER BY key
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_system_block_versions(
    pool: &PgPool,
    block_key: &str,
) -> Result<Vec<SystemBlockVersionRow>, CustomError> {
    sqlx::query_as::<_, SystemBlockVersionRow>(
        r#"
        SELECT id, block_key, version_number, content, change_summary, content_hash
        FROM system_block_versions
        WHERE block_key = $1
        ORDER BY version_number DESC
        "#,
    )
    .bind(block_key)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_bear_block_bindings(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearBlockBindingRow>, CustomError> {
    sqlx::query_as::<_, BearBlockBindingRow>(
        r#"
        SELECT bear_id, block_key, mode, custom_content, forked_from_version_id, last_reviewed_version_id
        FROM bear_block_bindings
        WHERE bear_id = $1
        ORDER BY block_key
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn upsert_bear_block_binding(
    pool: &PgPool,
    bear_id: Uuid,
    block_key: &str,
    mode: BearBlockBindingMode,
    custom_content: Option<&str>,
    forked_from_version_id: Option<i64>,
    last_reviewed_version_id: Option<i64>,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO bear_block_bindings (
            bear_id, block_key, mode, custom_content, forked_from_version_id, last_reviewed_version_id
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (bear_id, block_key)
        DO UPDATE SET mode = EXCLUDED.mode,
                      custom_content = EXCLUDED.custom_content,
                      forked_from_version_id = EXCLUDED.forked_from_version_id,
                      last_reviewed_version_id = EXCLUDED.last_reviewed_version_id,
                      updated_at = now()
        "#,
    )
    .bind(bear_id)
    .bind(block_key)
    .bind(mode.as_str())
    .bind(custom_content)
    .bind(forked_from_version_id)
    .bind(last_reviewed_version_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn resolve_managed_blocks_for_bear(
    pool: &PgPool,
    bear: &Bear,
) -> Result<ResolvedManagedBlockSet, CustomError> {
    let rows: Vec<(
        String,
        String,
        String,
        Option<i64>,
        Option<i32>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    )> = sqlx::query_as(
        r#"
        SELECT sb.key,
               sb.kind,
               sb.scope,
               sbv.id AS system_version_id,
               sbv.version_number AS system_version_number,
               sbv.content AS system_content,
               sbv.content_hash AS system_content_hash,
               bbb.mode,
               bbb.custom_content,
               bbb.forked_from_version_id,
               bbb.last_reviewed_version_id
        FROM system_blocks sb
        LEFT JOIN system_block_versions sbv ON sbv.id = sb.current_published_version_id
        LEFT JOIN bear_block_bindings bbb
               ON bbb.bear_id = $1 AND bbb.block_key = sb.key
        WHERE sb.status = 'published'
        ORDER BY sb.key
        "#,
    )
    .bind(bear.id)
    .fetch_all(pool)
    .await?;

    let mut blocks = Vec::with_capacity(rows.len());
    for (
        key,
        kind,
        scope,
        system_version_id,
        system_version_number,
        system_content,
        system_content_hash,
        mode,
        custom_content,
        forked_from_version_id,
        last_reviewed_version_id,
    ) in rows
    {
        let mode = mode.unwrap_or_else(|| BearBlockBindingMode::Inherit.as_str().to_string());
        let (effective_content, effective_hash, source_mode) = match mode.as_str() {
            "inherit" => {
                let content = system_content.ok_or_else(|| {
                    CustomError::System(format!(
                        "published system block {key} is missing current_published_version content"
                    ))
                })?;
                let hash = system_content_hash.unwrap_or_else(|| content_hash(&content));
                (content, hash, mode)
            }
            "custom" => {
                let content = custom_content.ok_or_else(|| {
                    CustomError::ValidationError(format!(
                        "custom binding for block {key} is missing custom_content"
                    ))
                })?;
                let hash = content_hash(&content);
                (content, hash, mode)
            }
            other => {
                return Err(CustomError::ValidationError(format!(
                    "unknown bear block binding mode for {key}: {other}"
                )));
            }
        };

        blocks.push(ResolvedManagedBlock {
            key,
            kind,
            scope,
            source_mode,
            effective_content,
            effective_content_hash: effective_hash,
            system_version_id,
            system_version_number,
            forked_from_version_id,
            last_reviewed_version_id,
        });
    }

    Ok(ResolvedManagedBlockSet {
        bear_id: bear.id,
        blocks,
    })
}

pub fn resolved_blocks_json(
    resolved: &ResolvedManagedBlockSet,
) -> Result<Json<serde_json::Value>, CustomError> {
    serde_json::to_value(resolved)
        .map(Json)
        .map_err(|e| CustomError::Parsing(format!("serialize resolved managed blocks: {e}")))
}

pub fn compile_managed_config_for_bear(
    bear: &Bear,
    resolved: ResolvedManagedBlockSet,
) -> Result<CompiledBearConfig, CustomError> {
    let mut rendered_prompts = serde_json::Map::new();
    let mut rendered_prompt_hashes = serde_json::Map::new();

    for role in BearAgentRole::ALL {
        let role_prompt = context_composition::render_managed_role_prompt(bear, role, Some(&resolved))?;
        let role_key = role.as_str().to_string();
        rendered_prompt_hashes.insert(role_key.clone(), json!(content_hash(&role_prompt)));
        rendered_prompts.insert(role_key, json!(role_prompt));
    }

    let resolved_json = serde_json::to_value(&resolved)
        .map_err(|e| CustomError::Parsing(format!("serialize resolved managed blocks: {e}")))?;
    let rendered_prompts_value = serde_json::Value::Object(rendered_prompts);
    let rendered_prompt_hashes_value = serde_json::Value::Object(rendered_prompt_hashes);
    let tool_guidance_hashes_value = json!({});
    let config_payload = json!({
        "resolved_blocks": resolved_json,
        "rendered_prompts": rendered_prompts_value,
        "rendered_prompt_hashes": rendered_prompt_hashes_value,
        "tool_guidance_hashes": tool_guidance_hashes_value,
    });
    let config_hash = content_hash(&config_payload.to_string());

    Ok(CompiledBearConfig {
        bear_id: bear.id,
        compiled_version: 1,
        resolved_blocks,
        rendered_prompts: rendered_prompts_value,
        rendered_prompt_hashes: rendered_prompt_hashes_value,
        tool_guidance_hashes: tool_guidance_hashes_value,
        config_hash,
    })
}

pub async fn upsert_compiled_bear_config(
    pool: &PgPool,
    compiled: &CompiledBearConfig,
) -> Result<(), CustomError> {
    let resolved_blocks_json = serde_json::to_value(&compiled.resolved_blocks).map_err(|e| {
        CustomError::Parsing(format!("serialize compiled resolved managed blocks: {e}"))
    })?;
    sqlx::query(
        r#"
        INSERT INTO bear_compiled_configs (
            bear_id,
            compiled_version,
            resolved_blocks_json,
            rendered_prompts_json,
            rendered_prompt_hashes_json,
            tool_guidance_hashes_json,
            config_hash,
            compiled_at,
            updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, now(), now())
        ON CONFLICT (bear_id)
        DO UPDATE SET compiled_version = EXCLUDED.compiled_version,
                      resolved_blocks_json = EXCLUDED.resolved_blocks_json,
                      rendered_prompts_json = EXCLUDED.rendered_prompts_json,
                      rendered_prompt_hashes_json = EXCLUDED.rendered_prompt_hashes_json,
                      tool_guidance_hashes_json = EXCLUDED.tool_guidance_hashes_json,
                      config_hash = EXCLUDED.config_hash,
                      compiled_at = now(),
                      updated_at = now()
        "#,
    )
    .bind(compiled.bear_id)
    .bind(compiled.compiled_version)
    .bind(Json(resolved_blocks_json))
    .bind(Json(compiled.rendered_prompts.clone()))
    .bind(Json(compiled.rendered_prompt_hashes.clone()))
    .bind(Json(compiled.tool_guidance_hashes.clone()))
    .bind(&compiled.config_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_compiled_bear_config(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Option<BearCompiledConfigRow>, CustomError> {
    sqlx::query_as::<_, BearCompiledConfigRow>(
        r#"
        SELECT bear_id,
               compiled_version,
               resolved_blocks_json,
               rendered_prompts_json,
               rendered_prompt_hashes_json,
               tool_guidance_hashes_json,
               config_hash
        FROM bear_compiled_configs
        WHERE bear_id = $1
        "#,
    )
    .bind(bear_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn compile_and_store_managed_config_for_bear(
    pool: &PgPool,
    bear: &Bear,
) -> Result<CompiledBearConfig, CustomError> {
    seed_system_blocks(pool).await?;
    let resolved = resolve_managed_blocks_for_bear(pool, bear).await?;
    let compiled = compile_managed_config_for_bear(bear, resolved)?;
    upsert_compiled_bear_config(pool, &compiled).await?;
    Ok(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::Json;
    use time::OffsetDateTime;

    fn test_bear() -> Bear {
        Bear {
            id: Uuid::nil(),
            slug: "builder".to_string(),
            name: "Builder Bear".to_string(),
            description: String::new(),
            default_model: Some("openai/gpt-4o".to_string()),
            tools_enabled: None,
            letta_agent_type: None,
            letta_tool_ids: Json(Vec::new()),
            runtime_plan: None,
            context_profile: None,
            memfs_repo_path: None,
            provisioning_version: 1,
            system_prompt: String::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn content_hash_is_deterministic() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abcd"));
    }

    #[test]
    fn managed_space_block_key_matches_roles() {
        assert_eq!(managed_space_block_key(BearAgentRole::Talk), "space_instruction.talk");
        assert_eq!(managed_space_block_key(BearAgentRole::Pair), "space_instruction.pair");
        assert_eq!(managed_space_block_key(BearAgentRole::Curate), "space_instruction.curate");
        assert_eq!(managed_space_block_key(BearAgentRole::Work), "space_instruction.work");
        assert_eq!(managed_space_block_key(BearAgentRole::Watch), "space_instruction.watch");
    }

    #[test]
    fn seed_data_contains_expected_blocks_in_order() {
        let blocks = system_block_seed_data();
        let keys: Vec<&str> = blocks.iter().map(|b| b.key).collect();
        assert_eq!(
            keys,
            vec![
                "den_baseline",
                "space_instruction.talk",
                "space_instruction.pair",
                "space_instruction.curate",
                "space_instruction.work",
                "space_instruction.watch",
            ]
        );
    }

    #[test]
    fn resolved_blocks_json_serializes() {
        let resolved = ResolvedManagedBlockSet {
            bear_id: test_bear().id,
            blocks: vec![ResolvedManagedBlock {
                key: "den_baseline".to_string(),
                kind: "prompt_text".to_string(),
                scope: "global".to_string(),
                source_mode: "inherit".to_string(),
                effective_content: "hello".to_string(),
                effective_content_hash: content_hash("hello"),
                system_version_id: Some(1),
                system_version_number: Some(1),
                forked_from_version_id: None,
                last_reviewed_version_id: None,
            }],
        };
        let json = resolved_blocks_json(&resolved).unwrap();
        assert_eq!(json.0["blocks"][0]["key"], "den_baseline");
    }
}
