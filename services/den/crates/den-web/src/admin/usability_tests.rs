//! Full checked-in parent rendering: no stub base, sidebar or child template.

use minijinja::{context, Value};

#[path = "html_assertions.rs"]
mod html_assertions;
pub(crate) use html_assertions::{assert_link, assert_no_link, opening_tag};

pub(crate) fn render(name: &str, values: Value) -> String {
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let defaults = context! {
            app_display_name => "Bears",
            clients => Vec::<Value>::new(),
            tokens => Vec::<Value>::new(),
            reflection_run_summary => context! {},
            reflection_run_filters => context! {},
            reflection_slo => context! {},
            entity_summary => context! {},
            reflection_watermark => context! {},
            conv => context! {},
            model_registry => context! {
                report => den_llm::model_registry::gateway_compatibility_report(Vec::<String>::new()),
            },
            theme_descriptions => crate::theme_descriptions(),
            day_of_week_names => crate::day_of_week_names(),
            template_tag => name.trim_end_matches(".html").replace('/', "-"),
            session => context! { username => "Operator", is_admin => true },
            bear => context! {
                name => "Atlas", slug => "atlas", id => "bear-id", created_at => 0,
                updated_at => 0, live_reflection_enabled => false,
            },
    };
    // Named defaults shadow spreads; use ordered spreads so the fixture wins.
    crate::template_environment(&config)
        .get_template(name)
        .expect("checked-in template exists")
        .render(context! { ..values, ..defaults })
        .unwrap_or_else(|error| panic!("render {name}: {error:#}"))
}

pub(crate) fn assert_visible(html: &str, text: &str) {
    let position = html.find(text).unwrap_or_else(|| panic!("missing {text}"));
    let mut disclosures = Vec::new();
    for fragment in html[..position].split('<').skip(1) {
        let tag = fragment.split('>').next().unwrap();
        if tag == "details" || tag.starts_with("details ") {
            disclosures.push(tag.split_whitespace().any(|attribute| attribute == "open"));
        } else if tag == "/details" {
            disclosures.pop().expect("balanced disclosures");
        }
    }
    assert!(
        disclosures.iter().all(|open| *open),
        "{text} is hidden in closed Help"
    );
}

#[test]
fn operator_pages_render_one_shared_shell_and_complete_authorized_rail() {
    for name in [
        "admin/menu.html",
        "admin/models/index.html",
        "admin/workers/index.html",
        "admin/loop_control/index.html",
        "admin/runs/index.html",
        "admin/sandbox/index.html",
        "admin/users/list.html",
        "admin/membership/list.html",
        "admin/oauth_clients/list.html",
        "admin/oauth_tokens/list.html",
    ] {
        let html = render(
            name,
            context! { usage => context! { status => "ok", has_rows => false } },
        );
        assert!(html.starts_with("<!doctype html>"), "{name}");
        assert!(html.contains("/assets/css/style.css"), "{name}");
        let viewport = opening_tag(&html, "meta", "name", "viewport");
        assert_eq!(
            viewport.attribute("content"),
            Some("width=device-width, initial-scale=1"),
            "{name}"
        );
        assert_eq!(
            html.matches("class=\"bear-manage-nav\"").count(),
            1,
            "{name}"
        );
        assert!(
            html.contains("aria-label=\"Operator navigation\""),
            "{name}"
        );
        assert!(!html.contains("aria-label=\"Den management\""), "{name}");
        for (href, label) in [
            ("/admin", "Operator home"),
            ("/admin/users/", "Users"),
            ("/admin/bears/", "Bears"),
            ("/admin/membership/", "Bear membership"),
            ("/admin/models", "Models"),
            ("/admin/workers/", "Workers"),
            ("/admin/loop-control/", "Loop control"),
            ("/admin/runs/", "Run diagnostics"),
            ("/admin/sandbox", "Sandbox images"),
            ("/admin/oauth_clients/", "OAuth clients"),
            ("/admin/oauth_tokens/", "OAuth tokens"),
            ("/cabinet", "Cabinet"),
            ("/connections", "Connections"),
            ("/reviews", "Reviews"),
            ("/work/surfaces", "Repositories"),
            ("/settings", "Settings"),
            ("/status", "Stack status"),
        ] {
            assert_link(&html, href, label);
        }
        assert!(!html.contains("&lt;nav"));
    }
    let home = render("admin/menu.html", context! {});
    assert!(home.contains("email verification and invitations"));
    assert!(home.contains("failures and persisted run evidence"));
}

#[test]
fn non_operator_render_does_not_advertise_operator_controls() {
    let html = render(
        "admin/menu.html",
        context! {
            session => context! { username => "Member", is_admin => false },
        },
    );
    for href in [
        "/admin",
        "/admin/users/",
        "/admin/models",
        "/admin/membership/",
        "/admin/oauth_tokens/",
    ] {
        assert_no_link(&html, href);
    }
    assert_link(&html, "/cabinet", "Cabinet");
    assert_eq!(html.matches("class=\"bear-manage-nav\"").count(), 1);
}

#[test]
fn sandbox_child_head_and_title_are_inserted_in_the_real_document() {
    for running in [true, false] {
        let html = render(
            "admin/sandbox/operation.html",
            context! {
                running, operation => context! { state => "running", target => "rust", kind => "build", log_tail => "BUILD EVIDENCE" },
            },
        );
        let head = html.split("</head>").next().unwrap();
        assert!(head.contains("Sandbox operation"));
        assert_eq!(head.contains("http-equiv=\"refresh\""), running);
        assert_visible(&html, "BUILD EVIDENCE");
    }
}

