use crate::bears::{
    hats::memory_binding,
    model_configurations::{self, PrimaryModelSource, ResolvedPrimaryModel},
};
use den_core::{
    ids::{BearId, ModelConfigurationId},
    DenError, ThinkingEffort,
};
use den_llm::model_registry::{
    ModelTokenCalibration, MODEL_TOKEN_CALIBRATION_EMA_ALPHA, MODEL_TOKEN_CALIBRATION_MAX_RATIO,
    MODEL_TOKEN_CALIBRATION_MIN_RATIO,
};
use den_llm::ModelOption;
use serde_json::Value;
use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

use crate::{bears, conversation::persistence};

#[derive(Debug, Clone, FromRow)]
pub struct ModelSelectionOptionRow {
    pub handle: String,
    pub display_name: String,
    pub metadata_json: Json<Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConversationModelSelectionView {
    pub selection_mode: String,
    pub requested_model: Option<String>,
    pub selected_model: Option<String>,
    pub effective_model: String,
    pub source: String,
    pub configuration_id: Option<ModelConfigurationId>,
    pub configuration_name: Option<String>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub model_options: Vec<ModelOption>,
}

fn metadata_u32(metadata: &Value, key: &str) -> Option<u32> {
    metadata
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn option_from_row(row: ModelSelectionOptionRow) -> ModelOption {
    if let Some(entry) = den_llm::model_registry::entry_for_handle(&row.handle) {
        return entry.to_model_option_for_handle(&row.handle);
    }
    let context_window = metadata_u32(&row.metadata_json.0, "context_window");
    let max_output_tokens = metadata_u32(&row.metadata_json.0, "max_output_tokens");
    ModelOption {
        handle: row.handle,
        label: row.display_name,
        context_window,
        max_output_tokens,
    }
}

/// Stable Den-owned model options for user-facing selectors.
///
/// Bifrost remains the execution/availability authority, but the selector list
/// should not flicker when Bifrost's live catalog temporarily shrinks or expands.
pub async fn list_selectable_model_options(pool: &PgPool) -> Result<Vec<ModelOption>, DenError> {
    let rows = match sqlx::query_as::<_, ModelSelectionOptionRow>(
        r"
        SELECT handle, display_name, metadata_json
        FROM model_selection_options
        WHERE selectable = TRUE
        ORDER BY COALESCE(sort_order, 100000), display_name, handle
        ",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not load model_selection_options; using static Den model registry fallback"
            );
            return Ok(den_llm::model_registry::selectable_model_options());
        }
    };

    let mut options = rows.into_iter().map(option_from_row).collect::<Vec<_>>();
    if options.is_empty() {
        options = den_llm::model_registry::selectable_model_options();
    }
    Ok(options)
}

fn simplify_model_option_label_for_acp(label: &str) -> String {
    // ACP option labels have little horizontal space. Registry labels commonly append
    // operator metadata as either ` — metadata unknown` or parenthesized token limits
    // like `GPT-5 (1M ctx / 128k out)`; strip those suffixes for display only.
    label
        .trim()
        .strip_suffix(" — metadata unknown")
        .unwrap_or_else(|| label.trim())
        .split(" (")
        .next()
        .unwrap_or_else(|| label.trim())
        .trim()
        .to_string()
}

pub fn simplify_model_option_for_acp(option: &ModelOption) -> ModelOption {
    ModelOption {
        handle: option.handle.clone(),
        label: simplify_model_option_label_for_acp(&option.label),
        context_window: None,
        max_output_tokens: None,
    }
}

pub async fn list_selectable_model_options_for_acp(
    pool: &PgPool,
) -> Result<Vec<ModelOption>, DenError> {
    Ok(list_selectable_model_options(pool)
        .await?
        .iter()
        .map(simplify_model_option_for_acp)
        .collect())
}

