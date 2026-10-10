//! Render the checked-in parent chain, assets, navigation, and controls together.
use minijinja::{context, Value};
use serde_json::json;

fn render(template: &str, data: Value) -> String {
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let env = crate::template_environment(&config);
    let page = env
        .get_template(template)
        .unwrap()
        .render(context! {
            app_display_name => "BEARS",
            session => json!({"username": "tester", "is_admin": false}),
            bear => json!({"id": "bear", "slug": "bear", "name": "Test Bear", "description": "Purpose", "system_prompt": "Stored steering", "default_model": "obsolete/model"}),
            ..data
        })
        .unwrap();
    assert!(page.contains("<!doctype html>"));
    assert!(page.contains("name=\"viewport\""));
    assert!(page.contains("/assets/css/style.css"));
    assert!(page.contains("aria-label=\"Den\""));
    assert!(page.contains("bear-manage"));
    page
}

fn escaped_html(value: &str) -> String {
    minijinja::Environment::new()
        .render_str("{{ value | e }}", context! { value })
        .unwrap()
}

fn effective() -> serde_json::Value {
    json!({"name": "Focused", "model_handle": "catalog/model", "effort_label": "High", "source_label": "Bear default", "status": "unavailable", "status_detail": "Selected model is unavailable <retry>"})
}

fn main_content(page: &str) -> &str {
    page.split_once("<main id=\"bear-content\">")
        .unwrap()
        .1
        .split_once("</main>")
        .unwrap()
        .0
}

fn assert_visible(page: &str, text: &str) {
    let prefix = page
        .split_once(text)
        .unwrap_or_else(|| panic!("missing {text}"))
        .0;
    assert_eq!(
        prefix.matches("<details").count(),
        prefix.matches("</details>").count(),
        "{text} is inside a disclosure"
    );
}

#[test]
fn purpose_uses_resolved_configuration_and_member_readable_models() {
    for can_manage_bear in [false, true] {
        let page = render(
            "bear/manage/identity.html",
            context! {
                can_manage_bear, effective_model => effective(), hats => json!([]),
            },
        );
        let main = main_content(&page);
        for text in [
            "Focused",
            "High",
            "Bear default",
            "Selected model is unavailable &lt;retry&gt;",
            ">Models</a>",
        ] {
            assert_visible(main, text);
        }
        assert_visible(main, &escaped_html("catalog/model"));
        assert!(!main.contains(&escaped_html("obsolete/model")));
        assert_eq!(
            main.contains("Edit name &amp; description"),
            can_manage_bear
        );
        assert_eq!(main.contains("Stored steering"), can_manage_bear);
    }
}

