use super::*;

#[test]
fn blank_membership_is_unrestricted_not_private() {
    let mut audience = MembershipIntersection::default();
    audience.include(&[], &[]);
    assert!(audience.people.is_none());
    assert!(audience.bears.is_none());
}

#[test]
fn each_restricted_ancestor_intersects_both_principal_classes() {
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let mut audience = MembershipIntersection::default();
    audience.include(&[1, 2], &[first, second]);
    audience.include(&[], &[]);
    audience.include(&[2, 3], &[second]);
    assert_eq!(
        audience.people.unwrap(),
        std::iter::once(UserId::new(2)).collect()
    );
    assert_eq!(
        audience.bears.unwrap(),
        std::iter::once(BearId::new(second)).collect()
    );
}

#[test]
fn people_only_membership_excludes_bears_even_when_a_parent_names_them() {
    let bear = uuid::Uuid::new_v4();
    let mut audience = MembershipIntersection::default();
    audience.include(&[1], &[]);
    audience.include(&[1], &[bear]);
    assert!(audience.bears.unwrap().is_empty());
}
