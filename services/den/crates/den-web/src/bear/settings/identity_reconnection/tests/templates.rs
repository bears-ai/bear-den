use super::*;

fn render(template: &str, context: serde_json::Value) -> String {
    let mut env = minijinja::Environment::new();
    env.add_filter("urlencode", crate::filters::urlencode);
    env.add_template("bear/_manage.html", "{% block manage %}{% endblock %}")
        .unwrap();
    env.add_template(
        "bear/manage/identity_reconnection_origins.html",
        include_str!("../../../../templates/bear/manage/identity_reconnection_origins.html"),
    )
    .unwrap();
    env.add_template(
        "list",
        include_str!("../../../../templates/bear/manage/identity_reconnection.html"),
    )
    .unwrap();
    env.add_template(
        "review",
        include_str!("../../../../templates/bear/manage/identity_reconnection_review.html"),
    )
    .unwrap();
    env.add_template(
        "receipt",
        include_str!("../../../../templates/bear/manage/identity_reconnection_receipt.html"),
    )
    .unwrap();
    env.get_template(template).unwrap().render(context).unwrap()
}

fn receipt_fixture(receipt_id: Uuid) -> core::IdentityReconnectionReceipt {
    core::IdentityReconnectionReceipt {
        receipt_id,
        recorded_bear_id: "<origin-bear>".into(),
        actor_user_id: UserId::new(3),
        previous_user_id: UserId::new(42),
        prior_generation: 1,
        resulting_generation: 2,
        entity_id: "<portable-person>".into(),
        destination_user_id: Some(UserId::new(7)),
        displaced_entity_id: Some("<parked-person>".into()),
        recorded_at: "<recorded-time>".into(),
    }
}

#[test]
fn metadata_is_escaped_and_replacement_has_separate_consent_without_client_control_data() {
    let identity = json!({"entity_id": "person\"<tag>", "display_name": "Alice<script>", "active_user_id": null,
        "origins": [{"source_host": "<source>", "source_bear_id": "<bear>", "source_user_id": 42,
        "username": "<username>", "display_name": "<display>", "was_exporter": true, "recorded_at": "<time>"}]});
    let account = json!({"user_id": 7, "username": "<destination>", "display_name": "<name>"});
    let common = json!({"bear": {"slug": "example", "name": "Example"}, "identity": identity,
        "actor": account, "destination": account, "nonce": Uuid::new_v4(), "confirmation_token": Uuid::new_v4(),
        "review_id": Uuid::new_v4(), "reconnecting": true, "displaced_entity_id": "<parked>",
        "error": "<error>", "identities": [identity], "accounts": [account], "prepare_token": Uuid::new_v4(), "history": {"receipts": [], "unreadable_entries": 0}, "receipt_limit": 100});
    for template in ["list", "review"] {
        let page = render(template, common.clone());
        assert!(page.contains("&lt;source&gt;"));
        assert!(page.contains("&lt;username&gt;"));
        assert!(!page.contains("<script>"));
        assert!(!page.contains("<destination>"));
        assert!(!page.contains("name=\"expected"));
        assert!(!page.contains("name=\"actor"));
        assert!(!page.contains("name=\"binding_version\""));
        assert!(page.contains("No facts are lost or combined"));
    }
    let page = render("review", common.clone());
    assert!(page.contains("name=\"confirm_replacement\""));
    assert!(page.contains("&lt;parked&gt;"));
    assert!(page.contains("&lt;error&gt;"));
    assert!(!page.contains("name=\"entity_id\""));
    assert!(!page.contains("name=\"destination_user_id\""));
    let mut revocation = common;
    revocation["reconnecting"] = json!(false);
    revocation["destination"] = serde_json::Value::Null;
    revocation["displaced_entity_id"] = serde_json::Value::Null;
    let page = render("review", revocation);
    assert!(page.contains("Confirm revocation"));
    assert!(!page.contains("name=\"confirm_replacement\""));
    assert!(page.contains("records and relations preserved"));
}

#[test]
fn receipt_displays_the_committed_mapping_and_escapes_confirmation_labels() {
    let receipt_id = Uuid::new_v4();
    let page = render(
        "receipt",
        json!({
            "bear": {"slug": "example"}, "outcome": "Committed", "actor": {"username": "<actor>", "user_id": 3},
            "destination": {"username": "<destination>", "display_name": "<name>"},
            "receipt": receipt_fixture(receipt_id)
        }),
    );
    assert!(page.contains(&receipt_id.to_string()));
    for escaped in [
        "&lt;actor&gt;",
        "&lt;destination&gt;",
        "&lt;name&gt;",
        "&lt;portable-person&gt;",
        "&lt;parked-person&gt;",
        "&lt;recorded-time&gt;",
        "&lt;origin-bear&gt;",
    ] {
        assert!(page.contains(escaped), "missing {escaped}");
    }
    assert!(page.contains("account 7"));
    assert!(page.contains("all records and relations preserved"));
    assert!(page.contains("not a new grant or a live account-profile view"));
}