#[test]
fn models_lead_with_configuration_and_usage_not_disabled_member_forms() {
    let disabled_control =
        regex::Regex::new(r"<(?:form|input|button|select)\b[^>]*\bdisabled").unwrap();
    for can_manage_bear in [false, true] {
        let page = render(
            "bear/settings/models.html",
            context! {
                can_manage_bear, effective_model => effective(),
                configurations => json!([{"configuration": {"id": "config", "name": "Focused", "model_handle": "catalog/model"}, "fields": {"name": "Preserved <draft>", "model_handle": "catalog/new", "thinking_effort": "medium"}, "status": "unavailable", "status_detail": "Unavailable configuration", "effort_label": "High"}]),
                default_selection => "config", new_configuration => json!({"name": "", "model_handle": "", "thinking_effort": ""}),
                bear_loop_control => "careful", bear_tool_budget_multiplier => "2",
                stored_bear_loop_control => "careful", stored_bear_tool_budget_multiplier => "2",
                field_errors => None::<Value>, bifrost_virtual_key_clear => false,
                bifrost_virtual_key_configured => true,
                bifrost_usage => json!({"status": "unavailable", "error": "Usage service cannot be reached"}),
            },
        );
        let main = main_content(&page);
        assert_visible(main, &escaped_html("catalog/model"));
        for text in [
            "reasoning High",
            "Unavailable configuration",
            "Usage service cannot be reached",
            "Saved loop control: <strong>careful</strong>",
            "saved tool budget multiplier: <strong>2</strong>",
        ] {
            assert_visible(main, text);
        }
        assert!(main.contains("<h2>Models</h2>"));
        assert!(main.contains("<h3>Bear default configuration</h3>"));
        assert!(!main.contains("</h2></h2>"));
        assert!(!main.contains("<h3></h3>"));
        assert!(!disabled_control.is_match(main));
        assert_eq!(
            main.contains("value=\"config\" selected disabled"),
            can_manage_bear
        );
        assert_eq!(main.contains("<form"), can_manage_bear);
        assert_eq!(main.contains("Preserved &lt;draft&gt;"), can_manage_bear);
        if can_manage_bear {
            assert!(main.contains("value=\"config\" selected"));
            assert!(main.contains("value=\"medium\" selected"));
            assert!(
                main.find("Usage &amp; gateway status").unwrap()
                    < main.find("Advanced settings:").unwrap()
            );
            assert!(main.contains("Leave blank to keep the stored secret"));
            assert!(main.contains("does not revoke the key at Bifrost"));
            assert!(main.contains("starts usage and budget metrics fresh"));
        }
    }
    let healthy = render(
        "bear/settings/models.html",
        context! {
            can_manage_bear => false, effective_model => effective(), configurations => json!([]),
            bifrost_usage => json!({"status": "ok", "virtual_key_name": "Bear key", "is_active": "true", "has_budgets": true, "has_model_usage": true, "budget_rows": [{"scope": "Monthly", "current_usage": "12", "max_limit": "100", "remaining": "88", "reset_duration": "Monthly"}], "model_usage_rows": [{"model": "catalog/model", "provider": "Provider", "total_requests": "3", "total_tokens": "900", "total_cost": "$0.12"}]}),
        },
    );
    assert_visible(main_content(&healthy), "$0.12");
    assert_visible(main_content(&healthy), "Usage limits");
}

#[test]
fn hat_blockers_and_privacy_cost_consequences_are_visible() {
    for choices in [
        json!([]),
        json!([{"id": "repo", "name": "Repository", "selected": false}]),
    ] {
        let needs_assignment = choices.as_array().unwrap().is_empty();
        let page = render(
            "bear/manage/hat.jinja",
            context! {
                can_manage_bear => true, hat => json!({"id": "hat", "name": "Engineering", "purpose": "Review", "identity_prompt": "Identity", "work_enabled": false, "auto_curate_enabled": false}),
                effective_model => effective(), configurations => json!([]), grant_count => 0, historical_hat_records => 3, choices,
                web_grants => json!({"fetch_tool_grant_id": null, "search_tool_grant_id": null, "hosts": []}),
            },
        );
        let main = main_content(&page);
        for text in [
            "Work is blocked: no repositories are permitted",
            "What this hat can use",
            "api.search.brave.com",
            "privacy, network, and usage costs",
            "Existing relays may remain reachable",
            "cannot guarantee removal",
            "confirm_audience",
            "Review 3 historical hat records",
        ] {
            assert_visible(main, text);
        }
        assert!(main.contains("#hat-resources"));
        if needs_assignment {
            assert_visible(main, "Assign a repository to this Bear");
            assert!(main.contains("href=\"/bear/bear/resources\""));
        } else {
            assert_visible(main, "Select and save a permitted repository above");
            assert_visible(main, "Save permitted repositories");
        }
        assert!(main.contains("value=\"enable\" disabled"));
        assert!(main.contains("Help: how access is enforced"));
        assert!(main.contains("confirm_future_job_audience"));
    }
}

