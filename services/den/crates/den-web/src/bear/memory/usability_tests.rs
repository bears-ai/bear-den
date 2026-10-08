use minijinja::context;

use crate::admin::usability_tests::{assert_visible, render};

#[test]
fn memory_feedback_counts_and_review_actions_are_outside_closed_diagnostics() {
    let html = render(
        "bear/memory/dashboard.html",
        context! {
            can_manage_bear => true, head_count => 4,
            stats => context! { record_count => 8, shared_count => 2, profile_local_count => 6, distinct_paths => 4,
                sequence_high_water => 12345, db_path => "DB_DIAGNOSTIC_ONLY", pending_observations => 2 },
            reviewable_proposal_count => 1, pending_review_count => 1, needs_human_review_count => 0,
            pending_proposals => [context! { id => "proposal", title => "VISIBLE REVIEW CONTENT", requires_human => true, sensitivity => "private" }],
            import_notice => "IMPORT SAVED", import_error => "IMPORT FAILED <unsafe>",
            inspection_errors => ["ACTUAL STORE READ FAILURE"],
            review_notice => "REVIEW SAVED", review_error => "REVIEW FAILED",
        },
    );
    for text in [
        "IMPORT SAVED",
        "IMPORT FAILED",
        "REVIEW SAVED",
        "REVIEW FAILED",
        "current entries",
        "records (all versions)",
        "shared (core)",
        "VISIBLE REVIEW CONTENT",
        "clear review queue",
    ] {
        assert_visible(&html, text);
    }
    assert!(html.contains("&lt;unsafe&gt;"));
    assert_visible(&html, "ACTUAL STORE READ FAILURE");
    assert_eq!(html.matches("class=\"bear-manage-nav\"").count(), 1);
    assert!(
        html.find("Help: store and search diagnostics").unwrap()
            < html.find("DB_DIAGNOSTIC_ONLY").unwrap()
    );
    assert!(html.contains("id=\"legacy-memory-import\" open"));
    assert_visible(&html, "128 MiB");
}

#[test]
fn failed_counts_and_unverified_import_state_do_not_look_empty_or_ready() {
    let html = render(
        "bear/memory/dashboard.html",
        context! {
            can_manage_bear => true, stats => Option::<u8>::None, head_count => Option::<i64>::None,
            reviewable_proposal_count => Option::<i64>::None, legacy_import_locked => true,
            import_error => "FAILED MIGRATION",
        },
    );
    assert_visible(&html, "Memory store statistics are unavailable");
    assert_visible(&html, "Review counts unavailable");
    assert_visible(&html, "memory store state could not be verified");
    assert!(!html.contains("Nothing awaiting review."));
    assert!(!html.contains("disabled because this Bear already has memory records"));
}

#[test]
fn search_distinguishes_configuration_from_capability_for_both_viewers() {
    for can_manage_bear in [true, false] {
        let html = render(
            "bear/memory/search.html",
            context! {
                can_manage_bear, semantic_available => false, q => "preserved query",
                notice => "REAL FALLBACK NOTICE", mode_used => "keyword", result_count => 0,
            },
        );
        assert_visible(&html, "not configured in this deployment");
        assert_visible(&html, "REAL FALLBACK NOTICE");
        assert!(!html.contains("not yet available"));
        assert!(html.contains("value=\"preserved query\""));
        let configured = render(
            "bear/memory/search.html",
            context! {
                can_manage_bear, semantic_available => true, mode_used => "semantic",
            },
        );
        assert!(configured.contains("value=\"semantic\" checked"));
        assert!(!configured.contains("not configured in this deployment"));
    }
}

#[test]
fn entry_entity_and_reflection_inspection_keep_the_actual_objects_available() {
    let entry = render(
        "bear/memory/record.html",
        context! {
            is_head => false, record => context! {
                content_text => "VISIBLE CANONICAL CONTENT", visibility => "private", author_profile => "pair",
                memory_id => "record-id", metadata_json => context! { evidence => "VISIBLE RECORD EVIDENCE" },
            },
        },
    );
    for text in [
        "VISIBLE CANONICAL CONTENT",
        "private",
        "VISIBLE RECORD EVIDENCE",
        "superseded version",
        "Request review",
    ] {
        assert_visible(&entry, text);
    }
    let entity = render(
        "bear/memory/entity.html",
        context! {
            entity => context! { display_name => "ENTITY NAME", entity_id => "ENTITY ID", trust => "recorded trust",
                metadata_json => context! { evidence => "ENTITY EVIDENCE" } },
            handles => [context! { handle_value => "ENTITY HANDLE" }],
        },
    );
    for text in [
        "ENTITY ID",
        "ENTITY HANDLE",
        "ENTITY EVIDENCE",
        "recorded trust",
    ] {
        assert_visible(&entity, text);
    }
    let run = render(
        "bear/memory/reflection_run.html",
        context! {
            detail => context! { run => context! { status_label => "failed", error => "ACTUAL RUN FAILURE" },
                input_summary_pretty => "RUN INPUT EVIDENCE", output_summary_pretty => "RUN OUTPUT EVIDENCE" },
        },
    );
    for text in [
        "ACTUAL RUN FAILURE",
        "RUN INPUT EVIDENCE",
        "RUN OUTPUT EVIDENCE",
    ] {
        assert_visible(&run, text);
    }
}

