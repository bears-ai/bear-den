//! Boundary projection for settings inspection. The parent settings handler can
//! use this instead of classifying rendered explanations or exposing normal definitions.

use serde::Serialize;

pub(crate) fn read_result<T, E: std::fmt::Display>(
    result: Result<T, E>,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(format!("{label} unavailable: {error}"));
            None
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PartialList<T> {
    pub items: Vec<T>,
    pub complete: bool,
}

impl<T> PartialList<T> {
    pub(crate) fn combine<E: std::fmt::Display>(
        first: Result<Vec<T>, E>,
        second: Result<Vec<T>, E>,
        labels: [&str; 2],
        errors: &mut Vec<String>,
    ) -> Self {
        let first = read_result(first, labels[0], errors);
        let second = read_result(second, labels[1], errors);
        let complete = first.is_some() && second.is_some();
        let items = first
            .into_iter()
            .flatten()
            .chain(second.into_iter().flatten())
            .collect();
        Self { items, complete }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ReflectionFeedback {
    pub status_label: String,
    pub status_explanation: String,
    pub error: Option<String>,
    pub needs_attention: bool,
}

#[derive(Debug)]
enum Outcome {
    Processed,
    Skipped,
    Failed,
    Unknown,
}

impl Outcome {
    fn parse(status: Option<&str>) -> Self {
        match status {
            Some("processed") => Self::Processed,
            Some("skipped") => Self::Skipped,
            Some("failed" | "error") => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug)]
enum Skip {
    BelowThreshold,
    NoNewContent,
    Disabled,
    NoArtifact,
    Other(String),
}

impl Skip {
    fn parse(reason: &str) -> Self {
        match reason {
            "below_compaction_threshold" => Self::BelowThreshold,
            "no_uncompacted_content" => Self::NoNewContent,
            "live_reflection_disabled" => Self::Disabled,
            "no_compaction_artifact" => Self::NoArtifact,
            other => Self::Other(other.to_string()),
        }
    }
}

// Integration point for `settings::reflection_rows_for_bear`: pass the recorded
// status, skip reason and optional error field before serializing the row.
#[allow(dead_code)] // Parent settings.rs integration is outside this change's write scope.
pub(crate) fn reflection_feedback(
    status: Option<&str>,
    skipped_reason: Option<&str>,
    recorded_error: Option<&str>,
) -> ReflectionFeedback {
    let outcome = Outcome::parse(status);
    let error = recorded_error
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string);
    if matches!(outcome, Outcome::Failed) || error.is_some() {
        return ReflectionFeedback {
            status_label: "Failed".into(),
            status_explanation: error.clone().unwrap_or_else(|| {
                "Reflection failed; inspect the recorded payload before retrying.".into()
            }),
            error,
            needs_attention: true,
        };
    }
    let (label, explanation, needs_attention) = match skipped_reason.map(Skip::parse) {
        Some(Skip::BelowThreshold) => (
            "Below threshold",
            "Normal compaction policy skipped this sweep.",
            false,
        ),
        Some(Skip::NoNewContent) => (
            "No new content",
            "The checkpoint already covers the available transcript.",
            false,
        ),
        Some(Skip::Disabled) => (
            "Live reflection disabled",
            "Enable live reflection in Advanced settings to run live sweeps.",
            false,
        ),
        Some(Skip::NoArtifact) => (
            "Not inspected",
            "No checkpoint was available. Retry manual reflection to request a fresh checkpoint.",
            true,
        ),
        Some(Skip::Other(reason)) => {
            return ReflectionFeedback {
                status_label: "Skipped".into(),
                status_explanation: format!("Reflection skipped: {reason}."),
                error: None,
                needs_attention: true,
            };
        }
        None => match outcome {
            Outcome::Processed => ("Processed", "The recorded checkpoint was inspected.", false),
            Outcome::Skipped => (
                "Skipped",
                "No skip reason was recorded; inspect the payload.",
                true,
            ),
            Outcome::Unknown => (
                "Unknown",
                "No recognized outcome was recorded; inspect the payload.",
                true,
            ),
            Outcome::Failed => unreachable!(),
        },
    };
    ReflectionFeedback {
        status_label: label.into(),
        status_explanation: explanation.into(),
        error,
        needs_attention,
    }
}

#[cfg(test)]
#[path = "inspection_tests.rs"]
mod tests;
