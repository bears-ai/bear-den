//! Render the checked-in templates with the real parent shell and shared macros.
use minijinja::{context, Value};

pub(super) fn render(name: &str, values: Value) -> String {
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    crate::template_environment(&config)
        .get_template(name)
        .expect("checked-in template")
        .render(context! { app_display_name => "Bears", public_web_origin => "https://den.example", ..values })
        .unwrap_or_else(|error| panic!("render {name}: {error:#}"))
}

fn escaped_html(value: &str) -> String {
    minijinja::Environment::new()
        .render_str("{{ value | e }}", context! { value })
        .unwrap()
}

fn signed_in_session() -> Value {
    context! { user_id => 1, username => "casey", is_admin => false, theme => "system" }
}

fn assert_signed_in_shell(html: &str) {
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("/assets/css/style.css"));
    assert!(html.contains("name=\"viewport\""));
    assert!(html.contains("aria-label=\"Den management\""));
    assert!(!html.contains("href=\"/admin\""));
}

fn input<'a>(html: &'a str, name: &str) -> &'a str {
    let needle = format!("name=\"{name}\"");
    let start = html.find(&needle).expect("named input");
    let start = html[..start].rfind("<input ").expect("input start");
    let end = start + html[start..].find('>').expect("input end");
    &html[start..=end]
}

pub(super) fn assert_password_inputs(html: &str, autocomplete: &str) {
    for name in ["password", "password_check"] {
        let tag = input(html, name);
        assert!(tag.contains("type=\"password\""), "{tag}");
        assert!(
            tag.contains(&format!("autocomplete=\"{autocomplete}\"")),
            "{tag}"
        );
        assert!(tag.contains("required"), "{tag}");
        assert!(tag.contains("minlength=\"8\""), "{tag}");
        assert!(
            !tag.contains("value="),
            "password input must never have a value"
        );
    }
}

#[test]
fn password_macro_masks_fields_omits_values_and_shows_plural_errors() {
    let html = render(
        "account/password.html",
        context! {
            form => context! {
                password => "PRIVATE PASSWORD", password_check => "PRIVATE CONFIRMATION",
                errors => context! { password => [context! {
                    message => "Use at least 8 characters.", params => context! { value => "PRIVATE ERROR PARAMETER" },
                }], password_check => [context! { message => "Passwords must match." }] },
            },
        },
    );
    assert_password_inputs(&html, "new-password");
    for text in [
        "Password was not changed",
        "Use at least 8 characters.",
        "Passwords must match.",
        "role=\"alert\"",
        "aria-invalid=\"true\"",
    ] {
        assert!(html.contains(text), "{text}");
    }
    for secret in [
        "PRIVATE PASSWORD",
        "PRIVATE CONFIRMATION",
        "PRIVATE ERROR PARAMETER",
    ] {
        assert!(!html.contains(secret));
    }
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("/assets/css/style.css"));
    assert!(html.contains("name=\"viewport\""));
}

#[test]
fn global_macro_errors_are_not_replaced_by_an_undefined_form_map() {
    let mut env = minijinja::Environment::new();
    env.add_template("forms.jinja", include_str!("../templates/forms.jinja"))
        .unwrap();
    env.add_template("test.html", "{% import 'forms.jinja' as forms with context %}{{ forms.input('username', 'Username', 'text', 'required') }}").unwrap();
    let html = env
        .get_template("test.html")
        .unwrap()
        .render(context! {
            errors => context! { username => [context! { message => "Enter a username." }] },
        })
        .unwrap();
    assert!(html.contains("Enter a username."));
}

#[test]
fn registration_keeps_invitation_consent_and_actual_constraints() {
    let html = render(
        "account/register.html",
        context! {
            pattern_invite => "^[a-zA-Z0-9_-]{8,128}$", pattern_username => "^[a-zA-Z0-9]+$",
            invite => context! { key => "invite_123", username => "alex", display_name => "Alex" },
            user => context! { invite_key => "invite_123", username => "casey", email => "casey@example.test", terms => "on", errors => context! { password_check => [context! { message => "Passwords must match." }] } },
        },
    );
    assert_password_inputs(&html, "new-password");
    assert!(html.contains("Invitation available:"));
    assert!(html.contains("Alex (alex)"));
    assert!(html.contains("Create account"));
    assert!(html.contains("4–30 letters and numbers only"));
    assert!(input(&html, "username").contains("pattern=\"^[a-zA-Z0-9]+$\""));
    assert!(!input(&html, "display_name").contains("required"));
    assert!(input(&html, "terms").contains("required value=\"on\" checked"));
    assert!(html.contains("I agree to the terms of service"));
    assert!(html.contains("Ask your Den administrator for the terms before agreeing"));
    assert!(html.contains("Passwords must match."));
    assert!(html.contains("value=\"casey\""));
    let empty = render(
        "account/register.html",
        context! {
            pattern_invite => "^[a-zA-Z0-9_-]{8,128}$", pattern_username => "^[a-zA-Z0-9]+$",
        },
    );
    assert!(empty.contains("You need an invitation"));
    assert!(empty.contains("Ask your Den administrator for the terms before agreeing"));
    assert!(input(&empty, "terms").contains("required value=\"on\""));
    assert_password_inputs(&empty, "new-password");
}