pub async fn apply_conversation_model_selection(
    pool: &PgPool,
    conversation_id: Uuid,
    selection_mode: &str,
    requested_model: Option<&str>,
    explicit_reason: &str,
    auto_reason: &str,
) -> Result<crate::conversation::persistence::ConversationModelState, DenError> {
    let selected = match selection_mode.trim() {
        "explicit" => Some(
            bears::model_configurations::validate_model_configuration(
                pool,
                requested_model.unwrap_or(""),
                None,
            )
            .await?
            .model_handle
            .into_string(),
        ),
        "auto" => None,
        _ => {
            return Err(DenError::ValidationError(
                "choose an explicit model pin or automatic inheritance".into(),
            ));
        }
    };
    crate::conversation::persistence::set_conversation_model_state(
        pool,
        conversation_id,
        if selected.is_some() {
            "explicit"
        } else {
            "auto"
        },
        selected.as_deref(),
        selected.as_deref(),
        Some(if selected.is_some() {
            explicit_reason
        } else {
            auto_reason
        }),
    )
    .await
}

pub async fn load_conversation_model_selection_view(
    pool: &PgPool,
    bear: &bears::Bear,
    user_id: i32,
    default_model: &str,
    conversation_external_id: &str,
    source_client_session_id: Option<&str>,
    acp_friendly_options: bool,
) -> Result<ConversationModelSelectionView, DenError> {
    let model_options = if acp_friendly_options {
        list_selectable_model_options_for_acp(pool).await?
    } else {
        list_selectable_model_options(pool).await?
    };
    let conversation = persistence::ensure_conversation_for_external_id(
        pool,
        bear.id,
        Some(user_id),
        conversation_external_id,
        source_client_session_id,
        None,
    )
    .await?;
    let model_state = persistence::get_conversation_model_state(pool, conversation.id).await?;
    let primary =
        resolve_conversation_primary_model(pool, bear.id.into(), conversation.id, default_model)
            .await?;
    Ok(ConversationModelSelectionView {
        selection_mode: model_state
            .as_ref()
            .map(|row| row.selection_mode.clone())
            .unwrap_or_else(|| "auto".to_string()),
        requested_model: model_state
            .as_ref()
            .and_then(|row| row.requested_model.clone()),
        selected_model: model_state
            .as_ref()
            .and_then(|row| row.selected_model.clone()),
        source: primary_source_label(primary.source).to_string(),
        effective_model: primary.model_handle,
        configuration_id: primary.configuration_id,
        configuration_name: primary.configuration_name,
        thinking_effort: primary.thinking_effort,
        model_options,
    })
}

/// Parse the legacy persisted selector at its boundary. An invalid explicit pin
/// must not disappear into inheritance (including a blank or missing model).
pub async fn conversation_model_pin(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Option<String>, DenError> {
    let Some(state) = persistence::get_conversation_model_state(pool, conversation_id).await?
    else {
        return Ok(None);
    };
    match state.selection_mode.as_str() {
        "auto" => Ok(None),
        "explicit" => state
            .selected_model
            .or(state.requested_model)
            .map(Some)
            .ok_or_else(|| {
                DenError::ValidationError(
                    "explicit conversation model selection has no model".into(),
                )
            }),
        _ => Err(DenError::ValidationError(
            "invalid conversation model selection mode".into(),
        )),
    }
}

/// Resolve from the canonical conversation's verified hat, never a runtime target.
pub async fn resolve_conversation_primary_model(
    pool: &PgPool,
    bear_id: BearId,
    conversation_id: Uuid,
    deployment_default: &str,
) -> Result<ResolvedPrimaryModel, DenError> {
    let memory_binding::ResolvedMemoryBinding::Bound(grant) =
        memory_binding::for_conversation(pool, bear_id, conversation_id).await?;
    let pin = conversation_model_pin(pool, conversation_id).await?;
    model_configurations::resolve_primary(
        pool,
        bear_id,
        grant.hat_id(),
        pin.as_deref(),
        deployment_default,
    )
    .await
}

fn primary_source_label(source: PrimaryModelSource) -> &'static str {
    match source {
        PrimaryModelSource::ConversationPin => "conversation_explicit",
        PrimaryModelSource::HatOverride => "hat_override",
        PrimaryModelSource::BearDefault => "bear_default",
        PrimaryModelSource::DeploymentDefault => "deployment_default",
    }
}

