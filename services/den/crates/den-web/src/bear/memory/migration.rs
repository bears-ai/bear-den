//! Migration failures stay on the authorized action page, never in redirect URLs.

use super::{dashboard_view, AppState, CustomError, DashboardQuery, Path, Query, Response, State};

pub(super) async fn failure(
    state: AppState,
    auth: crate::auth_backend::AuthSession,
    slug: &str,
    message: String,
) -> Result<Response, CustomError> {
    dashboard_view(
        Path(slug.to_string()),
        Query(DashboardQuery {
            import_error: Some(message),
            ..DashboardQuery::default()
        }),
        State(state),
        auth,
    )
    .await
}
