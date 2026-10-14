// Exercise the adapter slice independently of the parent router composition.
// Ignored SQLx tests create isolated databases; the standalone SDK probe reuses
// bear-armature's existing rmcp artifacts without a den-web dependency.
pub use den_web::AppState;
#[path = "../src/cabinet_mcp.rs"]
mod cabinet_mcp;

#[test]
fn adapter_exports_the_parent_composition_router() {
    let _: axum::Router<AppState> = cabinet_mcp::router();
}
