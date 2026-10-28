//! Leases survive BearWire client waits, but never the owning process. Live
//! streams retain an Arc after cancellation removes the waiting-run entry.
use den_core::DenError;
use den_service::tool_turns::ConversationTurnLease;
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
#[derive(Hash, PartialEq, Eq)]
struct RunKey {
    session: String,
    run: String,
}
static LEASES: LazyLock<Mutex<HashMap<RunKey, Arc<ConversationTurnLease>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) fn insert(session: &str, run: &str, lease: Arc<ConversationTurnLease>) {
    LEASES.lock().expect("source lease registry").insert(
        RunKey {
            session: session.into(),
            run: run.into(),
        },
        lease,
    );
}
pub(crate) fn get(session: &str, run: Option<&str>) -> Option<Arc<ConversationTurnLease>> {
    LEASES
        .lock()
        .ok()?
        .get(&RunKey {
            session: session.into(),
            run: run?.into(),
        })
        .cloned()
}
pub(crate) async fn acquire(
    pool: &PgPool,
    session: &crate::agent_loop::AgentLoopSession,
) -> Result<Arc<ConversationTurnLease>, DenError> {
    if let Some(lease) = get(&session.client_session_id, session.run_id.as_deref()) {
        return Ok(lease);
    }
    let canonical = den_service::conversation::persistence::get_conversation_for_external_id(
        pool,
        session.bear_id,
        &session.conversation_id,
    )
    .await?
    .ok_or_else(|| DenError::Authorization("canonical continuation source is missing".into()))?;
    ConversationTurnLease::acquire(pool, canonical.id).await
}

pub(super) fn remove(session: &str, run: &str) {
    if let Ok(mut leases) = LEASES.lock() {
        leases.remove(&RunKey {
            session: session.into(),
            run: run.into(),
        });
    }
}