#[test]
fn skills_show_canonical_current_uses_and_prefill_attachment_edits() {
    let catalog = json!([
        {"id": "attached", "name": "Procedure", "version": "1", "content": "Reviewed <content>", "content_hash": "hash", "attached": true, "approved": true, "disabled": false, "owned": true, "profiles": ["chat", "work"]},
        {"id": "draft", "name": "Draft", "version": "2", "content": "Draft content", "attached": false, "approved": false, "disabled": false, "owned": true, "profiles": []},
        {"id": "catalog", "name": "Other", "version": "1", "content": "Other content", "attached": false, "approved": true, "disabled": false, "owned": false, "profiles": []}
    ]);
    let page = render(
        "bear/manage/skills.html",
        context! {
            can_manage_bear => true, catalog, draft => None::<Value>,
            error => None::<String>, pending_skill_id => None::<String>, pending_profiles => None::<Vec<String>>,
        },
    );
    let main = main_content(&page);
    assert_visible(
        main,
        "Current permitted uses:</strong> Browser Chat, Autonomous Work",
    );
    assert_visible(main, "Save permitted uses");
    assert!(main.contains("value=\"chat\" checked"));
    assert!(main.contains("value=\"work\" checked"));
    assert!(!main.contains("value=\"pair\" checked"));
    assert!(
        main.find("Your drafts &amp; catalog versions").unwrap()
            < main.find("id=\"skill-draft\"").unwrap()
    );
    assert!(
        main.find("Other approved catalog procedures").unwrap()
            < main.find("id=\"skill-catalog\"").unwrap()
    );
    assert!(main.contains("confirm_public\" value=\"true\" required"));
    assert!(main.contains("confirm_work\" value=\"true\""));
    assert!(!main.contains("confirm_work\" value=\"true\" checked"));
    assert!(main.contains("Reviewed &lt;content&gt;"));
    let other_uses = render(
        "bear/manage/skills.html",
        context! {
            can_manage_bear => true, catalog => json!([{"id": "existing", "name": "Existing", "version": "1", "attached": true, "approved": true, "owned": true, "profiles": ["curate", "watch"]}]),
            draft => None::<Value>, error => None::<String>, pending_skill_id => None::<String>, pending_profiles => None::<Vec<String>>,
        },
    );
    assert!(other_uses.contains("value=\"curate\" checked"));
    assert!(other_uses.contains("value=\"watch\" checked"));
    let pending = render(
        "bear/manage/skills.html",
        context! {
            can_manage_bear => true, catalog, draft => None::<Value>, error => "Work acknowledgement is required",
            pending_skill_id => "attached", pending_profiles => json!(["pair", "work"]),
        },
    );
    let main = main_content(&pending);
    assert_visible(main, "Work acknowledgement is required");
    assert_visible(
        main,
        "Current permitted uses:</strong> Browser Chat, Autonomous Work",
    );
    assert!(main.contains("value=\"pair\" checked"));
}

#[test]
fn saved_editors_show_failure_preserve_drafts_and_post_to_current_handle() {
    for template in ["bear/edit_overview.html", "bear/edit_prompt.html"] {
        let page = render(
            template,
            context! {
                provision_error => "Store could not initialize <evidence>",
                form => json!({"slug": "renamed", "name": "Draft <name>", "description": "Draft purpose", "system_prompt": "Draft </textarea><script>bad</script>"}),
            },
        );
        let main = main_content(&page);
        assert_visible(main, "Saved, but Bear initialization failed");
        assert_visible(
            main,
            "Submit the saved values again to retry initialization",
        );
        assert!(main.contains("Store could not initialize &lt;evidence&gt;"));
        assert!(main.contains("/advanced"));
        assert!(main.contains("action=\"/bear/bear/edit/"));
        assert!(!main.contains("<script>bad</script>"));
    }
    let page = render(
        "bear/new.html",
        context! {
            form => json!({"slug": "newbear", "name": "New", "description": "Purpose", "system_prompt": "Draft steering"}),
        },
    );
    assert!(page.contains("Optional Bear-wide steering"));
    assert!(page.contains("Handle"));
    assert!(page.contains("Next, create a hat"));
    let saved = render(
        "bear/new.html",
        context! {saved_bear_slug => "saved", provision_error => "Initialization failed"},
    );
    assert!(saved.contains("/bear/saved/models"));
    assert!(saved.contains("/bear/saved/advanced"));
    assert!(saved.contains("Do not create it again"));
    assert!(!saved.contains(">Create Bear</button>"));
}

