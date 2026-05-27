pub(super) fn acp_max_thought_bytes_per_turn() -> usize {
    std::env::var("BEARS_ACP_MAX_THOUGHT_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1024, 1024 * 1024))
        .unwrap_or(128 * 1024)
}

pub(super) fn should_flush_text(buffer: &str, max_chars: usize) -> bool {
    buffer.chars().count() >= max_chars
        || buffer.ends_with('\n')
        || buffer.ends_with(". ")
        || buffer.ends_with("! ")
        || buffer.ends_with("? ")
}

pub(super) fn truncate_utf8_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