#[test]
fn sign_in_preserves_identity_and_shows_actionable_errors_without_password_values() {
    let html = render(
        "login.html",
        context! {
            next => "/settings", message => "Try again.",
            form_data => context! { username => "ab", password => "PRIVATE LOGIN PASSWORD", errors => context! { password => [context! { message => "Enter your password (at least 8 characters)." }] } },
        },
    );
    assert!(html.contains("Sign in"));
    assert!(input(&html, "username").contains("value=\"ab\""));
    assert!(!input(&html, "username").contains("minlength"));
    assert!(input(&html, "password").contains("autocomplete=\"current-password\""));
    assert!(!input(&html, "password").contains("value="));
    assert!(!html.contains("PRIVATE LOGIN PASSWORD"));
    assert!(html.contains("Enter your password (at least 8 characters)."));
    assert!(html.contains("role=\"alert\""));
    let empty = render("login.html", context! { next => "" });
    assert!(empty.contains("Sign in"));
    assert!(input(&empty, "password").contains("type=\"password\""));
}

#[test]
fn public_pages_show_purpose_and_recovery_without_private_diagnostics() {
    let home = render("home.html", context! {});
    assert!(home.contains("lasting purpose"));
    assert!(home.contains("href=\"/login\""));
    assert!(home.contains("href=\"/account/register\""));
    assert!(!home.contains("Rust web application starter"));
    assert!(!home.contains("RUN_WEB"));
    let error = render(
        "error.html",
        context! {
            error_name => "PRIVATE CLASS", error_message => "PRIVATE PASSWORD postgresql://credentials", error_details => "PRIVATE TRACE",
        },
    );
    assert!(error.contains("This request could not be completed."));
    assert!(error.contains("check its current state"));
    assert!(error.contains("href=\"/\""));
    assert!(!error.contains("PRIVATE"));
    assert!(!error.contains("Hans"));
    let missing = render("404.html", context! { uri => "/missing?secret=PRIVATE" });
    assert!(missing.contains("Page not found"));
    assert!(missing.contains("href=\"/\""));
    assert!(!missing.contains("PRIVATE"));
}

#[test]
fn account_empty_tokens_have_actual_editor_setup_links_and_identity() {
    let html = render(
        "account/view.html",
        context! {
            session => signed_in_session(),
            user => context! { created => 0, username => "casey", display_name => "Casey", email => "casey@example.test", email_verified => false, passhash => "PRIVATE PASSWORD HASH" },
            editor_setup_bears => [context! { slug => "atlas", name => "Atlas" }],
            account_message => "Password changed.",
            armature_tokens => Vec::<Value>::new(),
            invites => [context! { key => "invite_123", new_username => "", new_display_name => "" }],
        },
    );
    assert_signed_in_shell(&html);
    let invite_href = format!(
        "href=\"{}/account/register?invite=invite_123\"",
        escaped_html("https://den.example"),
    );
    assert!(
        html.contains(&invite_href),
        "missing invitation link: {invite_href}"
    );
    for text in [
        "casey@example.test",
        "Email not verified",
        "Password changed.",
        "role=\"status\"",
        "No editor tokens yet.",
        "href=\"/bear/atlas/code-token\"",
        "href=\"/account/password\"",
        "href=\"/settings/email/verify\"",
        "href=\"/settings/email/edit\"",
    ] {
        assert!(html.contains(text), "{text}");
    }
    assert!(!html.contains("TODO"));
    assert!(!html.contains("Code with"));
    assert!(!html.contains("PRIVATE PASSWORD HASH"));
}