#[test]
fn overview_attention_is_visible_only_for_authorized_admin_data() {
    for status in ["degraded", "unknown"] {
        let page = render(
            "bear/settings/overview.html",
            context! {
                can_manage_bear => true, overview_summary => json!({"hats": [], "jobs": []}),
                recall_health => json!({"status": status, "detail": "Recall needs attention"}),
                memory_stats => json!({"record_count": 4}), pending_reviews => 0,
            },
        );
        assert_visible(main_content(&page), "Needs attention");
        assert_visible(main_content(&page), "Recall needs attention");
        assert_visible(main_content(&page), "Inspect diagnostics");
    }
    let member = render(
        "bear/settings/overview.html",
        context! {
            can_manage_bear => false, overview_summary => json!({"hats": [], "jobs": []}),
        },
    );
    assert!(!main_content(&member).contains("Needs attention"));
    assert!(!main_content(&member).contains("Diagnostics"));
}

#[test]
fn unconfigured_semantic_recall_is_neutral_unless_memory_statistics_fail() {
    for statistics_available in [true, false] {
        let page = render(
            "bear/settings/overview.html",
            context! {
                can_manage_bear => true, overview_summary => json!({"hats": [], "jobs": []}),
                recall_health => json!({"status": "unavailable", "detail": "Semantic recall is not configured; memory search uses the keyword fallback."}),
                memory_stats => if statistics_available { json!({"record_count": 4}) } else { json!(null) },
            },
        );
        let main = main_content(&page);
        assert_visible(main, "Search uses the keyword fallback");
        assert_eq!(main.contains("Needs attention"), !statistics_available);
        assert_eq!(
            main.contains("Memory statistics unavailable"),
            !statistics_available
        );
        assert!(!main.contains("Derived search: <strong>unavailable"));
    }
}

#[test]
fn enabled_hat_without_repositories_shows_blocker_and_reenable_sequence() {
    let page = render(
        "bear/manage/hat.jinja",
        context! {
            can_manage_bear => true, hat => json!({"id": "hat", "name": "Engineering", "work_enabled": true}),
            effective_model => effective(), configurations => json!([]), choices => json!([]),
            grant_count => 0, historical_hat_records => 0, web_grants => json!({"hosts": []}),
        },
    );
    let main = main_content(&page);
    assert_visible(main, "Work is blocked: no repositories are permitted");
    assert_visible(main, "Disable Job use below before adding repositories");
    assert_visible(main, "then review and enable Work again");
    assert_visible(main, "Assign a repository to this Bear");
    assert!(main.contains("value=\"disable\">Disable Job use"));
    assert!(!main.contains("Enable Work for an empty hat"));
}

#[test]
fn knowledge_review_failures_preserve_escaped_drafts_and_require_fresh_consent() {
    for (template, acknowledgement) in [
        ("bear/manage/hat_review.jinja", "acknowledge_sharing"),
        (
            "bear/manage/hat_core_review.jinja",
            "acknowledge_bear_and_work_audience",
        ),
        (
            "bear/manage/hat_legacy_review.jinja",
            "acknowledge_unverified_source_and_members",
        ),
    ] {
        let page = render(
            template,
            context! {
                can_manage_bear => true, hat => json!({"id": "hat", "name": "Engineering", "work_enabled": true}),
                selected => json!({"id": "source", "memory_id": "source", "kind": "original", "content_text": "Current source"}),
                current_head => json!({"id": "new-head", "memory_id": "new-head", "content_text": "Current target"}),
                page => json!({"inventory": [], "candidates": []}), error => "Entry changed since review",
                draft => json!({"kind": "edited", "reviewed_content": "Draft </textarea><script>unsafe</script>", "review_notes": "Preserved <rationale>", "acknowledge_sharing": true, "acknowledge_bear_and_work_audience": true, "acknowledge_unverified_source_and_members": true, "work_audience_reviewed": true}),
            },
        );
        let main = main_content(&page);
        for text in [
            "Entry changed since review",
            "No entry was published",
            "Current source",
            "Current target",
            "Preserved &lt;rationale&gt;",
        ] {
            assert_visible(main, text);
        }
        assert!(main.contains("value=\"edited\""));
        assert!(main.contains("name=\"expected_head\" value=\"new-head\""));
        assert!(main.contains("Draft &lt;"));
        assert!(!main.contains("<script>unsafe</script>"));
        assert!(main.contains(&format!(
            "name=\"{acknowledgement}\" value=\"true\" required"
        )));
        assert!(!main.contains(" checked"));
    }
}

