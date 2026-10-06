//! Untrusted MIME declarations become a typed, conservative preview decision.

use serde::Serialize;

pub const MAX_TEXT_PREVIEW_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Media {
    Text,
    Json,
    Png,
    Jpeg,
    Gif,
    Webp,
    Pdf,
    DownloadOnly,
}

impl Media {
    pub fn parse(value: Option<&str>) -> Self {
        let essence = value
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "application/json" => Self::Json,
            "image/png" => Self::Png,
            "image/jpeg" => Self::Jpeg,
            "image/gif" => Self::Gif,
            "image/webp" => Self::Webp,
            "application/pdf" => Self::Pdf,
            // These are shown only as escaped source, never active documents.
            "application/xml"
            | "application/javascript"
            | "application/xhtml+xml"
            | "image/svg+xml" => Self::Text,
            other if other.starts_with("text/") => Self::Text,
            _ => Self::DownloadOnly,
        }
    }

    pub fn inline_type(self, bytes: &[u8]) -> Option<&'static str> {
        match self {
            Self::Png if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
            Self::Jpeg if bytes.starts_with(b"\xff\xd8\xff") => Some("image/jpeg"),
            Self::Gif if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => {
                Some("image/gif")
            }
            Self::Webp if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => {
                Some("image/webp")
            }
            Self::Pdf if bytes.starts_with(b"%PDF-") => Some("application/pdf"),
            _ => None,
        }
    }

    pub fn text(self, bytes: &[u8]) -> Option<TextPreview> {
        if !matches!(self, Self::Text | Self::Json) {
            return None;
        }
        let source = std::str::from_utf8(bytes).ok()?;
        let formatted = if self == Self::Json && bytes.len() <= MAX_TEXT_PREVIEW_BYTES {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
        } else {
            None
        };
        let source = formatted.as_deref().unwrap_or(source);
        let mut end = source.len().min(MAX_TEXT_PREVIEW_BYTES);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        Some(TextPreview {
            content: source[..end].into(),
            truncated: end < source.len(),
        })
    }
}

#[derive(Serialize)]
pub struct TextPreview {
    pub content: String,
    pub truncated: bool,
}