#[test]
fn context_leads_with_recorded_budget_and_content_not_an_assembly_lesson() {
    let html = render(
        "bear/settings/context.html",
        context! {
            can_manage_bear => true,
            latest_budget => context! { model => "MODEL EVIDENCE", conversation_id => "conv", conversation_title => "Recorded conversation",
                over_budget => true, estimated_input_tokens => 12000, reserved_output_tokens => 1000,
                components => [context! { label => "BUDGET COMPONENT", tokens => 5000, pct_display => "42" }] },
            compiled_bound_prompts => [context! { role => "Bear base", char_count => 900, prompt_preview => "STORED PROMPT PREVIEW" }],
            standing_notes => [context! { title => "Standing note", scope => "every stance", body_preview => "STORED NOTE PREVIEW" }],
            recall_configured => true,
        },
    );
    for text in [
        "BUDGET COMPONENT",
        "over budget",
        "STORED PROMPT PREVIEW",
        "STORED NOTE PREVIEW",
        "not a live turn preview",
    ] {
        assert_visible(&html, text);
    }
    assert!(html.find("BUDGET COMPONENT").unwrap() < html.find("Help: how context").unwrap());
    assert!(html.contains("not a successful retrieval or health check"));
    assert!(!html.contains("what your Bear sees when it thinks"));
}

#[test]
fn reflection_definitions_are_optional_but_skips_failures_and_retry_remain_visible() {
    for template in [
        "bear/settings/reflections.html",
        "bear/settings/conversation.html",
    ] {
        let html = render(
            template,
            context! {
                can_manage_bear => true, conv => context! { id => "conv" },
                reflections => [
                    context! { status => "processed", status_label => "Processed", status_explanation => "NORMAL DEFINITION NOT REPEATED" },
                    context! { status => "failed", status_label => "Failed", error => "ACTUAL REFLECTION ERROR", needs_attention => true, retry_href => "conversations/conv/reflect" },
                    context! { status => "skipped", status_label => "Skipped", skipped_reason => "ACTUAL SKIP REASON", status_explanation => "ACTUAL SKIP EXPLANATION" },
                ],
            },
        );
        for text in [
            "ACTUAL REFLECTION ERROR",
            "ACTUAL SKIP REASON",
            "ACTUAL SKIP EXPLANATION",
            "Retry now",
        ] {
            assert_visible(&html, text);
        }
        assert!(!html.contains("NORMAL DEFINITION NOT REPEATED"));
        assert!(html.contains("Help: reflection statuses"));
    }
}

#[test]
fn policy_preference_is_not_permission_and_risk_warning_is_visible() {
    let html = render(
        "bear/settings/policy.html",
        context! { can_manage_bear => true, hats_configured => true },
    );
    assert_visible(&html, "Preferred changes source ordering, not permission");
    assert_visible(
        &html,
        "Combining private data, external content and outbound actions",
    );
    assert!(html.contains("aria-describedby=\"source-policy-effects\""));
}

#[test]
fn advanced_activity_and_reconsideration_keep_real_feedback_and_repair() {
    let advanced = render(
        "bear/settings/advanced.html",
        context! { can_manage_bear => true, message => "SETTINGS SAVED" },
    );
    assert_visible(&advanced, "SETTINGS SAVED");
    assert_visible(&advanced, "Inspect reflection outcomes and retry failures");
    assert_visible(&advanced, "Memory file statistics unavailable");
    let activity = render(
        "bear/settings/conversations.html",
        context! {
            message => "REFLECTION SAVED", error => "REFLECTION FAILED",
            live_reflection_status => context! { enabled => true, workers_enabled => false, status_label => "Worker not running", status_explanation => "RUN_WORKERS REPAIR" },
        },
    );
    for text in [
        "REFLECTION SAVED",
        "REFLECTION FAILED",
        "RUN_WORKERS REPAIR",
        "Live reflection settings",
    ] {
        assert_visible(&activity, text);
    }
    assert!(!activity.contains("sandbox is not built yet"));
    assert!(!activity.contains("when Cabinet is deployed"));
    let result = render(
        "bear/settings/reconsider_result.html",
        context! {
            conv => context! { id => "conv" }, result => context! { compaction_status => "failed",
                compaction_diagnostic => "REAL COMPACTION FAILURE", skipped_reason => "REAL RECONSIDERATION SKIP", proposals_created => 0 },
        },
    );
    assert_visible(&result, "REAL COMPACTION FAILURE");
    assert_visible(&result, "REAL RECONSIDERATION SKIP");
}

#[test]
fn proposal_content_sensitivity_and_preserved_draft_are_not_hidden_in_details() {
    let html = render(
        "bear/memory_proposal.html",
        context! {
            can_manage_bear => true, errors => "SAVE NOT CONFIRMED", saved => false,
            proposal => context! { id => "proposal", title => "Proposal", status => "pending", sensitivity => "private",
                requires_human => true, proposed_content => "REVIEW CONTENT", rationale => "REVIEW RATIONALE",
                source_refs => context! { conversation_id => "conversation" }, extraction => context! { is_memory_extraction => true } },
            form => context! { status => "rejected", decision_summary => "PRESERVED SUMMARY <unsafe>", review_notes => "PRESERVED REVIEW NOTES" },
        },
    );
    for text in [
        "SAVE NOT CONFIRMED",
        "REVIEW CONTENT",
        "REVIEW RATIONALE",
        "requires human review",
        "PRESERVED SUMMARY",
        "PRESERVED REVIEW NOTES",
        "only triage",
    ] {
        assert_visible(&html, text);
    }
    assert!(html.contains("value=\"rejected\" selected"));
    assert!(html.contains("PRESERVED SUMMARY &lt;unsafe&gt;"));
    assert!(html.contains("/conversations/conversation"));
}
