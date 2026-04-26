//! In-memory counters exposed as Prometheus text (no external metrics crates).
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static CHAT_SEND_STARTED: AtomicU64 = AtomicU64::new(0);
static CHAT_SEND_FINISHED_OK: AtomicU64 = AtomicU64::new(0);
static CHAT_SEND_FINISHED_EMPTY: AtomicU64 = AtomicU64::new(0);
static CHAT_SEND_FINISHED_PROXY_ERROR: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn chat_send_started() {
    CHAT_SEND_STARTED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn chat_send_finished_ok() {
    CHAT_SEND_FINISHED_OK.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn chat_send_finished_empty() {
    CHAT_SEND_FINISHED_EMPTY.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn chat_send_finished_proxy_error() {
    CHAT_SEND_FINISHED_PROXY_ERROR.fetch_add(1, Ordering::Relaxed);
}

/// Prometheus text exposition 0.0.4.
pub fn render_prometheus_text() -> String {
    let mut s = String::with_capacity(512);
    let a = CHAT_SEND_STARTED.load(Ordering::Relaxed);
    let b = CHAT_SEND_FINISHED_OK.load(Ordering::Relaxed);
    let c = CHAT_SEND_FINISHED_EMPTY.load(Ordering::Relaxed);
    let d = CHAT_SEND_FINISHED_PROXY_ERROR.load(Ordering::Relaxed);

    writeln!(
        s,
        "# HELP den_chat_send_started_total Chat POST /v1/chat/send requests that passed auth and received an upstream Codepool streaming response (SSE proxy about to start)."
    )
    .unwrap();
    writeln!(s, "# TYPE den_chat_send_started_total counter").unwrap();
    writeln!(s, "den_chat_send_started_total {a}").unwrap();

    writeln!(
        s,
        "# HELP den_chat_send_finished_ok_total SSE streams from Codepool that forwarded at least one byte."
    )
    .unwrap();
    writeln!(s, "# TYPE den_chat_send_finished_ok_total counter").unwrap();
    writeln!(s, "den_chat_send_finished_ok_total {b}").unwrap();

    writeln!(
        s,
        "# HELP den_chat_send_finished_empty_upstream_total SSE streams that ended with zero bytes from Codepool."
    )
    .unwrap();
    writeln!(
        s,
        "# TYPE den_chat_send_finished_empty_upstream_total counter"
    )
    .unwrap();
    writeln!(s, "den_chat_send_finished_empty_upstream_total {c}").unwrap();

    writeln!(
        s,
        "# HELP den_chat_send_finished_proxy_error_total SSE proxy failures (chunk error or drop before completion)."
    )
    .unwrap();
    writeln!(s, "# TYPE den_chat_send_finished_proxy_error_total counter").unwrap();
    writeln!(s, "den_chat_send_finished_proxy_error_total {d}").unwrap();

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_text_has_type_and_help_lines() {
        let s = render_prometheus_text();
        assert!(s.contains("# HELP den_chat_send_started_total"));
        assert!(s.contains("# TYPE den_chat_send_started_total counter"));
        assert!(s.contains("den_chat_send_started_total "));
    }
}