pub async fn resolve_model_option(
    pool: &PgPool,
    handle: &str,
) -> Result<Option<ModelOption>, DenError> {
    let trimmed = handle.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Some(row) = sqlx::query_as::<_, ModelSelectionOptionRow>(
        r"
        SELECT handle, display_name, metadata_json
        FROM model_selection_options
        WHERE handle = $1
        LIMIT 1
        ",
    )
    .bind(trimmed)
    .fetch_optional(pool)
    .await?
    {
        return Ok(Some(option_from_row(row)));
    }
    Ok(den_llm::model_registry::entry_for_handle(trimmed)
        .map(|entry| entry.to_model_option_for_handle(trimmed)))
}

fn calibration_registry_key(handle: &str) -> Option<&str> {
    let trimmed = handle.trim();
    if trimmed.is_empty() || den_llm::model_registry::is_routing_wildcard_model_handle(trimmed) {
        return None;
    }
    Some(den_llm::model_registry::resolve_model_handle(trimmed).unwrap_or(trimmed))
}

/// Load the stored chars→tokens calibration for a model handle from the Den
/// model registry (ADR-0047 §7). Returns `None` when the registry has no row
/// or no observed samples yet.
pub async fn load_model_token_calibration(
    pool: &PgPool,
    handle: &str,
) -> Result<Option<ModelTokenCalibration>, DenError> {
    let Some(key) = calibration_registry_key(handle) else {
        return Ok(None);
    };
    let row = sqlx::query_as::<_, (Option<f64>, i64)>(
        r"
        SELECT calibration_tokens_per_char, calibration_sample_count
        FROM model_selection_options
        WHERE handle = $1
        LIMIT 1
        ",
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(|err| DenError::Database(format!("load model token calibration: {err}")))?;
    Ok(row.and_then(|(tokens_per_char, sample_count)| {
        tokens_per_char.map(|tokens_per_char| ModelTokenCalibration {
            tokens_per_char,
            sample_count,
        })
    }))
}

/// Fold one observed Bifrost prompt-usage sample into the stored per-model
/// calibration ratio (ADR-0047 §7). A previously unseen concrete model gets a
/// non-selectable registry row, so observed Bifrost usage is retained without
/// accidentally advertising that model in selection UI. Returns `Ok(false)`
/// only when the sample or handle is unusable.
pub async fn record_model_token_calibration_sample(
    pool: &PgPool,
    handle: &str,
    assembled_prompt_chars: u64,
    observed_prompt_tokens: u64,
) -> Result<bool, DenError> {
    let Some(observed) = den_llm::model_registry::observed_tokens_per_char(
        observed_prompt_tokens,
        assembled_prompt_chars,
    ) else {
        return Ok(false);
    };
    let Some(key) = calibration_registry_key(handle) else {
        return Ok(false);
    };
    let result = sqlx::query(
        r"
        INSERT INTO model_selection_options (
            handle,
            display_name,
            selectable,
            recommended,
            calibration_tokens_per_char,
            calibration_sample_count,
            calibration_updated_at
        )
        VALUES ($1, $1, FALSE, FALSE, $2, 1, NOW())
        ON CONFLICT (handle) DO UPDATE
        SET calibration_tokens_per_char = CASE
                WHEN model_selection_options.calibration_tokens_per_char IS NULL
                    OR NOT (
                        model_selection_options.calibration_tokens_per_char
                            > '-Infinity'::DOUBLE PRECISION
                        AND model_selection_options.calibration_tokens_per_char
                            < 'Infinity'::DOUBLE PRECISION
                    )
                THEN $2
                ELSE LEAST(
                    $4,
                    GREATEST(
                        $5,
                        model_selection_options.calibration_tokens_per_char
                            + $3 * ($2 - model_selection_options.calibration_tokens_per_char)
                    )
                )
            END,
            calibration_sample_count = CASE
                WHEN model_selection_options.calibration_sample_count = 9223372036854775807
                THEN model_selection_options.calibration_sample_count
                ELSE model_selection_options.calibration_sample_count + 1
            END,
            calibration_updated_at = NOW()
        ",
    )
    .bind(key)
    .bind(observed)
    .bind(MODEL_TOKEN_CALIBRATION_EMA_ALPHA)
    .bind(MODEL_TOKEN_CALIBRATION_MAX_RATIO)
    .bind(MODEL_TOKEN_CALIBRATION_MIN_RATIO)
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("record model token calibration sample: {err}")))?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn calibration_samples_roundtrip_through_model_registry(pool: PgPool) {
        // No samples yet: the seeded registry row has a NULL ratio.
        assert_eq!(
            load_model_token_calibration(&pool, "openai/gpt-4.1")
                .await
                .expect("load"),
            None
        );

        // First sample (alias resolves to the canonical registry key).
        let stored = record_model_token_calibration_sample(&pool, "gpt-4.1", 4_000, 1_200)
            .await
            .expect("record");
        assert!(stored);
        let calibration = load_model_token_calibration(&pool, "openai/gpt-4.1")
            .await
            .expect("load")
            .expect("calibration stored");
        assert!((calibration.tokens_per_char - 0.3).abs() < 1e-12);
        assert_eq!(calibration.sample_count, 1);

        // Second sample folds via EMA: 0.3 + 0.2 * (0.25 - 0.3) = 0.29.
        let stored = record_model_token_calibration_sample(&pool, "openai/gpt-4.1", 4_000, 1_000)
            .await
            .expect("record");
        assert!(stored);
        let calibration = load_model_token_calibration(&pool, "openai/gpt-4.1")
            .await
            .expect("load")
            .expect("calibration stored");
        assert!((calibration.tokens_per_char - 0.29).abs() < 1e-12);
        assert_eq!(calibration.sample_count, 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn calibration_sample_persists_unknown_models_and_skips_unusable_samples(pool: PgPool) {
        // Unusable sample: zero chars can never produce a ratio.
        assert!(
            !record_model_token_calibration_sample(&pool, "openai/gpt-4.1", 0, 1_000)
                .await
                .expect("record")
        );
        // An unseeded concrete model gets a non-selectable calibration row.
        assert!(record_model_token_calibration_sample(
            &pool,
            "vendor/unseeded-model",
            4_000,
            1_000
        )
        .await
        .expect("record"));
        let calibration = load_model_token_calibration(&pool, "vendor/unseeded-model")
            .await
            .expect("load")
            .expect("calibration stored");
        assert!((calibration.tokens_per_char - 0.25).abs() < 1e-12);
        assert_eq!(calibration.sample_count, 1);
    }

    #[test]
    fn simplify_model_option_for_acp_drops_context_window_details() {
        let option = ModelOption {
            handle: "openai/gpt-5".to_string(),
            label: "GPT-5 (1M ctx / 128k out)".to_string(),
            context_window: Some(1_000_000),
            max_output_tokens: Some(128_000),
        };

        let simplified = simplify_model_option_for_acp(&option);
        assert_eq!(simplified.handle, option.handle);
        assert_eq!(simplified.label, "GPT-5");
        assert_eq!(simplified.context_window, None);
        assert_eq!(simplified.max_output_tokens, None);
    }

    #[test]
    fn simplify_model_option_for_acp_drops_metadata_unknown_suffix() {
        let option = ModelOption {
            handle: "custom/model".to_string(),
            label: "Custom model — metadata unknown".to_string(),
            context_window: None,
            max_output_tokens: None,
        };

        let simplified = simplify_model_option_for_acp(&option);
        assert_eq!(simplified.label, "Custom model");
    }
}
