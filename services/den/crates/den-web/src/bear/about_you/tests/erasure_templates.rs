use super::*;
use crate::admin::usability_tests::{assert_link, opening_tag, render};

#[test]
fn erasure_discloses_all_history_scope_and_limits_without_claiming_pending_cleanup_is_complete() {
    let common = minijinja::context! {
        bear => minijinja::context! { name => "About you Bear", slug => "aboutyou" },
        can_manage_bear => false, capture_enabled => false,
        capture_token => Uuid::new_v4().to_string(), erase_token => Uuid::new_v4().to_string(),
        current => minijinja::context! { state => "Retracted", name => Option::<String>::None },
        candidates => [minijinja::context! {
            id => "erased-candidate", name => Option::<String>::None, state => "Erased",
            approve => Option::<String>::None, reviews => Vec::<minijinja::Value>::new(),
        }],
    };
    let page = render(
        "bear/memory/about_you.html",
        minijinja::context! {
            erasure_cleanup_pending => true, ..common.clone(),
        },
    );
    assert!(page.contains("<h3>Erasure cleanup pending</h3>"));
    assert!(page.contains("Erasure is not complete while cleanup is pending"));
    assert_link(&page, BASE, "Retry cleanup");
    assert!(!page.contains("Erasure complete"));
    assert!(!page.contains("name=\"name\""));
    for text in [
        "all current and historical preferred-name fact body versions",
        "not just the 100 recent requests shown above",
        "not a complete history or export",
        "content-free audit records remain",
        "Original human conversation history is separate and is not erased",
        "cannot recall input already sent to a provider or remove existing backups",
        "not a forensic disk scrub",
        "Erased preferred-name request</h4>",
        "<p>Erased</p>",
        "<strong>Retracted</strong>",
    ] {
        assert!(
            page.contains(text),
            "missing erasure disclosure/display: {text}"
        );
    }
    for action in ["publish", "reject", "expire", "defer", "resume"] {
        assert!(!page.contains(&format!("id=\"{action}-erased-candidate\"")));
    }
    let target = format!("{BASE}/erase");
    let erase = opening_tag(&page, "form", "id", "erase-names");
    assert_eq!(erase.attribute("method"), Some("post"));
    assert_eq!(erase.attribute("action"), Some(target.as_str()));
    let markup = page
        .split_once("<form id=\"erase-names\"")
        .unwrap()
        .1
        .split("</form>")
        .next()
        .unwrap();
    let confirmation = opening_tag(markup, "input", "name", "acknowledged");
    assert_eq!(confirmation.attribute("type"), Some("checkbox"));
    assert!(confirmation.has_attribute("required"));
    assert_eq!(
        markup.matches("name=\"").count(),
        2,
        "only token and separate acknowledgement are submitted"
    );
    let settled = render(
        "bear/memory/about_you.html",
        minijinja::context! {
            erasure_cleanup_pending => false, ..common,
        },
    );
    assert!(!settled.contains("<h3>Erasure cleanup pending</h3>"));
    assert!(!settled.contains("Erasure complete"));
}