#[test]
fn model_usage_errors_are_conditional_and_empty_catalog_does_not_diagnose_auth() {
    for status in ["ok", "unavailable"] {
        let html = render(
            "admin/models/index.html",
            context! {
                usage => context! { status, error => "ACTUAL GATEWAY FAILURE <unsafe>", has_rows => false },
                catalog_source => "bifrost", catalog_stale => false, catalog_row_count => 0,
            },
        );
        assert!(!html.contains("check Bifrost management authentication"));
        assert_eq!(
            html.contains("ACTUAL GATEWAY FAILURE"),
            status == "unavailable"
        );
        if status == "unavailable" {
            assert_visible(&html, "ACTUAL GATEWAY FAILURE");
            assert!(html.contains("&lt;unsafe&gt;"));
        } else {
            assert!(html.contains("No usage rows returned."));
        }
    }
}

#[test]
fn pkce_form_constraints_feedback_and_values_precede_help_without_duplicates() {
    let html = render(
        "admin/oauth_clients/pkce_test.html",
        context! {
            client_id => 7, client_name => "Editor", sample_verifier => "UNUSED SAMPLE",
            form_data => context! { code_verifier => "UNIQUE VERIFIER", code_challenge => "UNIQUE CHALLENGE", code_challenge_method => "plain" },
            test_result => context! { success => false, message => "MATCH FAILED", validation_details => "ACTUAL VALIDATION EVIDENCE" },
        },
    );
    assert_visible(&html, "MATCH FAILED");
    assert_visible(&html, "ACTUAL VALIDATION EVIDENCE");
    assert_visible(&html, "Plain is less secure");
    for value in ["UNIQUE VERIFIER", "UNIQUE CHALLENGE", "MATCH FAILED"] {
        assert_eq!(html.matches(value).count(), 1);
    }
    assert!(html.contains("minlength=\"43\" maxlength=\"128\""));
    assert!(html.contains("value=\"plain\" selected"));
    assert!(html.find("Test PKCE validation").unwrap() < html.find("Help: how PKCE").unwrap());
    assert!(!html.contains("<style>"));
    assert!(!html.contains("Math.random"));
}

#[test]
fn membership_errors_preserve_selected_objects_and_roles_with_local_effects() {
    let html = render(
        "admin/membership/grant.html",
        context! {
            form => context! { user_id => 2, bear_id => "second", role => "admin" },
            users => [context! { id => 1, username => "First" }, context! { id => 2, username => "Second" }],
            bears => [context! { id => "first", name => "First Bear" }, context! { id => "second", name => "Second Bear" }],
            errors => context! { bear_id => [context! { message => "BEAR SELECTION FAILURE" }] },
        },
    );
    assert_visible(&html, "Access was not granted");
    assert_visible(&html, "BEAR SELECTION FAILURE");
    for option in [
        "value=\"2\" selected",
        "value=\"second\" selected",
        "value=\"admin\" selected",
    ] {
        assert!(html.contains(option), "{option}");
    }
    assert_visible(&html, "private memory inspection");
    let invalid = render(
        "admin/membership/grant.html",
        context! {
            form => context! { user_id => 9, bear_id => "<missing>", role => "<invalid>" },
        },
    );
    assert!(invalid.contains("Unavailable user #9"));
    assert!(invalid.contains("&lt;missing&gt;"));
    assert!(invalid.contains("&lt;invalid&gt;"));
}

#[test]
fn email_override_consequence_is_visible_and_storage_mechanics_are_in_help() {
    let html = render(
        "admin/users/edit.html",
        context! {
            id => 7,
            user => context! {
                username => "casey", display_name => "Casey", email => "casey@example.test",
                email_verified => 0, theme => "system", week_start_day => 0,
            },
        },
    );
    assert_visible(&html, "without the user completing an email check");
    let email = opening_tag(&html, "input", "name", "email");
    assert_eq!(email.attribute("type"), Some("email"));
    assert_eq!(email.attribute("value"), Some("casey@example.test"));
    assert!(opening_tag(&html, "select", "name", "email_verified").has_attribute("required"));
    assert!(
        html.find("Help: how the override").unwrap()
            < html.find("email_configs.verified_at").unwrap()
    );
    assert!(!html.contains("Weekly reports will be sent"));
}

#[test]
fn operator_run_evidence_is_available_once_and_lookup_is_escaped() {
    let html = render(
        "admin/runs/detail.html",
        context! {
            run => context! { state => "failed", terminal_reason => "ACTUAL TERMINAL REASON", run_id => "run-id" },
            events => [context! { sequence_no => 1, event_type => "tool.failed", event_json => "UNIQUE FAILURE PAYLOAD", is_failure => true }],
        },
    );
    assert_visible(&html, "ACTUAL TERMINAL REASON");
    assert_visible(&html, "UNIQUE FAILURE PAYLOAD");
    assert_eq!(html.matches("UNIQUE FAILURE PAYLOAD").count(), 1);
    let lookup = render(
        "admin/runs/index.html",
        context! { lookup_run_id => "\"><script>INJECTION</script>" },
    );
    assert!(!lookup.contains("<script>INJECTION</script>"));
    let lookup_field = opening_tag(&lookup, "input", "name", "run_id");
    assert_eq!(
        lookup_field.attribute("value"),
        Some("\"><script>INJECTION</script>")
    );
    assert_eq!(lookup_field.attribute("type"), Some("search"));
}
