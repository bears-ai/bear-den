use super::*;
use crate::{
    bears::db::{create_bear, BearParams},
    conversation::persistence::ensure_conversation_for_external_id,
};

fn context(bear_id: Uuid, user_id: Option<i32>, profile: BearProfile) -> ArtifactAccessContext {
    ArtifactAccessContext {
        bear_id,
        user_id,
        profile,
    }
}

async fn finalized(
    pool: &PgPool,
    bear_id: Uuid,
    creator: Option<i32>,
    visibility: ArtifactVisibility,
) -> ArtifactMetadata {
    let artifact = reserve_artifact(
        pool,
        ReserveArtifactInput {
            bear_id,
            created_by_user_id: creator,
            owner_profile: BearProfile::Pair,
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

#[sqlx::test(migrations = "../../migrations")]
async fn human_artifact_access_never_inherits_another_persons_profile(pool: PgPool) {
    let bear_id = create_bear(
        &pool,
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
    .fetch_one(&pool)
    .await
    .unwrap();
    let other = sqlx::query_scalar!(
        "INSERT INTO users (username, email) VALUES ($1, $1) RETURNING id",
        "artifactother"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let conversation_id = "den-conv-artifact-profile";
    ensure_conversation_for_external_id(&pool, bear_id, Some(owner), conversation_id, None, None)
        .await
        .unwrap();
    for visibility in [
        ArtifactVisibility::PrivateToProfile,
        ArtifactVisibility::SameUser,
        ArtifactVisibility::HandoffRequested,
    ] {
        let artifact = finalized(&pool, bear_id, Some(owner), visibility).await;
        let foreign = context(bear_id, Some(other), BearProfile::Pair);
        assert!(
            matches!(
                authorize_artifact_access(
                    &pool,
                    &artifact.artifact_ref,
                    foreign.clone(),
                    ArtifactAccessLevel::Content,
                )
                .await,
                Err(DenError::Authorization(_)),
            ),
            "{visibility:?} must not become readable to another human with the same profile"
        );
        assert!(
            matches!(
                authorize_artifact_access(
                    &pool,
                    &artifact.artifact_ref,
                    context(bear_id, Some(other), BearProfile::Curate),
                    ArtifactAccessLevel::Metadata
                )
                .await,
                Err(DenError::Authorization(_)),
            ),
            "a human must not claim the internal curator by profile label"
        );
        assert!(
            matches!(
                authorize_artifact_access(
                    &pool,
                    &artifact.artifact_ref,
                    context(bear_id, None, BearProfile::Pair),
                    ArtifactAccessLevel::Metadata
                )
                .await,
                Err(DenError::Authorization(_)),
            ),
            "an ownerless Pair context must not inherit a human's artifact"
        );
        authorize_artifact_access(
            &pool,
            &artifact.artifact_ref,
            context(bear_id, Some(owner), BearProfile::Pair),
            ArtifactAccessLevel::Content,
        )
        .await
        .unwrap();
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
        let citations =
            list_conversation_artifact_citations(&pool, bear_id, conversation_id, foreign)
                .await
                .unwrap();
        let citation = citations
            .iter()
            .find(|c| c.artifact_ref == artifact.artifact_ref)
            .unwrap();
        assert_eq!(citation.kind, "unavailable");
        assert!(citation.summary.is_none());
        if visibility == ArtifactVisibility::PrivateToProfile {
            assert!(matches!(
                authorize_artifact_access(
                    &pool,
                    &artifact.artifact_ref,
                    context(bear_id, Some(owner), BearProfile::Chat),
                    ArtifactAccessLevel::Metadata
                )
                .await,
                Err(DenError::Authorization(_)),
            ));
        }
    }
    let internal = finalized(&pool, bear_id, None, ArtifactVisibility::PrivateToProfile).await;
    authorize_artifact_access(
        &pool,
        &internal.artifact_ref,
        context(bear_id, None, BearProfile::Pair),
        ArtifactAccessLevel::Metadata,
    )
    .await
    .unwrap();
    assert!(authorize_artifact_access(
        &pool,
        &internal.artifact_ref,
        context(bear_id, Some(owner), BearProfile::Pair),
        ArtifactAccessLevel::Metadata,
    )
    .await
    .is_err());
}
