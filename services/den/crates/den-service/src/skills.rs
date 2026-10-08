//! Reviewed, immutable instruction-only procedures; attachments pin canonical catalog versions.

use den_core::{
    ids::{BearId, UserId},
    DenError, RuntimeContextLabel,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct SkillId(pub Uuid);
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub id: SkillId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub content: String,
    pub content_hash: String,
    pub approved: bool,
    pub disabled: bool,
    pub attached: bool,
    pub owned: bool,
    pub profiles: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSkill {
    pub name: String,
    pub version: String,
    pub description: String,
    pub content: String,
    pub content_hash: String,
    pub profiles: Vec<RuntimeContextLabel>,
}

pub fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
fn validate(name: &str, version: &str, description: &str, content: &str) -> Result<(), DenError> {
    if name.trim().is_empty()
        || name.chars().count() > 120
        || version.trim().is_empty()
        || version.len() > 60
        || description.len() > 2000
        || content.trim().is_empty()
        || content.len() > 32000
    {
        return Err(DenError::ValidationError(
            "skill name, version and procedure required; procedure is limited to 32000 bytes"
                .into(),
        ));
    }
    Ok(())
}

pub async fn list(pool: &PgPool, bear: BearId, actor: UserId) -> Result<Vec<Skill>, DenError> {
    let role = crate::bears::db::membership_role_for_user(pool, actor.get(), bear.as_uuid())
        .await?
        .ok_or_else(|| DenError::NotFound("Bear not found".into()))?;
    let admin = crate::bears::db::role_is_bear_admin(role.as_deref());
    let rows = sqlx::query!("SELECT c.id,c.name,c.version,c.description,c.content,c.content_hash,c.status,c.owner_user_id,(m.enabled AND m.content_hash=c.content_hash) AS attached,m.applies_to_profiles AS \"applies_to_profiles?\" FROM skill_catalog_entries c LEFT JOIN bear_skills_manifest m ON m.catalog_entry_id=c.id AND m.bear_id=$1 WHERE ($2 AND (c.owner_user_id=$3 OR c.status = 'approved')) OR (c.status IN ('approved','disabled') AND m.enabled AND m.content_hash=c.content_hash) ORDER BY c.name,c.version",bear.as_uuid(),admin,actor.get()).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            if row.content_hash != hash(&row.content) {
                return Err(DenError::System("skill integrity mismatch".into()));
            }
            Ok(Skill {
                id: SkillId(row.id),
                name: row.name,
                version: row.version,
                description: row.description,
                content: row.content,
                content_hash: row.content_hash,
                approved: row.status == "approved",
                disabled: row.status == "disabled",
                attached: row.attached.unwrap_or(false),
                owned: row.owner_user_id == actor.get(),
                profiles: row.applies_to_profiles.unwrap_or_default(),
            })
        })
        .collect()
}

