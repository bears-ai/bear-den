/// Normalized claim comparison for proposal freshness and consolidation. A
/// matching fingerprint suggests a possible duplicate, not permission to share.
pub(crate) fn memory_claim_fingerprint(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_fingerprint_normalizes_case_punctuation_and_whitespace() {
        assert_eq!(
            memory_claim_fingerprint("  Use BearWire  for armature transport.\n"),
            memory_claim_fingerprint("use bearwire for armature transport")
        );
        assert_ne!(
            memory_claim_fingerprint("Use BearWire for armature transport"),
            memory_claim_fingerprint("Do not use BearWire for armature transport")
        );
    }
}
