use minijinja::{context, Value};

pub(super) fn render(name: &str, values: Value) -> String {
    render_with_origin(name, values, None)
}

pub(super) fn render_with_origin(name: &str, values: Value, origin: Option<&str>) -> String {
    let mut config = crate::config::Config::test_stub();
    config.cabinet_web_origin = origin.map(|origin| origin.parse().unwrap());
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let defaults = context! {
        app_display_name => "Bears",
        template_tag => name.trim_end_matches(".html").replace('/', "-"),
        session => context! { username => "Casey", is_admin => true },
        item_title => "A shared page", cabinet_ref => "cabi_fixture",
        page => context! {}, audience => context! { kind => "open_wiki" },
        current_audience => context! { kind => "open_wiki" },
        target_audience => context! { kind => "open_wiki" },
        roots_only => true, items => Vec::<Value>::new(), review_items => Vec::<Value>::new(),
        versions => Vec::<Value>::new(), entries => Vec::<Value>::new(),
        author_labels => Vec::<Value>::new(), history => context! {}, view => context! {},
        details => context! {}, preview => context! {},
        bear => context! { slug => "atlas", name => "Atlas" },
        bear_slug => "atlas", can_manage_bear => true,
        mission => context! {}, provider_status => context! {},
        dispatch_preflight => context! { dispatchable => true },
        overview_summary => context! {}, member_form => context! {},
        live_reflection_status => context! {},
    };
    super::normalize_markup(
        crate::template_environment(&config)
            .get_template(name)
            .unwrap()
            .render(context! { ..values, ..defaults })
            .unwrap(),
    )
}

fn assert_cabinet_only(html: &str) {
    assert!(html.contains("cabinet-surface"));
    assert!(html.contains("aria-label=\"Cabinet navigation\""));
    assert!(html.contains("class=\"logo-wordmark\">Cabinet</span>"));
    for text in [
        "Browse",
        "Search",
        "Pages",
        "Reviews",
        "Uploads",
        "Saved copies",
        "Return to Den",
        "Logout",
    ] {
        assert!(html.contains(text), "missing {text}");
    }
    for href in [
        "/admin",
        "/settings",
        "/connections",
        "/reviews",
        "/work/surfaces",
    ] {
        assert!(
            !html.contains(&format!("href=\"{href}\"")),
            "unexpected Den link {href}"
        );
    }
    assert!(!html.contains("aria-label=\"Den management\""));
    assert!(!html.contains(">Bears</a>"));
}

#[test]
fn cabinet_templates_render_distinct_shell_even_for_admin_and_with_fixture_defaults() {
    for name in [
        "index",
        "item",
        "history",
        "discussion",
        "new",
        "edit",
        "attachment",
        "uploads",
        "saved_copies",
        "retire_snapshot",
        "move",
    ] {
        let html = render(
            &format!("cabinet/{name}.html"),
            context! { cabinet_surface => true },
        );
        assert_cabinet_only(&html);
        assert!(html.contains("href=\"/\">Return to Den"));
        assert!(html.contains("href=\"/cabinet\" class=\"logo cabinet-logo\""));
        assert!(html.contains("/assets/css/style.css"));
        assert!(html.contains("href=\"#cabinet-content\">Skip to content"));
        assert!(html.contains("id=\"cabinet-content\" tabindex=\"-1\""));
        assert!(!html.contains("style=\""));
        assert!(!html.contains("<style"));
    }
}

#[test]
fn cabinet_context_uses_configured_links_and_den_shell_keeps_legacy_fallback() {
    let html = render(
        "cabinet/index.html",
        context! {
            cabinet_surface => true, cabinet_home_url => "https://cabinet.example/cabinet",
            den_home_url => "https://den.example",
        },
    );
    assert_cabinet_only(&html);
    assert!(html.contains("href=\"https://den.example\">Return to Den"));
    assert!(html.contains("href=\"https://cabinet.example/cabinet\" class=\"logo cabinet-logo\""));
    let legacy = render("cabinet/index.html", context! {});
    assert!(legacy.contains("aria-label=\"Den management\""));
    assert!(legacy.contains("href=\"/cabinet\""));
    let den = render_with_origin(
        "cabinet/index.html",
        context! { cabinet_surface => false, cabinet_home_url => "https://cabinet.example/cabinet" },
        Some("https://cabinet.example"),
    );
    assert!(den.contains("aria-label=\"Den management\""));
    assert_eq!(
        den.matches("href=\"https://cabinet.example/cabinet\"")
            .count(),
        3
    );
    assert!(!den.contains("cabinet-surface"));
    assert!(den.contains("href=\"/admin\""));
}

