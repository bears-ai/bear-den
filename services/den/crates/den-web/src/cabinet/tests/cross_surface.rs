//! Den-to-Cabinet links use the configured resolver; Den-owned actions stay local.

use super::templates::{render, render_with_origin};
use minijinja::context;

#[test]
fn den_bear_operator_and_job_navigation_resolves_cabinet_without_moving_capabilities() {
    for name in [
        "_den_nav.html",
        "admin/_operator_nav.html",
        "bear/settings/_bear_nav.html",
        "bear/settings/access.html",
        "bear/settings/conversations.html",
        "bear/settings/overview.html",
        "bear/settings/policy.html",
        "work/index.html",
    ] {
        let local = render(name, context! {});
        assert!(local.contains("href=\"/cabinet\""), "{name}");
        let dedicated = render_with_origin(name, context! {}, Some("https://cabinet.example"));
        assert!(
            dedicated.contains("href=\"https://cabinet.example/cabinet\""),
            "{name}"
        );
        assert!(!dedicated.contains("href=\"/cabinet"), "{name}");
        assert!(
            !dedicated.contains("action=\"https://cabinet.example"),
            "{name}"
        );
    }
    let capabilities = render_with_origin(
        "bear/settings/policy.html",
        context! {},
        Some("https://cabinet.example"),
    );
    assert!(capabilities.contains("action=\"/bear/atlas/web-sources\""));
    assert!(capabilities.contains("href=\"/bear/atlas/hats\""));
    assert!(capabilities.contains("Page content and page access are managed in Cabinet"));
    assert!(!capabilities.contains("/policy\""));
}

#[test]
fn review_links_pin_the_immutable_revision_on_the_configured_cabinet() {
    let values = context! {
        cabinet_reviews => [context! {
            cabinet_ref => "cabi_review", version_ref => "cabv_review",
            title => "<script>title()</script>", revision => 4,
        }],
    };
    let local = render("reviews.html", values.clone());
    assert!(local.contains("href=\"/cabinet/cabi_review?version=cabv_review\""));
    let dedicated = render_with_origin("reviews.html", values, Some("https://cabinet.example"));
    assert!(dedicated
        .contains("href=\"https://cabinet.example/cabinet/cabi_review?version=cabv_review\""));
    assert!(!dedicated.contains("href=\"/cabinet"));
    assert!(!dedicated.contains("<script>title()"));
}

#[test]
fn job_mission_and_retirement_links_resolve_but_job_evidence_and_actions_stay_den_owned() {
    let values = context! {
        job_id => "job_fixture", status => "completed",
        mission => context! {
            reference => "cabi_mission", title => "Mission", can_edit => true,
            revision => 2, version => "cabv_published",
            saved_copies => [context! { reference => "artifact_active", readable => true },
                context! { reference => "artifact_retired", retired => true }],
        },
    };
    let local = render("work/job.html", values.clone());
    assert!(local.contains("href=\"/cabinet/cabi_mission\""));
    assert!(local.contains("href=\"/cabinet/saved-copies/artifact_active/retire\""));
    let dedicated = render_with_origin("work/job.html", values, Some("https://cabinet.example"));
    for path in [
        "/cabinet/cabi_mission",
        "/cabinet/saved-copies",
        "/cabinet/saved-copies/artifact_active/retire",
        "/cabinet/saved-copies/artifact_retired/retire",
    ] {
        assert!(
            dedicated.contains(&format!("href=\"https://cabinet.example{path}\"")),
            "{path}"
        );
    }
    assert!(!dedicated.contains("href=\"/cabinet"));
    assert!(dedicated.contains("action=\"/bear/atlas/jobs/job_fixture/mission\""));
    assert!(dedicated.contains("action=\"/bear/atlas/jobs/job_fixture/mission/snapshot\""));
    assert!(dedicated
        .contains("href=\"/bear/atlas/jobs/job_fixture/evidence/artifact_active/content\""));
}

#[test]
fn deletion_links_resolve_active_and_retired_receipts_without_moving_bear_deletion() {
    let values = context! {
        copies => context! { copies => [context! { reference => "artifact_active" },
            context! { reference => "artifact_retired", retired => true }] },
        preview => context! { can_delete => true },
    };
    let local = render("bear/delete.html", values.clone());
    assert!(local.contains("href=\"/cabinet/saved-copies/artifact_retired/retire\""));
    let dedicated = render_with_origin("bear/delete.html", values, Some("https://cabinet.example"));
    for reference in ["artifact_active", "artifact_retired"] {
        assert!(dedicated.contains(&format!(
            "href=\"https://cabinet.example/cabinet/saved-copies/{reference}/retire\""
        )));
    }
    assert!(!dedicated.contains("href=\"/cabinet"));
    assert!(dedicated.contains("action=\"/bear/atlas/delete\""));
}

#[test]
fn cabinet_internal_navigation_and_forms_remain_local_with_a_dedicated_origin() {
    for name in ["index", "item", "edit", "discussion", "history"] {
        let html = render_with_origin(
            &format!("cabinet/{name}.html"),
            context! { cabinet_surface => true, can_post => true },
            Some("https://cabinet.example"),
        );
        assert!(!html.contains("href=\"https://cabinet.example"), "{name}");
        assert!(!html.contains("action=\"https://cabinet.example"), "{name}");
        assert!(!html.contains("Den management"), "{name}");
        assert!(html.contains("href=\"/cabinet\""), "{name}");
    }
}
