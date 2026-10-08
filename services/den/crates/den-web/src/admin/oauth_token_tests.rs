use super::*;
use crate::admin::usability_tests::{assert_visible, render};
use den_oauth::oauth::OAuthScope;
use minijinja::context;
use serde_json::json;

fn record() -> AccessTokenWithContext {
    AccessTokenWithContext {
        token_id: 7,
        token: "PRIVATE TOKEN EVIDENCE".into(),
        client_id: 1,
        user_id: 2,
        scopes: json!([OAuthScope::all()[0].as_str()]),
        expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
        revoked: false,
        token_created_at: OffsetDateTime::UNIX_EPOCH,
        client_identifier: "client".into(),
        client_name: "Editor".into(),
        username: "Casey".into(),
        email: "casey@example.test".into(),
        display_name: "Casey".into(),
    }
}

#[test]
fn populated_token_views_render_scopes_status_and_revocation_without_method_calls() {
    let scope = OAuthScope::all()[0].as_str();
    let active = TokenView::at(record(), OffsetDateTime::UNIX_EPOCH);
    let html = render(
        "admin/oauth_tokens/view.html",
        context! { token => &active },
    );
    for text in [
        scope,
        "Active",
        "PRIVATE TOKEN EVIDENCE",
        "Keep this credential private",
        "does not undo prior actions",
        "Revoke token",
    ] {
        assert_visible(&html, text);
    }
    let list = render(
        "admin/oauth_tokens/list.html",
        context! { tokens => [&active] },
    );
    assert_visible(&list, scope);
    assert_visible(&list, "Active");
    assert!(!list.contains("PRIVATE TOKEN EVIDENCE"));
    let expired = TokenView::at(
        record(),
        OffsetDateTime::UNIX_EPOCH + time::Duration::hours(2),
    );
    assert!(matches!(expired.state, TokenState::Expired));
    assert!(!expired.can_revoke);
    let mut revoked = record();
    revoked.revoked = true;
    let revoked = TokenView::at(revoked, OffsetDateTime::UNIX_EPOCH);
    assert!(matches!(revoked.state, TokenState::Revoked));
    assert!(!revoked.can_revoke);
}

#[test]
fn malformed_scopes_are_a_visible_inspection_failure_not_an_empty_success() {
    let mut record = record();
    record.scopes = json!(42);
    let token = TokenView::at(record, OffsetDateTime::UNIX_EPOCH);
    assert!(token.scope_error.is_some());
    let html = render("admin/oauth_tokens/view.html", context! { token });
    assert_visible(&html, "Could not parse scopes");
    assert_visible(&html, "42");
}

#[test]
fn generation_preserves_selection_and_exposes_the_actual_credential_once() {
    let token = TokenView::at(record(), OffsetDateTime::UNIX_EPOCH);
    let html = render(
        "admin/oauth_tokens/generate.html",
        context! {
            clients => [context! { id => 1, name => "Editor", client_id => "client" }],
            users => [TokenUserOption::from((2, "casey".into(), "Casey".into(), "casey@example.test".into()))],
            form_data => context! { client_id => "1", user_id => "2", scopes => [OAuthScope::all()[0].as_str()], expires_in => 24 },
            available_scopes => [OAuthScope::all()[0].as_str()], generated_token => token,
        },
    );
    assert!(html.contains("value=\"1\" selected"));
    assert!(html.contains("value=\"2\" selected"));
    assert!(html.contains("value=\"24\""));
    assert_visible(&html, "PRIVATE TOKEN EVIDENCE");
    assert_eq!(html.matches("PRIVATE TOKEN EVIDENCE").count(), 1);
    assert!(html.contains("Inspect or revoke this token"));
}