pub async fn create_draft(
    pool: &PgPool,
    actor: UserId,
    name: &str,
    version: &str,
    description: &str,
    content: &str,
) -> Result<SkillId, DenError> {
    validate(name, version, description, content)?;
    let checksum = hash(content);
    let id=sqlx::query_scalar!("INSERT INTO skill_catalog_entries(owner_user_id,name,version,description,content,content_hash,status) VALUES($1,$2,$3,$4,$5,$6,'draft') RETURNING id",actor.get(),name.trim(),version.trim(),description,content,checksum).fetch_one(pool).await?;
    Ok(SkillId(id))
}
pub async fn approve(
    pool: &PgPool,
    actor: UserId,
    id: SkillId,
    checksum: &str,
    confirm_public: bool,
) -> Result<(), DenError> {
    if !confirm_public {
        return Err(DenError::ValidationError(
            "acknowledge publication to the Den catalog".into(),
        ));
    }
    let changed=sqlx::query!("UPDATE skill_catalog_entries SET status='approved',reviewed_by_user_id=$2,reviewed_at=now() WHERE id=$1 AND owner_user_id=$2 AND status='draft' AND content_hash=$3",id.0,actor.get(),checksum).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound("skill draft missing or changed".into()));
    }
    Ok(())
}
pub async fn disable(pool: &PgPool, actor: UserId, id: SkillId) -> Result<(), DenError> {
    let changed = sqlx::query!(
        "UPDATE skill_catalog_entries SET status='disabled' WHERE id=$1 AND owner_user_id=$2",
        id.0,
        actor.get()
    )
    .execute(pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound("owned skill not found".into()));
    }
    Ok(())
}
pub async fn attach(
    pool: &PgPool,
    bear: BearId,
    actor: UserId,
    id: SkillId,
    checksum: &str,
    profiles: &[RuntimeContextLabel],
    confirm_work: bool,
) -> Result<(), DenError> {
    if profiles.is_empty() || (profiles.contains(&RuntimeContextLabel::JobRun) && !confirm_work) {
        return Err(DenError::ValidationError(
            "choose uses and acknowledge autonomous Work when selected".into(),
        ));
    }
    let names = profiles
        .iter()
        .map(|profile| profile.as_str().to_string())
        .collect::<Vec<_>>();
    let changed=sqlx::query!("INSERT INTO bear_skills_manifest(bear_id,skill_name,skill_version,source,content_hash,applies_to_profiles,installed_at,catalog_entry_id,enabled,attached_by_user_id) SELECT $1,c.name,c.version,'catalog',c.content_hash,$4,now(),c.id,true,$2 FROM skill_catalog_entries c WHERE c.id=$3 AND c.status='approved' AND c.content_hash=$5 AND EXISTS(SELECT 1 FROM user_bear ub WHERE ub.bear_id=$1 AND ub.user_id=$2 AND lower(btrim(ub.role))='admin') ON CONFLICT(bear_id,skill_name,skill_version) DO UPDATE SET catalog_entry_id=EXCLUDED.catalog_entry_id,content_hash=EXCLUDED.content_hash,applies_to_profiles=EXCLUDED.applies_to_profiles,enabled=true,attached_by_user_id=EXCLUDED.attached_by_user_id,updated_at=now()",bear.as_uuid(),actor.get(),id.0,&names,checksum).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound(
            "approved skill or Bear-admin grant unavailable".into(),
        ));
    }
    Ok(())
}
pub async fn detach(
    pool: &PgPool,
    bear: BearId,
    actor: UserId,
    id: SkillId,
) -> Result<(), DenError> {
    let changed=sqlx::query!("UPDATE bear_skills_manifest m SET enabled=false,updated_at=now() WHERE m.bear_id=$1 AND m.catalog_entry_id=$3 AND EXISTS(SELECT 1 FROM user_bear ub WHERE ub.bear_id=$1 AND ub.user_id=$2 AND lower(btrim(ub.role))='admin')",bear.as_uuid(),actor.get(),id.0).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::NotFound(
            "attached skill or Bear-admin grant unavailable".into(),
        ));
    }
    Ok(())
}
pub async fn effective(
    pool: &PgPool,
    bear: BearId,
    profile: RuntimeContextLabel,
) -> Result<Vec<PortableSkill>, DenError> {
    let rows=sqlx::query!("SELECT c.name,c.version,c.description,c.content,c.content_hash,m.applies_to_profiles FROM bear_skills_manifest m JOIN skill_catalog_entries c ON c.id=m.catalog_entry_id WHERE m.bear_id=$1 AND m.enabled AND c.status='approved' AND m.content_hash=c.content_hash AND $2=ANY(m.applies_to_profiles) ORDER BY c.name,c.version",bear.as_uuid(),profile.as_str()).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            if hash(&row.content) != row.content_hash {
                return Err(DenError::System("skill integrity mismatch".into()));
            }
            let profiles = row
                .applies_to_profiles
                .into_iter()
                .map(|name| name.parse().map_err(DenError::ValidationError))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PortableSkill {
                name: row.name,
                version: row.version,
                description: row.description,
                content: row.content,
                content_hash: row.content_hash,
                profiles,
            })
        })
        .collect()
}

pub fn validate_portable(items: &[PortableSkill]) -> Result<(), DenError> {
    if items.len() > 100 {
        return Err(DenError::ValidationError(
            "bundle may contain at most 100 skills".into(),
        ));
    }
    let mut keys = std::collections::BTreeSet::new();
    for item in items {
        validate(&item.name, &item.version, &item.description, &item.content)?;
        if item.content_hash != hash(&item.content)
            || item.profiles.is_empty()
            || !keys.insert((&item.name, &item.version))
        {
            return Err(DenError::ValidationError(
                "invalid skill hash, uses or duplicate version in bundle".into(),
            ));
        }
    }
    Ok(())
}
pub async fn export(pool: &PgPool, bear: BearId) -> Result<Vec<PortableSkill>, DenError> {
    let mut items = std::collections::BTreeMap::new();
    for profile in RuntimeContextLabel::ALL {
        for item in effective(pool, bear, profile).await? {
            items
                .entry((item.name.clone(), item.version.clone()))
                .or_insert(item);
        }
    }
    Ok(items.into_values().collect())
}
pub async fn stage_import(
    pool: &PgPool,
    bear: BearId,
    actor: UserId,
    items: &[PortableSkill],
) -> Result<(), DenError> {
    validate_portable(items)?;
    for item in items {
        let existing=sqlx::query_scalar!("SELECT id FROM skill_catalog_entries WHERE owner_user_id=$1 AND name=$2 AND version=$3 AND content_hash=$4",actor.get(),item.name,item.version,item.content_hash).fetch_optional(pool).await?;
        let id = match existing {
            Some(id) => SkillId(id),
            None => {
                create_draft(
                    pool,
                    actor,
                    &item.name,
                    &item.version,
                    &item.description,
                    &item.content,
                )
                .await?
            }
        };
        let names = item
            .profiles
            .iter()
            .map(|profile| profile.as_str().to_string())
            .collect::<Vec<_>>();
        let changed=sqlx::query!("INSERT INTO bear_skills_manifest(bear_id,skill_name,skill_version,source,content_hash,applies_to_profiles,catalog_entry_id,enabled,attached_by_user_id) SELECT $1,$3,$4,'imported_package',$5,$6,$7,false,$2 WHERE EXISTS(SELECT 1 FROM user_bear WHERE bear_id=$1 AND user_id=$2 AND lower(btrim(role))='admin') ON CONFLICT(bear_id,skill_name,skill_version) DO NOTHING",bear.as_uuid(),actor.get(),item.name,item.version,item.content_hash,&names,id.0).execute(pool).await?;
        if changed.rows_affected() != 1 {
            return Err(DenError::NotFound(
                "imported skill or Bear-admin grant unavailable".into(),
            ));
        }
    }
    Ok(())
}
