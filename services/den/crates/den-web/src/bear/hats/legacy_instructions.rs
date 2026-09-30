//! Bear-admin inspection of effective pre-hat interaction instructions.
//! These are reference material, not hat identity and never copied automatically.

use den_core::DenError;
use den_service::bears::{
    context_profile_from_json, managed_space_block_key, resolve_managed_blocks_for_bear, Bear,
    BearBlockBindingMode, BearProfile,
};
use serde::Serialize;
use sqlx::PgPool;

#[derive(Serialize)]
pub(super) struct PreviousInstruction {
    pub profile: &'static str,
    pub source: &'static str,
    pub content: String,
}

pub(super) async fn for_bear(
    pool: &PgPool,
    bear: &Bear,
) -> Result<Vec<PreviousInstruction>, DenError> {
    let Some(profile) = context_profile_from_json(&bear.context_profile)? else {
        return Ok(Vec::new());
    };
    let resolved = resolve_managed_blocks_for_bear(pool, bear).await?;
    let mut instructions = Vec::new();
    for role in [BearProfile::Chat, BearProfile::Pair, BearProfile::Work] {
        let key = managed_space_block_key(role);
        let block = resolved.blocks.iter().find(|block| block.key == key);
        let (content, source) = match block {
            Some(block) if block.source_mode == BearBlockBindingMode::Custom.as_str() => {
                (block.effective_content.clone(), "Bear customization")
            }
            Some(block) => (
                block.effective_content.clone(),
                "Published platform default",
            ),
            None => (
                profile.role_contracts.get(role).to_owned(),
                "Legacy profile fallback",
            ),
        };
        if !content.trim().is_empty() {
            instructions.push(PreviousInstruction {
                profile: role.as_str(),
                source,
                content,
            });
        }
    }
    Ok(instructions)
}
