use super::*;

#[test]
fn deletion_preview_blocks_only_sole_admin_memberships_and_serializes_typed_ids() {
    let mut preview = UserDeletionPreview {
        user_id: UserId::new(7),
        username: "target".into(),
        has_active_private_copies: false,
        bears: vec![UserDeletionBear {
            bear_id: BearId::new(uuid::Uuid::nil()),
            name: "Atlas".into(),
            is_admin: true,
            last_admin: false,
        }],
    };
    assert!(!preview.is_blocked());
    preview.bears[0].last_admin = true;
    assert!(preview.is_blocked());
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(json["user_id"], 7);
    assert_eq!(json["bears"][0]["bear_id"], uuid::Uuid::nil().to_string());
    assert!(matches!(
        DenError::from(UserDeletionError::LastBearAdmin(preview)),
        DenError::ValidationError(_)
    ));
}

#[test]
fn active_private_copy_blocker_is_typed_generic_and_independent_of_admin_memberships() {
    let preview = UserDeletionPreview {
        user_id: UserId::new(7),
        username: "creator".into(),
        bears: vec![],
        has_active_private_copies: true,
    };
    assert!(preview.is_blocked());
    let encoded = serde_json::to_string(&preview).unwrap();
    assert!(!encoded.contains("artifact_"));
    assert!(!encoded.contains("snapshot_count"));
    let error = DenError::from(UserDeletionError::ActivePrivateCopies(preview));
    assert!(matches!(error, DenError::ValidationError(_)));
    assert!(error.to_string().contains("creator"));
}

#[test]
fn compatibility_errors_do_not_expose_constraint_details_or_reclassify_other_database_errors() {
    let error = DenError::from(UserDeletionError::Referenced {
        constraint: Some("private_constraint_name".into()),
    });
    assert!(matches!(error, DenError::ValidationError(_)));
    assert!(!error.to_string().contains("private_constraint_name"));
    assert!(matches!(
        delete_error(sqlx::Error::RowNotFound),
        UserDeletionError::Database(sqlx::Error::RowNotFound)
    ));
    assert!(matches!(
        DenError::from(UserDeletionError::NotFound),
        DenError::NotFound(_)
    ));
}