#[test]
fn personal_settings_and_email_errors_keep_values_without_obsolete_promises() {
    let html = render(
        "settings/edit.html",
        context! {
            theme_descriptions => crate::theme_descriptions(), day_of_week_names => crate::day_of_week_names(),
            settings_form => context! { display_name => "Draft", theme => "dark", week_start_day => 2, errors => context! { display_name => [context! { message => "Use 3–100 characters." }] } },
        },
    );
    assert!(html.contains("value=\"Draft\""));
    assert!(html.contains("value=\"dark\" selected"));
    assert!(html.contains("value=\"2\" selected"));
    assert!(html.contains("Use 3–100 characters."));
    assert!(!html.contains("Weekly reports"));
    let email = render(
        "settings/email/edit.html",
        context! { form => context! { email => "draft@example", errors => context! { email => [context! { message => "Enter a valid email address." }] } } },
    );
    assert!(email.contains("value=\"draft@example\""));
    assert!(email.contains("Email was not changed"));
    assert!(email.contains("Enter a valid email address."));
    let view = render(
        "settings/view.html",
        context! { theme_descriptions => crate::theme_descriptions(), day_of_week_names => crate::day_of_week_names(), theme => "system", week_start_day => 1 },
    );
    assert!(!view.contains("TODO"));
    assert!(view.contains("Not verified"));
}

#[test]
fn verification_states_offer_supported_send_and_change_email_recovery() {
    let verify = render(
        "settings/email/verify.html",
        context! { email_address => "casey@example.test", token => "send-token" },
    );
    assert!(verify.contains("casey@example.test"));
    assert!(verify.contains("Send verification email"));
    assert!(verify.contains("href=\"/settings/email/edit\""));
    let verified = render(
        "settings/email/verify.html",
        context! { email_address => "casey@example.test", email_verified => true },
    );
    assert!(verified.contains("is verified"));
    assert!(!verified.contains("name=\"token\""));
    for state in ["expired", "invalid"] {
        let result = render(
            "settings/email/verify_result.html",
            context! { verify_message_key => state },
        );
        assert!(result.contains("role=\"alert\""));
        assert!(result.contains("href=\"/settings/email/verify\""));
        assert!(result.contains("href=\"/settings/email/edit\""));
        assert!(!result.contains("Continue to Dashboard"));
    }
    let sent = render(
        "settings/email/verify_sent.html",
        context! { email_sent_to => "casey@example.test" },
    );
    assert!(sent.contains("Verification email sent"));
    assert!(sent.contains("href=\"/login\""));
    assert!(sent.contains("Send another verification email"));
    assert!(sent.contains("Change email"));
}

#[test]
fn first_setup_prioritizes_purpose_and_model_then_hat_without_executing_a_task() {
    let form = crate::onboarding::FirstBearForm::default();
    let expected_steering = escaped_html(&form.user_steering);
    let html = render(
        "onboarding/first_bear.html",
        context! {
            session => signed_in_session(),
            form => &form, model_catalog_configured => true,
            model_options => [context! { handle => "test/model", label => "Test model" }],
        },
    );
    assert_signed_in_shell(&html);
    assert!(html.contains(">Handle</label>"));
    assert!(
        html.contains(&expected_steering),
        "default steering must remain intact"
    );
    assert!(html.contains("name=\"slug\""));
    assert!(html.contains(">Purpose</label>"));
    assert!(
        html.find("default_model_field").unwrap()
            < html.find("<summary>Optional steering").unwrap()
    );
    assert!(html.contains("<details>"));
    assert!(html.contains("does not automatically execute this task"));
    assert!(html.contains("After creation, select or create a hat."));
    for stale in [
        "For this MVP",
        "protected role contracts",
        "saved in the Bear context profile",
        ">Slug</label>",
    ] {
        assert!(!html.contains(stale), "obsolete setup copy: {stale}");
    }
    let invalid = render(
        "onboarding/first_bear.html",
        context! {
            session => signed_in_session(),
            form => &form,
            errors => context! { bear_context => [context! { message => "Context is too long." }] },
        },
    );
    assert_signed_in_shell(&invalid);
    assert!(invalid.contains("<details open>"));
    assert!(invalid.contains("role=\"alert\""));
    assert!(invalid.contains("Context is too long."));
    assert!(invalid.contains("Your Bear was not created"));
    let saved = render(
        "onboarding/first_bear.html",
        context! {
            session => signed_in_session(),
            form => &form, runtime_sync_error => "Your Bear was created, but runtime setup could not finish.",
        },
    );
    assert_signed_in_shell(&saved);
    assert!(saved.contains("Your Bear was created, but runtime setup could not finish."));
    assert!(saved.contains("role=\"alert\""));
    assert!(saved.contains("href=\"/bear/builder-bear/models\""));
    assert!(saved.contains("href=\"/bear/builder-bear/hats\""));
    assert!(!saved.contains("<button type=\"submit\">Create my first Bear"));
}