#[test]
fn cabinet_signed_out_shell_has_login_return_and_no_den_rail() {
    let html = render(
        "login.html",
        context! {
            cabinet_surface => true, session => context! {}, form => context! {},
        },
    );
    assert!(html.contains("Return to Den"));
    assert!(html.contains("href=\"/login\""));
    assert!(!html.contains("Den management"));
    assert!(!html.contains("href=\"/admin\""));
}

#[test]
fn page_tabs_mark_the_current_section_and_do_not_bind_discussion_to_a_revision() {
    for (name, label) in [
        ("item", "Overview"),
        ("edit", "Overview"),
        ("discussion", "Discussion"),
        ("history", "History"),
    ] {
        let html = render(
            &format!("cabinet/{name}.html"),
            context! { cabinet_surface => true },
        );
        assert!(html.contains("aria-label=\"Page sections\""));
        let tabs = html
            .split("aria-label=\"Page sections\"")
            .nth(1)
            .unwrap()
            .split("</nav>")
            .next()
            .unwrap();
        assert_eq!(tabs.matches("aria-current=\"page\"").count(), 1);
        assert!(!tabs.contains("role=\"tab\""));
        assert!(html.contains(&format!("aria-current=\"page\">{label}</a>")));
        assert!(html.contains("href=\"/cabinet/cabi_fixture/discussion\""));
        assert!(!html.contains("discussion?version="));
    }
}

#[test]
fn discussion_escapes_posts_authors_and_preserved_drafts_and_gates_the_form() {
    let draft = "</textarea><script>draft()</script>";
    let values = context! {
        cabinet_surface => true, can_post => true, draft, error => "Shorten your draft.",
        entries => [context! {
            discussion_ref => "cabd_fixture", content => "<script>posted()</script>\nA second line",
            author_name => "<img src=x onerror=attack()>", authored_at => "2026-01-01T00:00:00Z",
        }], author_labels => ["Person (user 4)"],
    };
    let html = render("cabinet/discussion.html", values);
    assert!(html.contains("Current page discussion"));
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("aria-invalid=\"true\""));
    assert!(html.contains("maxlength=\"16000\""));
    assert!(html.contains("&lt;script&gt;posted()&lt;/script&gt;"));
    assert!(html.contains("&lt;/textarea&gt;&lt;script&gt;draft()&lt;/script&gt;"));
    assert!(!html.contains("<script>posted()"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains("name=\"user_id\""));
    assert!(!html.contains("name=\"authored_by\""));
    let readonly = render(
        "cabinet/discussion.html",
        context! { cabinet_surface => true, can_post => false, draft },
    );
    assert!(!readonly.contains("method=\"post\""));
    assert!(readonly.contains("Your unposted draft"));
    let archived = render(
        "cabinet/discussion.html",
        context! { cabinet_surface => true, can_post => false, archived => true },
    );
    assert!(archived.contains("Restore it before adding a message"));
    assert!(!archived.contains("method=\"post\""));
}

#[test]
fn history_shows_review_current_authorship_and_safe_explicit_comparison() {
    let html = render(
        "cabinet/history.html",
        context! {
            cabinet_surface => true,
            versions => [context! { version_ref => "cabv_current", revision => 3,
                review => "approved", is_current => true, author_label => "Person (user 4)",
                authored_at => "2026-01-01T00:00:00Z" },
                context! { version_ref => "cabv_pending", revision => 4, review => "pending", author_label => "Bear" }],
            comparison => context! { before_revision => 1, current_revision => 3,
                before_ref => "cabv_before", current_ref => "cabv_current",
                before_content => "<script>before()</script>", current_content => "<img onerror=now()>", unchanged => false },
        },
    );
    assert!(html.contains("Current</strong> · Approved"));
    assert!(html.contains("Pending review · unpublished"));
    assert!(html.contains("Person (user 4)"));
    assert!(html.contains("datetime=\"2026-01-01T00:00:00Z\""));
    assert!(html.contains("href=\"/cabinet/cabi_fixture?version=cabv_current\""));
    assert!(html.contains("href=\"/cabinet/cabi_fixture?version=cabv_before\""));
    assert!(html.contains("scope=\"col\""));
    assert!(html.contains("aria-label=\"Page revision history\" tabindex=\"0\""));
    assert!(html.contains("Before · revision 1"));
    assert!(html.contains("&lt;script&gt;before()&lt;/script&gt;"));
    assert!(!html.contains("<script>before()"));
    assert!(!html.contains("<img onerror=now()"));
    assert!(!html.contains("history?before=cabv_pending"));
}

