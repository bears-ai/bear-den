use super::super::session_state::{now, UNAVAILABLE};
use super::*;

#[test]
fn random_tokens_and_scope_require_actor_bear_session_and_fresh_age() {
    let first = ActionNonce::new();
    let second = ActionNonce::new();
    assert_ne!(first, second);
    assert_eq!(first.0.get_version(), Some(uuid::Version::Random));
    let bear = BearId::new(Uuid::new_v4());
    let actor = UserId::new(7);
    let scope = Scope::new(bear, actor, "session-one".into());
    let at = now();
    assert!(scope.authorize(bear, actor, "session-one", at));
    assert!(!scope.authorize(BearId::new(Uuid::new_v4()), actor, "session-one", at));
    assert!(!scope.authorize(bear, UserId::new(8), "session-one", at));
    assert!(!scope.authorize(bear, actor, "session-two", at));
    assert!(!scope.authorize(bear, actor, "session-one", at + 900));
    assert!(!scope.authorize(bear, actor, "session-one", at - 5));
    assert!(!scope.authorize(bear, actor, "session-one", i64::MIN));
}

#[test]
fn forms_accept_only_tokens_names_and_explicit_acknowledgement() {
    let token = ActionNonce::new();
    let form = |acknowledged| ActionForm {
        token,
        name: None,
        acknowledged,
    };
    assert!(Intent::Capture(true).action(form(false)).is_err());
    assert!(matches!(
        Intent::Capture(true).action(form(true)).unwrap(),
        OwnerMemoryAction::SetCapture(true)
    ));
    let fact = personal_memory::PersonalFactId(Uuid::new_v4());
    assert!(Intent::Retract {
        expected_head: fact
    }
    .action(form(false))
    .is_err());
    assert!(
        matches!(Intent::Retract { expected_head: fact }.action(form(true)).unwrap(), OwnerMemoryAction::Retract { expected_head } if expected_head == fact)
    );
    let candidate_id = personal_memory::PersonalCandidateId(Uuid::new_v4());
    assert!(Intent::Publish {
        candidate_id,
        expected_head: Some(fact)
    }
    .action(form(false))
    .is_err());
    let request_id = Uuid::new_v4();
    assert!(Intent::ErasePreferredNames { request_id }
        .action(form(false))
        .is_err());
    for _ in 0..2 {
        assert!(matches!(
            Intent::ErasePreferredNames { request_id }.action(form(true)).unwrap(),
            OwnerMemoryAction::ErasePreferredNames { request_id: actual } if actual == request_id
        ));
    }
    let forged = serde_json::json!({"token": token.0, "actor": 8, "expected_head": fact.0});
    assert!(serde_json::from_value::<ActionForm>(forged).is_err());
    assert!(Intent::Capture(true)
        .action(ActionForm {
            token,
            name: Some("not a capture field".into()),
            acknowledged: true
        })
        .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn registry_is_bounded_scoped_and_preserves_request_ids_without_consuming_retries(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    let registry = Registry::default();
    let token = ActionNonce::new();
    let request_id = Uuid::new_v4();
    let erase_token = ActionNonce::new();
    let erase_request_id = Uuid::new_v4();
    registry
        .register(
            Scope::new(f.bear_id, f.owner, "session-one".into()),
            f.inspect().await.owner,
            vec![
                (token, Intent::Request { request_id }),
                (
                    erase_token,
                    Intent::ErasePreferredNames {
                        request_id: erase_request_id,
                    },
                ),
            ],
        )
        .unwrap();
    let authorize = |operation, bear, actor, session: &str| {
        registry.authorize(token, operation, bear, actor, session)
    };
    for denied in [
        authorize(Operation::Capture, f.bear_id, f.owner, "session-one"),
        authorize(Operation::Request, f.bear_id, f.other, "session-one"),
        authorize(
            Operation::Request,
            BearId::new(Uuid::new_v4()),
            f.owner,
            "session-one",
        ),
        authorize(Operation::Request, f.bear_id, f.owner, "session-two"),
        registry.authorize(
            ActionNonce::new(),
            Operation::Request,
            f.bear_id,
            f.owner,
            "session-one",
        ),
    ] {
        assert!(matches!(denied, Err(UNAVAILABLE)));
    }
    let one = authorize(Operation::Request, f.bear_id, f.owner, "session-one").unwrap();
    let two = authorize(Operation::Request, f.bear_id, f.owner, "session-one").unwrap();
    assert!(Arc::ptr_eq(&one.owner, &two.owner));
    for pending in [one, two] {
        let action = pending
            .intent
            .action(ActionForm {
                token,
                name: Some("Retry name".into()),
                acknowledged: false,
            })
            .unwrap();
        assert!(
            matches!(action, OwnerMemoryAction::RequestPreferredName { request_id: actual, .. } if actual == request_id)
        );
    }
    assert!(registry
        .authorize(
            erase_token,
            Operation::Request,
            f.bear_id,
            f.owner,
            "session-one"
        )
        .is_err());
    for _ in 0..2 {
        let pending = registry
            .authorize(
                erase_token,
                Operation::Erase,
                f.bear_id,
                f.owner,
                "session-one",
            )
            .unwrap();
        assert!(
            matches!(pending.intent, Intent::ErasePreferredNames { request_id: actual } if actual == erase_request_id)
        );
    }
    let actions = (0..513)
        .map(|_| (ActionNonce::new(), Intent::Capture(true)))
        .collect();
    assert!(registry
        .register(
            Scope::new(f.bear_id, f.owner, "session-one".into()),
            f.inspect().await.owner,
            actions
        )
        .is_err());
    assert!(
        authorize(Operation::Request, f.bear_id, f.owner, "session-one").is_ok(),
        "capacity pressure cannot replace an existing retry's request ID"
    );
}
