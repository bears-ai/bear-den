//! Rendering regressions for the ownership-led shell and permission-safe views.

use std::path::Path;

use minijinja::{context, Value};

use crate::{config::Config, template_environment};

fn render(name: &str, values: Value) -> String {
    let mut config = Config::test_stub();
    config.templates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/templates")
        .to_string_lossy()
        .into_owned();
    let bear = match values.get_attr("bear") {
        Ok(value) if !value.is_undefined() => value,
        _ => context! { name => "Atlas", slug => "atlas", description => "Care for the house" },
    };
    template_environment(&config)
        .get_template(name)
        .expect("template exists")
        .render(context! {
            app_display_name => "Bears",
            bear,
            ..values
        })
        .unwrap_or_else(|error| panic!("render {name}: {error:#}"))
}

#[test]
fn ownership_navigation_has_one_ordered_list_of_destinations() {
    let html = render(
        "bear/settings/_bear_nav.html",
        context! {
            can_manage_bear => true, bear_nav_active => "docket",
        },
    );
    let mut offset = 0;
    for label in [
        "Overview",
        "Chat",
        "Yours",
        "Purpose",
        "Hats",
        "Memory",
        "Skills",
        "This Den",
        "Tools",
        "Connections",
        "What it can use",
        "Jobs",
        "Cabinet",
        "Activity",
        "People",
        "Backup &amp; move",
    ] {
        let next = html[offset..]
            .find(label)
            .unwrap_or_else(|| panic!("missing {label}"));
        offset += next + label.len();
    }
    assert!(html.contains("/jobs\" aria-current=\"page\""));
    assert!(html.contains("href=\"/cabinet\""));
    assert!(!html.contains("Identity &amp; charter"));
    assert!(!html.contains(">Resources<"));
}

#[test]
fn member_navigation_does_not_advertise_admin_inspection_routes() {
    let html = render(
        "bear/settings/_bear_nav.html",
        context! { can_manage_bear => false },
    );
    assert!(html.contains("/identity#hats"));
    for route in [
        "/hats\"",
        "/activity",
        "/context",
        "/advanced",
        "/reflections",
    ] {
        assert!(!html.contains(route), "member navigation contains {route}");
    }
}

#[test]
fn den_header_has_shared_destinations_only_when_signed_in() {
    let signed_in = render(
        "dashboard.html",
        context! {
            session => context! { username => "Casey", is_admin => false }, bears => Vec::<Value>::new(),
        },
    );
    for route in ["/cabinet", "/connections", "/reviews"] {
        assert!(signed_in.contains(&format!("href=\"{route}\"")));
    }
    let logged_out = render("base.html", context! {});
    assert!(!logged_out.contains("href=\"/reviews\""));
}

#[test]
fn shared_pages_keep_one_den_sidebar_without_inventing_a_selected_bear() {
    for (template, tag, active) in [
        ("dashboard.html", "dashboard", "/"),
        ("connections.html", "connections", "/connections"),
        ("reviews.html", "reviews", "/reviews"),
        ("cabinet/index.html", "cabinet-index", "/cabinet"),
        ("cabinet/history.html", "cabinet-history", "/cabinet"),
        ("work/surfaces.html", "work-surfaces", "/work/surfaces"),
    ] {
        let html = render(
            template,
            context! {
                session => context! { username => "Casey", is_admin => false },
                template_tag => tag,
                bears => Vec::<Value>::new(),
                items => Vec::<Value>::new(),
            },
        );
        assert_eq!(
            html.matches("class=\"bear-manage-nav\"").count(),
            1,
            "{template}"
        );
        assert!(html.contains("aria-label=\"Den management\""), "{template}");
        assert!(
            html.contains(&format!("href=\"{active}\" aria-current=\"page\"")),
            "{template}"
        );
        assert!(
            !html.contains("aria-label=\"Bear management\""),
            "{template}"
        );
        assert!(!html.contains("href=\"/admin\""), "{template}");
    }
}

#[test]
fn bear_pages_override_the_den_sidebar_without_duplicating_navigation() {
    for template in [
        "bear/settings/overview.html",
        "bear/manage/identity.html",
        "design/chat.html",
    ] {
        let html = render(
            template,
            context! {
                session => context! { username => "Casey", is_admin => false },

                can_manage_bear => false,
                overview_summary => context! { hats => Vec::<Value>::new(), jobs => Vec::<Value>::new() },
            },
        );
        assert_eq!(
            html.matches("class=\"bear-manage-nav\"").count(),
            1,
            "{template}"
        );
        assert!(
            html.contains("aria-label=\"Bear management\""),
            "{template}"
        );
        assert!(
            !html.contains("aria-label=\"Den management\""),
            "{template}"
        );
        assert!(
            !html.contains("&lt;nav"),
            "sidebar must remain HTML, not escaped text"
        );
    }
}

