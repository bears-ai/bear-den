//! Test-only HTML assertions compare decoded attributes, not serialization spelling.

use std::collections::BTreeMap;

use regex::Regex;

pub(crate) struct OpeningTag {
    attributes: BTreeMap<String, String>,
}

impl OpeningTag {
    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    pub(crate) fn has_attribute(&self, name: &str) -> bool {
        self.attributes.contains_key(name)
    }
}

fn decode_entities(text: &str) -> String {
    let entities = Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|amp|lt|gt|quot|apos|nbsp);").unwrap();
    entities
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let entity = &captures[1];
            let character = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .and_then(|number| u32::from_str_radix(number, 16).ok())
                    .or_else(|| {
                        entity
                            .strip_prefix('#')
                            .and_then(|number| number.parse::<u32>().ok())
                    })
                    .and_then(char::from_u32),
            };
            character
                .map(|character| character.to_string())
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

fn attributes(source: &str) -> OpeningTag {
    let pattern =
        Regex::new(r#"([^\s=/'\"<>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?"#).unwrap();
    let attributes = pattern
        .captures_iter(source)
        .map(|captures| {
            let value = captures
                .get(2)
                .or_else(|| captures.get(3))
                .or_else(|| captures.get(4))
                .map(|value| decode_entities(value.as_str()))
                .unwrap_or_default();
            (captures[1].to_ascii_lowercase(), value)
        })
        .collect();
    OpeningTag { attributes }
}

pub(crate) fn opening_tag(html: &str, tag: &str, name: &str, value: &str) -> OpeningTag {
    let pattern = Regex::new(&format!(r"(?is)<{}\b([^<>]*)>", regex::escape(tag))).unwrap();
    let matches: Vec<_> = pattern
        .captures_iter(html)
        .map(|captures| attributes(&captures[1]))
        .filter(|element| element.attribute(name) == Some(value))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one <{tag}> with {name}={value:?}"
    );
    matches.into_iter().next().unwrap()
}

pub(crate) fn assert_link(html: &str, href: &str, label: &str) {
    let pattern = Regex::new(r"(?is)<a\b([^<>]*)>(.*?)</a\s*>").unwrap();
    let markup = Regex::new(r"(?s)<[^>]*>").unwrap();
    assert!(
        pattern.captures_iter(html).any(|captures| {
            let element = attributes(&captures[1]);
            let text = decode_entities(&markup.replace_all(&captures[2], ""));
            element.attribute("href") == Some(href)
                && text.split_whitespace().collect::<Vec<_>>().join(" ") == label
        }),
        "missing link {label:?} to {href:?}"
    );
}

pub(crate) fn assert_no_link(html: &str, href: &str) {
    let pattern = Regex::new(r"(?is)<a\b([^<>]*)>").unwrap();
    assert!(
        !pattern
            .captures_iter(html)
            .any(|captures| attributes(&captures[1]).attribute("href") == Some(href)),
        "unexpected link to {href:?}"
    );
}

#[test]
fn assertions_preserve_attribute_meaning_across_escaping_and_whitespace() {
    let html = "<a\n href='&#x2f;admin&#x2f;users&#x2f;'><strong>Users</strong></a><input name='paths' value='pair&#x2f;note.md'\n checked>";
    assert_link(html, "/admin/users/", "Users");
    assert_no_link(html, "/admin/models");
    let input = opening_tag(html, "input", "name", "paths");
    assert_eq!(input.attribute("value"), Some("pair/note.md"));
    assert!(input.has_attribute("checked"));
}
