use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

use den_core::DenError;

use super::prompt_fragments::{
    repository_prompt_bundle_registry, repository_prompt_fragment_registry,
    repository_prompt_source_version,
};
use super::{context_composition, Bear, BearProfile};

#[derive(Debug, Clone, FromRow)]
struct ManagedBlockResolutionRow {
    key: String,
    kind: String,
    scope: String,
    system_version_id: Option<i64>,
    system_version_number: Option<i32>,
    system_content: Option<String>,
    system_content_hash: Option<String>,
    mode: Option<String>,
    custom_content: Option<String>,
    forked_from_version_id: Option<i64>,
    last_reviewed_version_id: Option<i64>,
}

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

pub fn managed_space_block_key(role: BearProfile) -> String {
    format!("space_instruction.{}", role.as_str())
}

pub fn system_block_seed_data() -> Vec<SeedSystemBlock> {
    let registry = repository_prompt_fragment_registry().ok();
    let defaults = context_composition::default_role_contracts_for_bear("the Bear");
    let den_baseline_content = registry
        .as_ref()
        .and_then(|registry| registry.get("den_baseline"))
        .map(|fragment| fragment.body.clone())
        .unwrap_or_else(|| context_composition::den_baseline().to_string());
    let pair_contract_content = registry
        .as_ref()
        .and_then(|registry| registry.get("stance_pair"))
        .map(|fragment| fragment.body.clone())
        .unwrap_or_else(|| defaults.pair.clone());
    vec![
        SeedSystemBlock {
            key: "den_baseline",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Global,
            content: den_baseline_content,
            change_summary: "Seed current Den baseline prompt text.",
        },
        SeedSystemBlock {
            key: "space_instruction.chat",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: defaults.chat,
            change_summary: "Seed current chat space instruction text.",
        },
        SeedSystemBlock {
            key: "space_instruction.pair",
            kind: SystemBlockKind::PromptText,
            scope: SystemBlockScope::Space,
            content: pair_contract_content,
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

pub async fn seed_system_blocks(pool: &PgPool) -> Result<(), DenError> {
    for block in system_block_seed_data() {
        sqlx::query(
            r"
            INSERT INTO system_blocks (key, kind, scope, status)
            VALUES ($1, $2, $3, 'published')
            ON CONFLICT (key) DO NOTHING
            ",
        )
        .bind(block.key)
        .bind(block.kind.as_str())
        .bind(block.scope.as_str())
        .execute(pool)
        .await?;

        let hash = content_hash(&block.content);
        let existing: Option<(i64,)> = sqlx::query_as(
            r"
            SELECT id
            FROM system_block_versions
            WHERE block_key = $1 AND content_hash = $2
            ",
        )
        .bind(block.key)
        .bind(&hash)
        .fetch_optional(pool)
        .await?;

        let version_id = if let Some((id,)) = existing {
            id
        } else {
            let next_version: (i32,) = sqlx::query_as(
                r"
                SELECT COALESCE(MAX(version_number), 0)::integer + 1
                FROM system_block_versions
                WHERE block_key = $1
                ",
            )
            .bind(block.key)
            .fetch_one(pool)
            .await?;

            let inserted: (i64,) = sqlx::query_as(
                r"
                INSERT INTO system_block_versions (
                    block_key, version_number, content, change_summary, content_hash
                )
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                ",
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
            r"
            UPDATE system_blocks
            SET status = 'published',
                current_published_version_id = $2,
                updated_at = now()
            WHERE key = $1
            ",
        )
        .bind(block.key)
        .bind(version_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn list_system_blocks(pool: &PgPool) -> Result<Vec<SystemBlockRow>, DenError> {
    sqlx::query_as::<_, SystemBlockRow>(
        r"
        SELECT key, kind, scope, status, current_published_version_id
        FROM system_blocks
        ORDER BY key
        ",
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_system_block_versions(
    pool: &PgPool,
    block_key: &str,
) -> Result<Vec<SystemBlockVersionRow>, DenError> {
    sqlx::query_as::<_, SystemBlockVersionRow>(
        r"
        SELECT id, block_key, version_number, content, change_summary, content_hash
        FROM system_block_versions
        WHERE block_key = $1
        ORDER BY version_number DESC
        ",
    )
    .bind(block_key)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_bear_block_bindings(
    pool: &PgPool,
    bear_id: Uuid,
) -> Result<Vec<BearBlockBindingRow>, DenError> {
    sqlx::query_as::<_, BearBlockBindingRow>(
        r"
        SELECT bear_id, block_key, mode, custom_content, forked_from_version_id, last_reviewed_version_id
        FROM bear_block_bindings
        WHERE bear_id = $1
        ORDER BY block_key
        ",
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
) -> Result<(), DenError> {
    sqlx::query(
        r"
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
        ",
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
) -> Result<ResolvedManagedBlockSet, DenError> {
    let rows: Vec<ManagedBlockResolutionRow> = sqlx::query_as(
        r"
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
        ",
    )
    .bind(bear.id)
    .fetch_all(pool)
    .await?;

    let mut blocks = Vec::with_capacity(rows.len());
    for row in rows {
        let ManagedBlockResolutionRow {
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
        } = row;
        let mode = mode.unwrap_or_else(|| BearBlockBindingMode::Inherit.as_str().to_string());
        let (effective_content, effective_hash, source_mode) = match mode.as_str() {
            "inherit" => {
                let content = system_content.ok_or_else(|| {
                    DenError::System(format!(
                        "published system block {key} is missing current_published_version content"
                    ))
                })?;
                let hash = system_content_hash.unwrap_or_else(|| content_hash(&content));
                (content, hash, mode)
            }
            "custom" => {
                let content = custom_content.ok_or_else(|| {
                    DenError::ValidationError(format!(
                        "custom binding for block {key} is missing custom_content"
                    ))
                })?;
                let hash = content_hash(&content);
                (content, hash, mode)
            }
            other => {
                return Err(DenError::ValidationError(format!(
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
) -> Result<Json<serde_json::Value>, DenError> {
    serde_json::to_value(resolved)
        .map(Json)
        .map_err(|e| DenError::Parsing(format!("serialize resolved managed blocks: {e}")))
}

pub fn compile_managed_config_for_bear(
    bear: &Bear,
    resolved: ResolvedManagedBlockSet,
) -> Result<CompiledBearConfig, DenError> {
    let prompt_registry = repository_prompt_fragment_registry()?;
    let bundle_registry = repository_prompt_bundle_registry(&prompt_registry)?;
    bundle_registry.require("pair")?;
    let mut rendered_prompts = serde_json::Map::new();
    let mut rendered_prompt_hashes = serde_json::Map::new();

    for role in BearProfile::ALL {
        let role_prompt = context_composition::render_managed_role_prompt_with_registry(
            bear,
            role,
            Some(&resolved),
            Some(&prompt_registry),
        )?;
        let role_key = role.as_str().to_string();
        rendered_prompt_hashes.insert(role_key.clone(), json!(content_hash(&role_prompt)));
        rendered_prompts.insert(role_key, json!(role_prompt));
    }

    let bound_base = context_composition::render_bound_base_prompt_with_registry(
        bear,
        Some(&resolved),
        &prompt_registry,
    )?;
    rendered_prompt_hashes.insert("bound_base".into(), json!(content_hash(&bound_base)));
    rendered_prompts.insert("bound_base".into(), json!(bound_base));
    rendered_prompts.insert(
        "bound_source_version".into(),
        json!(repository_prompt_source_version()),
    );
    for mode in ["bound_chat_mode", "bound_pair_mode", "bound_work_mode"] {
        let fragment = prompt_registry.require(mode)?;
        let rendered = super::prompt_fragments::render_compile_time_fragment(
            fragment,
            &super::prompt_fragments::CompileTimePromptContext {
                bear_name: &bear.name,
                bear_slug: &bear.slug,
            },
        )?;
        rendered_prompt_hashes.insert(mode.into(), json!(content_hash(&rendered)));
        rendered_prompts.insert(mode.into(), json!(rendered));
    }

    let resolved_json = serde_json::to_value(&resolved)
        .map_err(|e| DenError::Parsing(format!("serialize resolved managed blocks: {e}")))?;
    let rendered_prompts_value = serde_json::Value::Object(rendered_prompts);
    let rendered_prompt_hashes_value = serde_json::Value::Object(rendered_prompt_hashes);
    let tool_guidance_hashes_value = json!({});
    let config_payload = json!({
        "resolved_blocks": resolved_json,
        "rendered_prompts": rendered_prompts_value,
        "rendered_prompt_hashes": rendered_prompt_hashes_value,
        "tool_guidance_hashes": tool_guidance_hashes_value,
        "prompt_source_version": repository_prompt_source_version(),
    });
    let config_hash = content_hash(&config_payload.to_string());

    Ok(CompiledBearConfig {
        bear_id: bear.id,
        compiled_version: 1,
        resolved_blocks: resolved,
        rendered_prompts: rendered_prompts_value,
        rendered_prompt_hashes: rendered_prompt_hashes_value,
        tool_guidance_hashes: tool_guidance_hashes_value,
        config_hash,
    })
}

pub async fn upsert_compiled_bear_config(
    pool: &PgPool,
    compiled: &CompiledBearConfig,
) -> Result<(), DenError> {
    let resolved_blocks_json = serde_json::to_value(&compiled.resolved_blocks).map_err(|e| {
        DenError::Parsing(format!("serialize compiled resolved managed blocks: {e}"))
    })?;
    sqlx::query(
        r"
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
        ",
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
) -> Result<Option<BearCompiledConfigRow>, DenError> {
    sqlx::query_as::<_, BearCompiledConfigRow>(
        r"
        SELECT bear_id,
               compiled_version,
               resolved_blocks_json,
               rendered_prompts_json,
               rendered_prompt_hashes_json,
               tool_guidance_hashes_json,
               config_hash
        FROM bear_compiled_configs
        WHERE bear_id = $1
        ",
    )
    .bind(bear_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn compile_and_store_managed_config_for_bear(
    pool: &PgPool,
    bear: &Bear,
) -> Result<CompiledBearConfig, DenError> {
    seed_system_blocks(pool).await?;
    let resolved = resolve_managed_blocks_for_bear(pool, bear).await?;
    let compiled = compile_managed_config_for_bear(bear, resolved)?;
    upsert_compiled_bear_config(pool, &compiled).await?;
    Ok(compiled)
}

#[cfg(test)]
mod tests;