#[test]
fn cabinet_navigation_marks_only_the_current_view() {
    for (view, label) in [
        ("browse", "Browse"),
        ("search", "Search"),
        ("pages", "Pages"),
        ("reviews", "Reviews"),
    ] {
        let html = render(
            "_cabinet_nav.html",
            context! { template_tag => "cabinet-index", index_view => view },
        );
        assert_eq!(html.matches("aria-current=\"page\"").count(), 1);
        assert!(html.contains(&format!("aria-current=\"page\">{label}</a>")));
    }
}

#[test]
fn page_markdown_is_sanitized_and_acl_controls_do_not_follow_admin_or_historical_content() {
    let values = context! {
        cabinet_surface => true, is_current => true, lifecycle => "active", version_ref => "cabv_published",
        content => "**Readable**\n\n<script>unsafe()</script>\n\n<img src=x onerror=unsafe()>\n\n[Bad](javascript:alert(1))",
        page => context! { can_write => false, can_manage => false, can_review => false },
        review => "pending",
    };
    let readonly = render("cabinet/item.html", values);
    assert!(readonly.contains("<strong>Readable</strong>"));
    for unsafe_markup in [
        "<script>unsafe()",
        "onerror=",
        "href=\"javascript:",
        ">Edit page</a>",
        "/policy\"",
        "/archive\"",
        "/delete\"",
        "/review\"",
    ] {
        assert!(!readonly.contains(unsafe_markup), "{unsafe_markup}");
    }
    assert!(readonly.contains("href=\"/cabinet/cabi_fixture?version=cabv_published\""));
    let owner = render(
        "cabinet/item.html",
        context! {
            cabinet_surface => true,
            is_current => true, lifecycle => "active", review => "pending",
            page => context! { can_write => true, can_manage => true, can_review => true },
        },
    );
    for action in [
        "/edit\"",
        "/policy\"",
        "/archive\"",
        "/delete\"",
        "/review\"",
    ] {
        assert!(owner.contains(action), "{action}");
    }
    let historical = render(
        "cabinet/item.html",
        context! {
            cabinet_surface => true,
            is_current => false, has_published => true, lifecycle => "active",
            page => context! { can_write => true },
        },
    );
    assert!(!historical.contains(">Edit page</a>"));
    assert!(!historical.contains("/archive\""));
    assert!(historical.contains("current policy"));
}

#[test]
fn editor_escapes_and_describes_preserved_drafts_and_archived_pages_disable_publication() {
    let draft = "</textarea><script>draft()</script>";
    let html = render(
        "cabinet/edit.html",
        context! {
            cabinet_surface => true,
            content => draft, item_title => "\" onfocus=attack()", error => "Your draft is preserved.",
        },
    );
    assert!(html.contains("id=\"edit-error\""));
    assert!(html.contains("aria-describedby=\"edit-error\""));
    assert!(html.contains("&lt;/textarea&gt;&lt;script&gt;draft()&lt;/script&gt;"));
    assert!(!html.contains("<script>draft()"));
    assert!(!html.contains("value=\"\" onfocus="));
    for archived in [false, true] {
        let html = render(
            "cabinet/edit.html",
            context! { cabinet_surface => true, content => draft, can_publish => false, archived },
        );
        assert!(!html.contains("method=\"post\""));
        assert!(html.contains("readonly"));
        assert!(!html.contains("<script>draft()"));
    }
}

#[test]
fn discussion_draft_validation_uses_unicode_characters_and_rejects_empty_or_nul() {
    use super::super::discussion::draft_error;
    assert!(draft_error(" \n\t ").is_some());
    assert!(draft_error("hello\0world").unwrap().contains("NUL"));
    assert!(draft_error(&"🦀".repeat(16_000)).is_none());
    assert!(draft_error(&"🦀".repeat(16_001))
        .unwrap()
        .contains("16,000"));
    assert!(draft_error("A helpful observation").is_none());
}
