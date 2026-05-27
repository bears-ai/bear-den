pub(in crate::api::acp) fn find_sse_frame_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(in crate::api::acp) fn strip_trailing_sse_delimiter_owned(mut frame: Vec<u8>) -> Vec<u8> {
    if frame.ends_with(b"\r\n\r\n") {
        frame.truncate(frame.len().saturating_sub(4));
    } else if frame.ends_with(b"\n\n") {
        frame.truncate(frame.len().saturating_sub(2));
    }
    frame
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) fn strip_trailing_sse_delimiter(frame: &[u8]) -> &[u8] {
    if frame.ends_with(b"\r\n\r\n") {
        &frame[..frame.len().saturating_sub(4)]
    } else if frame.ends_with(b"\n\n") {
        &frame[..frame.len().saturating_sub(2)]
    } else {
        frame
    }
}

pub(in crate::api::acp) fn parse_sse_event_body_to_json(body: &[u8]) -> Result<Option<serde_json::Value>, String> {
    let body = String::from_utf8_lossy(body);
    let mut data_lines = Vec::new();
    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            data_lines.push(rest);
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let joined = data_lines.join("\n");
    serde_json::from_str::<serde_json::Value>(&joined)
        .map(Some)
        .map_err(|err| {
            let summary = super::logging::summarize_json_parse_error_for_log(&joined);
            format!("{err}; body_preview={summary}")
        })
}
