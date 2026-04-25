pub mod time;

use ammonia;
use minijinja::Value;
use pulldown_cmark::{Options, Parser, html::push_html};

pub fn hexadecimal(value: Value) -> String {
    if let Some(number) = value.as_i64() {
        format!("{number:x}")
    } else {
        "NaN".to_string()
    }
}

pub fn markdown(value: Value) -> Value {
    if let Some(markdown) = value.as_str() {
        let md_parser = Parser::new_ext(markdown, Options::all());
        let mut unsafe_html: String = String::with_capacity(markdown.len() * 3 / 2);
        push_html(&mut unsafe_html, md_parser);

        let mut cleaner = ammonia::Builder::default();
        cleaner.set_tag_attribute_value("a", "target", "_blank");

        let safe_html = cleaner.clean(&unsafe_html).to_string();

        Value::from_safe_string(safe_html)
    } else {
        Value::from_safe_string("INVALID".to_string())
    }
}