#[test]
fn work_review_discloses_metadata_not_reviewed_content_or_consent() {
    let page = render(
        "bear/manage/hat_work_review.jinja",
        context! {
            can_manage_bear => true, hat => json!({"id": "hat", "name": "Engineering", "work_enabled": false}),
            identity_preview => "Identity to review", grant_count => 0,
            error => "Hat identity changed; start again", review_draft => json!({"rationale": "Previous <rationale>"}),
            snapshot => json!({"total_records": 1, "page": 1, "page_count": 1, "complete": true, "sha256": "memory-hash", "identity_sha256": "identity-hash", "records": [{"kind": "fact", "content_text": "Entire historical entry <content>", "metadata_json": "metadata", "visibility": "shared", "memory_id": "record"}]}),
        },
    );
    let main = main_content(&page);
    for text in [
        "Hat identity changed",
        "Restart review at page 1",
        "Identity to review",
        "Entire historical entry &lt;content&gt;",
        "including superseded or archived entries",
        "confirm_work_audience",
        "Previous &lt;rationale&gt;",
        "permit a repository",
    ] {
        assert_visible(main, text);
    }
    assert!(main.contains("Record details"));
    assert!(main.contains("Snapshot details"));
    assert!(main.contains("confirm_work_audience\" value=\"true\" required"));
    assert!(main.contains("type=\"submit\" disabled>Record review and enable Work"));
    for (template, acknowledgement) in [
        ("bear/manage/hat_review.jinja", "acknowledge_sharing"),
        (
            "bear/manage/hat_core_review.jinja",
            "acknowledge_bear_and_work_audience",
        ),
        (
            "bear/manage/hat_legacy_review.jinja",
            "acknowledge_unverified_source_and_members",
        ),
    ] {
        let page = render(
            template,
            context! {
                can_manage_bear => true, hat => json!({"id": "hat", "name": "Engineering", "work_enabled": true}),
                selected => json!({"id": "record", "memory_id": "record", "kind": "fact", "source": "Private source", "content_text": "Content being reviewed"}),
                page => json!({"inventory": [], "candidates": []}),
                draft => None::<Value>, error => None::<String>, current_head => None::<Value>,
            },
        );
        assert_visible(main_content(&page), "Content being reviewed");
        assert_visible(main_content(&page), acknowledgement);
        assert!(page.contains("Source details"));
    }
}

#[test]
fn people_recovery_and_editor_environment_guidance_are_action_local() {
    let page = render(
        "bear/settings/access.html",
        context! {
            can_manage_bear => true, members => json!([]), message => "Cannot remove the last bear admin.",
            error => "Access was not changed", member_form => json!({"username": "Preserved <person>", "role": "admin"}),
        },
    );
    let main = main_content(&page);
    assert_visible(main, "Cannot remove the last bear admin");
    assert_visible(main, "Grant another Admin first");
    assert_visible(main, "Grant or change access");
    assert_visible(main, "Access was not changed");
    assert!(main.contains("value=\"Preserved &lt;person&gt;\""));
    assert!(main.contains("value=\"admin\" selected"));
    assert!(main.contains("does not grant access to every record or page"));
    let token = render(
        "bear/code_token.html",
        context! {raw_token => "one-time-test-token", api_server_url => "https://den.example"},
    );
    let main = main_content(&token);
    assert_visible(main, "You will not be able to see it again");
    assert_visible(main, "Zed must receive");
    assert_visible(main, "process environment");
    assert_visible(main, "View or revoke your Code tokens");
    assert!(main.contains("Saving the token"));
    assert!(main.contains("does not configure Zed"));
}
