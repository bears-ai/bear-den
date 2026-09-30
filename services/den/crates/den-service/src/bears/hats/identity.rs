//! Compile the selected Bear-owned hat identity separately from platform-owned
//! execution-mode instructions. Only verified conversation/Job bindings may
//! supply the HatId; user text and a stance name are not authority to select it.

use std::sync::LazyLock;

use den_core::{
    ids::{BearId, HatId},
    DenError,
};
use serde_json::json;
use sqlx::PgPool;

use super::{manage::get_hat, BearHat};
use crate::bears::{
    context_composition::render_bound_base_prompt_with_registry,
    managed_blocks::{compile_and_store_managed_config_for_bear, get_compiled_bear_config},
    prompt_fragments::{
        render_compile_time_fragment, render_turn_fragment, CompileTimePromptContext,
    },
    repository_prompt_fragment_registry, repository_prompt_source_version, Bear, BearProfile,
    PromptFragmentRegistry,
};

static PROMPTS: LazyLock<Result<PromptFragmentRegistry, String>> =
    LazyLock::new(|| repository_prompt_fragment_registry().map_err(|error| error.to_string()));

fn prompts() -> Result<&'static PromptFragmentRegistry, DenError> {
    PROMPTS
        .as_ref()
        .map_err(|error| DenError::System(error.clone()))
}

fn mode_key(profile: BearProfile) -> Result<&'static str, DenError> {
    match profile {
        BearProfile::Chat => Ok("bound_chat_mode"),
        BearProfile::Pair => Ok("bound_pair_mode"),
        BearProfile::Work => Ok("bound_work_mode"),
        BearProfile::Curate | BearProfile::Watch => Err(DenError::Authorization(
            "internal roles do not inherit a conversation or IDE hat".into(),
        )),
    }
}

fn component(map: &serde_json::Value, key: &str) -> Option<String> {
    map.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// Preview the exact hat-specific component selected by a bound turn. Author
/// text is passed as a value to a repository-owned template, never parsed as
/// a template or interpreted as a tool grant.
pub fn render_hat_identity_component(bear: &Bear, hat: &BearHat) -> Result<String, DenError> {
    render_turn_fragment(
        prompts()?.require("bound_hat_identity")?,
        &json!({
            "bear_name": bear.name,
            "hat_name": hat.name,
            "identity_prompt": hat.identity_prompt,
        }),
    )
}

pub async fn bound_prompt_text(
    pool: &PgPool,
    bear: &Bear,
    profile: BearProfile,
    hat_id: HatId,
) -> Result<String, DenError> {
    let mode_key = mode_key(profile)?;
    let hat = get_hat(pool, BearId::new(bear.id), hat_id).await?;
    let registry = prompts()?;
    let (base, mode) = if bear.context_profile.is_some() {
        let cached = get_compiled_bear_config(pool, bear.id).await?;
        let compiled = match cached {
            Some(ref compiled)
                if component(&compiled.rendered_prompts_json.0, "bound_source_version")
                    == Some(repository_prompt_source_version())
                    && component(&compiled.rendered_prompts_json.0, "bound_base").is_some()
                    && component(&compiled.rendered_prompts_json.0, mode_key).is_some() =>
            {
                None
            }
            _ => Some(compile_and_store_managed_config_for_bear(pool, bear).await?),
        };
        let rendered = compiled
            .as_ref()
            .map(|compiled| &compiled.rendered_prompts)
            .or_else(|| {
                cached
                    .as_ref()
                    .map(|cached| &cached.rendered_prompts_json.0)
            })
            .ok_or_else(|| DenError::System("bound prompt compilation unavailable".into()))?;
        let base = component(rendered, "bound_base")
            .ok_or_else(|| DenError::System("compiled Bear identity missing".into()))?;
        let mode = component(rendered, mode_key)
            .ok_or_else(|| DenError::System("compiled interaction mode missing".into()))?;
        (base, mode)
    } else {
        let base = render_bound_base_prompt_with_registry(bear, None, registry)?;
        let mode = render_compile_time_fragment(
            registry.require(mode_key)?,
            &CompileTimePromptContext {
                bear_name: &bear.name,
                bear_slug: &bear.slug,
            },
        )?;
        (base, mode)
    };
    let identity = render_hat_identity_component(bear, &hat)?;
    Ok([base.as_str(), identity.as_str(), mode.as_str()]
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests;
