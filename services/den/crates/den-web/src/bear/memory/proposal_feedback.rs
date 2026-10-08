//! Proposal reads and recoverable form feedback share the same authorized projection.

use super::{
    bear_nav_context, context, get_sqlite_memory_proposal, memory_proposals,
    proposal_view_from_postgres, proposal_view_from_sqlite, web, AppState, CustomError,
    Deserialize, MemoryProposalResolutionForm, MemoryProposalView, Response, Uuid,
};

#[derive(Default, Deserialize)]
pub(super) struct ProposalQuery {
    #[serde(default)]
    pub saved: bool,
}

pub(super) async fn load_proposal(
    state: &AppState,
    bear_id: Uuid,
    proposal_id: Uuid,
) -> Result<MemoryProposalView, CustomError> {
    if let Some(proposal) =
        memory_proposals::get_for_bear(state.sqlx_pool(), bear_id, proposal_id).await?
    {
        return Ok(proposal_view_from_postgres(proposal));
    }
    let store = state.memory_stores.store_for_bear(bear_id).await?;
    get_sqlite_memory_proposal(&store, &proposal_id.to_string())
        .await?
        .map(proposal_view_from_sqlite)
        .ok_or_else(|| CustomError::NotFound("memory proposal not found".to_string()))
}

pub(super) async fn render_error(
    state: &AppState,
    auth_session: crate::auth_backend::AuthSession,
    bear: &den_service::bears::Bear,
    proposal: MemoryProposalView,
    form: MemoryProposalResolutionForm,
    error: String,
) -> Result<Response, CustomError> {
    web::render_template(
        state,
        "bear/memory_proposal.html",
        auth_session,
        context! {
            proposal,
            form,
            errors => error,
            can_manage_bear => true,
            ..bear_nav_context(bear, "memory"),
        },
    )
    .await
}
