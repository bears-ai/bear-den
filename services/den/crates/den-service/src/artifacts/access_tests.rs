use super::*;
use crate::{
    bears::db::{create_bear, BearParams},
    conversation::persistence::ensure_conversation_for_external_id,
};

const VISIBILITIES: [ArtifactVisibility; 4] = [
    ArtifactVisibility::PrivateToProfile,
    ArtifactVisibility::SameUser,
    ArtifactVisibility::HandoffRequested,
    ArtifactVisibility::BearVisible,
];
const OWNER_PROFILES: [BearProfile; 4] = [
    BearProfile::Pair,
    BearProfile::Chat,
    BearProfile::Work,
    BearProfile::Curate,
];

fn context(bear_id: Uuid, user_id: Option<i32>) -> ArtifactAccessContext {
    ArtifactAccessContext { bear_id, user_id }
}

async fn setup(pool: &PgPool) -> (Uuid, i32, i32) {
    let bear_id = create_bear(
        pool,
        BearParams {
            slug: "artifact-profile-access",
            name: "Artifact profile access",
            description: "test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let owner = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
        "artifactowner"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let other = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
        "artifactother"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (bear_id, owner, other)
}

async fn finalized(
    pool: &PgPool,
    bear_id: Uuid,
    creator: Option<i32>,
    owner_profile: BearProfile,
    visibility: ArtifactVisibility,
) -> ArtifactMetadata {
    let artifact = reserve_artifact(
        pool,
        ReserveArtifactInput {
            bear_id,
            created_by_user_id: creator,
            owner_profile,
            kind: "tool_output".into(),
            title: Some("private tool output".into()),
            summary: Some("private output summary".into()),
            content_type: Some("text/plain".into()),
            storage_kind: ArtifactStorageKind::DbText,
            visibility,
            provenance: serde_json::json!({}),
            metadata: serde_json::json!({}),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    finalize_metadata_only_artifact(
        pool,
        FinalizeArtifactInput {
            artifact_ref: artifact.artifact_ref,
            bear_id,
            storage_key: None,
            content_bytes: Some(25),
            content_sha256: None,
            metadata: serde_json::json!({}),
        },
    )
    .await
    .unwrap()
}

fn assert_redacted(citation: &ArtifactCitation, artifact: &ArtifactMetadata) {
    assert_eq!(citation.artifact_ref, artifact.artifact_ref);
    assert_eq!(citation.kind, "unavailable");
    assert_eq!(citation.lifecycle, artifact.lifecycle);
    assert!(!citation.readable);
    assert!(citation.title.is_none());
    assert!(citation.summary.is_none());
    assert!(citation.content_type.is_none());
    assert!(citation.content_bytes.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn authenticated_creator_access_is_independent_of_profile_and_channel(pool: PgPool) {
    let (bear_id, owner, other) = setup(&pool).await;
    for owner_profile in OWNER_PROFILES {
        for visibility in VISIBILITIES {
            let artifact = finalized(&pool, bear_id, Some(owner), owner_profile, visibility).await;
            // Browser and BearWire callers use the same authenticated human identity.
            let browser = context(bear_id, Some(owner));
            let bearwire = context(bear_id, Some(owner));
            for actor in [browser, bearwire] {
                for level in [ArtifactAccessLevel::Metadata, ArtifactAccessLevel::Content] {
                    authorize_artifact_access(&pool, &artifact.artifact_ref, actor.clone(), level)
                        .await
                        .unwrap();
                }
                let citation = citation_from_artifact(&artifact, &actor);
                assert!(citation.readable);
                assert_eq!(citation.title, artifact.title);
                assert_eq!(citation.summary, artifact.summary);
            }
            for level in [ArtifactAccessLevel::Metadata, ArtifactAccessLevel::Content] {
                let result = authorize_artifact_access(
                    &pool,
                    &artifact.artifact_ref,
                    context(bear_id, Some(other)),
                    level,
                )
                .await;
                if visibility == ArtifactVisibility::BearVisible {
                    result.unwrap();
                } else {
                    assert!(matches!(result, Err(DenError::Authorization(_))));
                }
            }
            if visibility != ArtifactVisibility::BearVisible {
                assert_redacted(
                    &citation_from_artifact(&artifact, &context(bear_id, Some(other))),
                    &artifact,
                );
            }
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn ownerless_context_has_no_generic_access_or_internal_profile_privilege(pool: PgPool) {
    let (bear_id, owner, _) = setup(&pool).await;
    for creator in [Some(owner), None] {
        for owner_profile in OWNER_PROFILES {
            for visibility in VISIBILITIES {
                let artifact = finalized(&pool, bear_id, creator, owner_profile, visibility).await;
                for level in [ArtifactAccessLevel::Metadata, ArtifactAccessLevel::Content] {
                    assert!(matches!(
                        authorize_artifact_access(
                            &pool,
                            &artifact.artifact_ref,
                            context(bear_id, None),
                            level,
                        )
                        .await,
                        Err(DenError::Authorization(_)),
                    ));
                }
                assert_redacted(
                    &citation_from_artifact(&artifact, &context(bear_id, None)),
                    &artifact,
                );
                if creator.is_none() && visibility != ArtifactVisibility::BearVisible {
                    assert!(matches!(
                        authorize_artifact_access(
                            &pool,
                            &artifact.artifact_ref,
                            context(bear_id, Some(owner)),
                            ArtifactAccessLevel::Metadata,
                        )
                        .await,
                        Err(DenError::Authorization(_)),
                    ));
                }
            }
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_citations_redact_unauthorized_actors(pool: PgPool) {
    let (bear_id, owner, other) = setup(&pool).await;
    let conversation_id = "den-conv-artifact-profile";
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), conversation_id, None, None)
        .await
        .unwrap();
    for visibility in VISIBILITIES {
        let artifact = finalized(&pool, bear_id, Some(owner), BearProfile::Pair, visibility).await;
        attach_conversation_artifact(
            &pool,
            AttachConversationArtifactInput {
                artifact_ref: artifact.artifact_ref.clone(),
                bear_id,
                conversation_id: conversation_id.into(),
                role: "output".into(),
                metadata: serde_json::json!({}),
                created_by_user_id: Some(owner),
            },
        )
        .await
        .unwrap();
        for user_id in [None, Some(other), Some(owner)] {
            let citations = list_conversation_artifact_citations(
                &pool,
                bear_id,
                conversation_id,
                context(bear_id, user_id),
            )
            .await
            .unwrap();
            let citation = citations
                .iter()
                .find(|c| c.artifact_ref == artifact.artifact_ref)
                .unwrap();
            if user_id == Some(owner)
                || (user_id.is_some() && visibility == ArtifactVisibility::BearVisible)
            {
                assert!(citation.readable);
                assert_eq!(citation.kind, artifact.kind);
                assert_eq!(citation.title, artifact.title);
                assert_eq!(citation.summary, artifact.summary);
                assert_eq!(citation.content_type, artifact.content_type);
                assert_eq!(citation.content_bytes, artifact.content_bytes);
            } else {
                assert_redacted(citation, &artifact);
            }
        }
    }
}
