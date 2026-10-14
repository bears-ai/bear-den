//! Cabinet-specific template and authenticated HTTP regression coverage.

mod cross_surface;
mod routes;
mod templates;

// MiniJinja escapes slashes too. Normalize that serialization spelling only;
// keep angle brackets encoded so XSS assertions still inspect safe markup.
fn normalize_markup(html: String) -> String {
    html.replace("&#x2f;", "/")
}