#[test]
fn receipt_history_is_escaped_informational_and_has_accurate_revocation_details() {
    let receipt_id = Uuid::new_v4();
    let page = render(
        "list",
        json!({
            "bear": {"slug": "example"}, "identities": [], "accounts": [],
            "receipt_limit": 100, "history": core::IdentityReceiptHistory { receipts: vec![receipt_fixture(receipt_id)], unreadable_entries: 2 }
        }),
    );
    assert!(page.contains(&receipt_id.to_string()));
    for escaped in [
        "&lt;origin-bear&gt;",
        "&lt;portable-person&gt;",
        "&lt;parked-person&gt;",
        "&lt;recorded-time&gt;",
    ] {
        assert!(page.contains(escaped), "missing {escaped}");
    }
    assert!(page.contains("up to 100 entries in a bounded history window"));
    assert!(page.contains(
        "current Bear decisions first, then imported history; newest first within each group."
    ));
    assert!(page
        .contains("Unreadable entries skipped in this bounded history window: <strong>2</strong>"));
    assert!(page.contains("retained in backups, not deleted"));
    assert!(page.contains("<dt>Receipt ID</dt>"));
    assert!(!page.contains("<p>Showing</p>"));
    assert!(!page.contains("<dt>Receipt</dt> ID</dt>"));
    assert!(page.contains("Imported receipts are informational historical claims"));
    assert!(page.contains("not authentication or permission grants"));
    assert!(page.contains("Recorded actor account ID</dt><dd>3"));
    assert!(page.contains("Previous recorded account ID</dt><dd>42"));
    assert!(page.contains("Destination account ID</dt><dd>7"));
    assert!(page.contains("1 → 2"));
    let mut revoked = receipt_fixture(Uuid::new_v4());
    revoked.destination_user_id = None;
    revoked.displaced_entity_id = None;
    let page = render(
        "receipt",
        json!({
            "bear": {"slug": "example"}, "outcome": "Revoked", "receipt": revoked,
            "actor": {"username": "Admin", "user_id": 3}, "destination": null
        }),
    );
    assert!(page.contains("Revoked account ID: 42"));
}

#[test]
fn unavailable_receipt_history_keeps_identity_forms_and_never_displays_a_cause() {
    let prepare_token = Uuid::new_v4();
    let page = render(
        "list",
        json!({
            "bear": {"slug": "example"},
            "identities": [{"entity_id": "parked-person", "display_name": "Alice", "active_user_id": null, "origins": []}],
            "accounts": [{"user_id": 7, "username": "destination", "display_name": "Destination"}],
            "prepare_token": prepare_token, "history": null, "receipt_limit": 100,
            "error": "private-history-error-canary"
        }),
    );
    assert!(page.contains("Receipt history unavailable"));
    assert!(page.contains("retained in backups, not deleted"));
    assert!(page.contains("Review reconnection of this person"));
    assert!(page.contains("name=\"prepare_token\""));
    assert!(page.contains(&prepare_token.to_string()));
    assert!(!page.contains("private-history-error-canary"));
    assert!(!page.contains("No readable identity decision receipts"));
    assert!(!page.contains("Unreadable entries skipped"));
}

const PRIVATE_REVIEW_CAUSE: &str = "private-review-cause-canary <imported-sensitive-data>";

pub(super) async fn unavailable_with_private_cause(
    State(state): State<AppState>,
    auth: AuthSession,
) -> Result<Response, CustomError> {
    super::super::views::validation_failure(
        &state,
        auth,
        "identityreview",
        CustomError::ValidationError(PRIVATE_REVIEW_CAUSE.into()),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn scoped_validation_response_withholds_private_cause_and_preserves_privacy_headers(
    pool: PgPool,
) {
    let fixture = fixture(pool).await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/test-identity-unavailable")
                .header(header::COOKIE, &fixture.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    let page = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(page.contains("Identity metadata or account labels"));
    assert!(page.contains("versions, expiry, or replay state"));
    assert!(page.contains("This request did not authorize a new change. Review current mappings and recent receipts before preparing another review."));
    assert!(page.contains("href=\"/bear/identityreview/portability/identities\""));
    assert!(!page.contains(PRIVATE_REVIEW_CAUSE));
    assert!(!page.contains("private-review-cause-canary"));
    assert!(!page.contains("imported-sensitive-data"));
    assert!(!page.contains("private-memory-canary-never-render"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_and_review_are_private_metadata_only_responses(pool: PgPool) {
    let fixture = fixture(pool).await;
    let location = fixture
        .prepare(&fixture.alice, Some(fixture.destination))
        .await;
    for path in [LIST, location.as_str()] {
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, &fixture.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let page = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(page.contains("Alice&lt;script&gt;"));
        assert!(!page.contains("private-memory-canary-never-render"));
        assert!(!page.contains("credential-canary-never-render"));
    }
}