#[test]
fn public_pages_do_not_render_an_authenticated_management_sidebar() {
    let html = render("base.html", context! {});
    assert!(!html.contains("class=\"bear-manage-nav\""));
    assert!(!html.contains("class=\"bear-manage\""));
    let admin = render(
        "dashboard.html",
        context! { session => context! { username => "Casey", is_admin => true } },
    );
    assert!(admin.contains("aria-label=\"Den management\""));
    assert!(admin.contains("href=\"/admin\""));
}

#[test]
fn overview_keeps_core_paths_and_omits_private_admin_summaries_for_members() {
    let html = render(
        "bear/settings/overview.html",
        context! {
            can_manage_bear => false,
            overview_summary => context! {
                hats => [context! { id => "hat", name => "Home", work_enabled => false }],
                jobs => [context! { id => "job", goal => "Visible Job", status => "ready" }],
            },
            recent_conversations => [context! { id => "private", title => "PRIVATE ADMIN TRANSCRIPT" }],
            memory_stats => context! { record_count => 9999 },
            pending_reviews => 77,
        },
    );
    for route in ["/bear/atlas\"", "/memory", "/resources", "/jobs"] {
        assert!(html.contains(route));
    }
    assert!(html.contains("<h2>Responsibilities</h2>"));
    assert!(html.contains("Visible Job"));
    assert!(!html.contains("PRIVATE ADMIN TRANSCRIPT"));
    assert!(!html.contains("9999"));
    assert!(!html.contains("77 memory"));
}

#[test]
fn resource_member_view_hides_other_sources_activity_even_if_supplied() {
    let html = render(
        "bear/settings/policy.html",
        context! {
            can_manage_bear => false, hats_configured => true,
            web_fetches => [context! { url => "PRIVATE FETCH", host => "private.example" }],
            plan_mode_rows => [context! { username => "PRIVATE USER", acp_session_id => "private-session" }],
        },
    );
    assert!(html.contains("Open Cabinet"));
    assert!(!html.contains("PRIVATE FETCH"));
    assert!(!html.contains("PRIVATE USER"));
    assert!(!html.contains("private-session"));
}

#[test]
fn failed_review_count_is_not_presented_as_an_empty_queue() {
    let html = render(
        "reviews.html",
        context! {
            bears => [context! { name => "Atlas", slug => "atlas", pending => Option::<i64>::None }],
        },
    );
    assert!(html.contains("Memory review count unavailable"));
    assert!(!html.contains("No memory proposals awaiting review"));
    assert!(html.contains("/memory#review-queue"));
}

#[test]
fn backup_page_reports_actual_bundle_limits_and_requires_admin_for_export_link() {
    let member = render(
        "bear/manage/portability.html",
        context! { can_manage_bear => false },
    );
    assert!(!member.contains("href=\"/bear/atlas/export.bear\""));
    let admin = render(
        "bear/manage/portability.html",
        context! { can_manage_bear => true },
    );
    assert!(admin.contains("href=\"/bear/atlas/export.bear\""));
    assert!(admin.contains("includes private source-local notes"));
    assert!(admin.contains("Hat definitions, identity and IDE choice"));
    assert!(admin.contains("new IDs, Work and automatic sharing off on import"));
    assert!(admin.contains("does not flush pending curation"));
}

#[test]
fn jobs_are_rendered_inside_the_same_bear_shell() {
    let html = render(
        "work/index.html",
        context! {
            bear_slug => "atlas", bear_name => "Atlas", can_manage_bear => false,
            provider_status => context! { configured => false },
            jobs => Vec::<Value>::new(),
        },
    );
    assert!(html.contains("aria-label=\"Bear management\""));
    assert!(html.contains("/jobs\" aria-current=\"page\""));
    assert!(html.contains("New Job"));
    assert!(!html.contains("/advanced"));
}

#[test]
fn purpose_content_is_escaped_and_only_admins_see_steering() {
    let values = |admin| {
        context! {
            can_manage_bear => admin,
            bear => context! { name => "Atlas", slug => "atlas", system_prompt => "<script>private-steering</script>" },
        }
    };
    let member = render("bear/manage/identity.html", values(false));
    assert!(!member.contains("private-steering"));
    let admin = render("bear/manage/identity.html", values(true));
    assert!(admin.contains("&lt;script&gt;private-steering"));
    assert!(!admin.contains("<script>private-steering"));
    assert!(admin.contains("/models"));
}
