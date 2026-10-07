//! Advisory lifecycle state projected by Den; local reservations never grant authority.

use super::*;
use bearwire_protocol::session::{SessionAccess, SessionAccessState};
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
pub(crate) struct DenSessionProjection {
    pub client_session_id: String,
    pub conversation_id: Option<String>,
    pub resolved_conversation_id: Option<String>,
    pub history_conversation_id: Option<String>,
    pub access: SessionAccess,
}

fn validate_opaque_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.trim() != id || id.chars().any(char::is_control) {
        bail!("Den returned an invalid opaque session/conversation identifier");
    }
    Ok(())
}

impl DenSessionProjection {
    pub(crate) fn parse(session_id: &str, session: &Value) -> Result<Self> {
        let projection: Self = serde_json::from_value(session.clone())
            .context("decode Den session access projection")?;
        validate_opaque_id(&projection.client_session_id)?;
        if projection.client_session_id != session_id {
            bail!("Den session projection does not match the requested client session");
        }
        for id in [
            projection.conversation_id.as_deref(),
            projection.resolved_conversation_id.as_deref(),
            projection.history_conversation_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            validate_opaque_id(id)?;
        }
        match projection.access.state {
            SessionAccessState::AwaitingHat => {
                if projection.resolved_conversation_id.is_some()
                    || projection.history_conversation_id.is_some()
                {
                    bail!("pending Den session has a durable history binding");
                }
            }
            SessionAccessState::Executable => {
                if projection.history_conversation_id.is_none()
                    || projection.history_conversation_id.as_deref() != projection.canonical_id()
                {
                    bail!("executable Den session has no consistent canonical history binding");
                }
            }
            SessionAccessState::ReadOnly => {
                if projection.access.may_select_hat {
                    bail!("read-only Den history cannot offer hat selection");
                }
            }
        }
        Ok(projection)
    }

    pub(crate) fn canonical_id(&self) -> Option<&str> {
        self.resolved_conversation_id
            .as_deref()
            .or(self.conversation_id.as_deref())
    }

    fn apply(&self, context: &mut SessionContext, session: &Value) {
        context.conversation_id.clone_from(&self.conversation_id);
        context
            .resolved_conversation_id
            .clone_from(&self.resolved_conversation_id);
        context.access = Some(self.access);
        context.thread_title = den_session_display_title(session);
        if !context.raw.is_object() {
            context.raw = json!({});
        }
        context.raw["den_acp_session"] = session.clone();
    }
}

pub(crate) async fn apply_den_session_projection(
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    session: &Value,
) -> Result<()> {
    let projection = DenSessionProjection::parse(session_id, session)?;
    let mut contexts = shared_state.session_contexts.lock().await;
    let mut context = contexts
        .get(session_id)
        .or_else(|| adapter_state.session_contexts.get(session_id))
        .cloned()
        .ok_or_else(|| anyhow!("cannot project Den state onto an unknown local session"))?;
    // A spawned prompt may carry an older local snapshot. Preserve the shared
    // reservation, so projecting a response cannot unlock a competing selection.
    if let Some(shared) = contexts.get(session_id) {
        context.interaction_reservation = shared.interaction_reservation.clone();
    }
    advance_session_generation(&context)?;
    projection.apply(&mut context, session);
    contexts.insert(session_id.to_string(), context.clone());
    adapter_state
        .session_contexts
        .insert(session_id.to_string(), context);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReservationKind {
    InitialInteraction,
    Restore,
}

#[derive(Clone, Copy, Debug)]
struct Reservation {
    token: Uuid,
    kind: ReservationKind,
}

#[derive(Debug, Default)]
pub(crate) struct SessionInteractionState {
    reservation: Option<Reservation>,
    // Unique epochs prevent ABA when a source changes and later changes back.
    generation: Uuid,
}

impl SessionInteractionState {
    #[cfg(test)]
    pub(crate) fn is_none(&self) -> bool {
        self.reservation.is_none()
    }
}

pub(crate) type InteractionReservationState = Arc<Mutex<SessionInteractionState>>;

pub(crate) fn advance_session_generation(context: &SessionContext) -> Result<()> {
    context
        .interaction_reservation
        .lock()
        .map_err(|_| anyhow!("session reservation poisoned"))?
        .generation = Uuid::new_v4();
    Ok(())
}

pub(crate) struct SessionInteractionReservation {
    state: InteractionReservationState,
    token: Uuid,
}

impl Drop for SessionInteractionReservation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .reservation
                .is_some_and(|reservation| reservation.token == self.token)
            {
                state.reservation = None;
            }
        }
    }
}

pub(super) struct SessionRestoreGuard {
    session_id: String,
    reservation: SessionInteractionReservation,
    generation: Uuid,
    known: bool,
    was_prompted: bool,
}

impl SessionRestoreGuard {
    fn validate(
        &self,
        contexts: &HashMap<String, SessionContext>,
        prompted: &HashSet<String>,
        state: &SessionInteractionState,
    ) -> Result<()> {
        let same_session = match (self.known, contexts.get(&self.session_id)) {
            (true, Some(context)) => {
                Arc::ptr_eq(&context.interaction_reservation, &self.reservation.state)
            }
            (false, None) => true,
            _ => false,
        };
        if !same_session
            || state.generation != self.generation
            || state
                .reservation
                .is_none_or(|reservation| reservation.token != self.reservation.token)
            || prompted.contains(&self.session_id) != self.was_prompted
        {
            bail!("Session changed while history was being restored. Retry load/resume against Den's current state.");
        }
        Ok(())
    }

    pub(super) async fn check_current(&self, shared_state: &AdapterSharedState) -> Result<()> {
        let contexts = shared_state.session_contexts.lock().await;
        let prompted = shared_state.prompted_sessions.lock().await;
        let state = self
            .reservation
            .state
            .lock()
            .map_err(|_| anyhow!("session reservation poisoned"))?;
        self.validate(&contexts, &prompted, &state)
    }
}

pub(super) async fn begin_session_restore(
    shared_state: &AdapterSharedState,
    session_id: &str,
) -> Result<SessionRestoreGuard> {
    let contexts = shared_state.session_contexts.lock().await;
    let prompted = shared_state.prompted_sessions.lock().await;
    let known = contexts.get(session_id);
    let state = known
        .map(|context| context.interaction_reservation.clone())
        .unwrap_or_default();
    let token = Uuid::new_v4();
    let generation = {
        let mut interaction = state
            .lock()
            .map_err(|_| anyhow!("session reservation poisoned"))?;
        if interaction.reservation.is_some() {
            bail!("A session interaction or restore is in progress. Retry load/resume after it finishes.");
        }
        interaction.reservation = Some(Reservation {
            token,
            kind: ReservationKind::Restore,
        });
        interaction.generation
    };
    Ok(SessionRestoreGuard {
        session_id: session_id.to_string(),
        reservation: SessionInteractionReservation { state, token },
        generation,
        known: known.is_some(),
        was_prompted: prompted.contains(session_id),
    })
}

#[derive(Clone, Copy)]
pub(crate) enum SessionInteractionKind {
    Productive,
    SelectHat,
    Configure,
}

pub(crate) async fn require_session_interaction(
    _adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    kind: SessionInteractionKind,
) -> Result<SessionContext> {
    let context = shared_state
        .session_contexts
        .lock()
        .await
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            anyhow!("Unknown ACP session. Create or successfully load a session first.")
        })?;
    // Missing shared state can mean this session was closed while an older
    // adapter snapshot survived. That snapshot must not reopen it implicitly.
    validate_interaction_access(&context, kind)?;
    {
        let state = context
            .interaction_reservation
            .lock()
            .map_err(|_| anyhow!("session reservation poisoned"))?;
        if state
            .reservation
            .is_some_and(|reservation| reservation.kind == ReservationKind::Restore)
        {
            bail!("Session history is being restored. Retry this interaction after load/resume finishes.");
        }
    }
    Ok(context)
}

fn validate_interaction_access(
    context: &SessionContext,
    kind: SessionInteractionKind,
) -> Result<()> {
    let access = context.access.ok_or_else(|| {
        anyhow!("Session access is not verified by Den. Successfully load or create a Den session first.")
    })?;
    match (access.state, kind) {
        (SessionAccessState::ReadOnly, _) => {
            bail!(
                "This session is read-only history. Start a new conversation to work with a hat."
            );
        }
        (SessionAccessState::AwaitingHat, SessionInteractionKind::Productive) => {
            bail!("This session is awaiting a hat. Use /hat to list hats, then /hat <name or id> before a productive turn.");
        }
        (_, SessionInteractionKind::SelectHat) if !access.may_select_hat => {
            bail!("The hat is fixed for this conversation. Start a new conversation to wear another hat.");
        }
        _ => {}
    }
    Ok(())
}

pub(crate) async fn reserve_session_interaction(
    _adapter_state: &AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    kind: SessionInteractionKind,
) -> Result<Option<SessionInteractionReservation>> {
    let contexts = shared_state.session_contexts.lock().await;
    let context = contexts.get(session_id).ok_or_else(|| {
        anyhow!("Unknown ACP session. Create or successfully load a session first.")
    })?;
    validate_interaction_access(context, kind)?;
    let completed = shared_state.prompted_sessions.lock().await;
    let state = context.interaction_reservation.clone();
    let mut reservation = state
        .lock()
        .map_err(|_| anyhow!("session reservation poisoned"))?;
    if reservation
        .reservation
        .is_some_and(|reservation| reservation.kind == ReservationKind::Restore)
    {
        bail!(
            "Session history is being restored. Retry this interaction after load/resume finishes."
        );
    }
    let already_succeeded = completed.contains(session_id);
    if matches!(kind, SessionInteractionKind::SelectHat) && already_succeeded {
        bail!("The initial hat selection or productive interaction has already succeeded. Start a new conversation to wear another hat.");
    }
    // Once admission has closed selection, steering prompts need not wait for
    // the preceding model turn. Only the initial prompt/selection race is fenced.
    if !matches!(kind, SessionInteractionKind::SelectHat)
        && (already_succeeded || context.access.is_some_and(|access| !access.may_select_hat))
    {
        return Ok(None);
    }
    let token = Uuid::new_v4();
    if reservation.reservation.is_some() {
        bail!("An initial prompt or hat selection is in progress. Retry after it finishes.");
    }
    reservation.reservation = Some(Reservation {
        token,
        kind: ReservationKind::InitialInteraction,
    });
    drop(reservation);
    Ok(Some(SessionInteractionReservation { state, token }))
}

pub(crate) async fn mark_session_productive_interaction(
    shared_state: &AdapterSharedState,
    session_id: &str,
) {
    let contexts = shared_state.session_contexts.lock().await;
    shared_state
        .prompted_sessions
        .lock()
        .await
        .insert(session_id.to_string());
    if let Some(context) = contexts.get(session_id) {
        if let Ok(mut state) = context.interaction_reservation.lock() {
            state.generation = Uuid::new_v4();
            if state
                .reservation
                .is_some_and(|reservation| reservation.kind == ReservationKind::InitialInteraction)
            {
                state.reservation = None;
            }
        }
    }
}

pub(crate) fn session_access_status(context: &SessionContext) -> &'static str {
    match context.access.map(|access| access.state) {
        Some(SessionAccessState::AwaitingHat) => {
            "Awaiting a hat; use /hat before a productive turn."
        }
        Some(SessionAccessState::Executable) => {
            "Den projects this session as executable; authority is rechecked on every effect."
        }
        Some(SessionAccessState::ReadOnly) => {
            "Read-only history; inspection does not grant execution as its owner."
        }
        None => "Session access has not been verified by Den.",
    }
}

pub(crate) fn with_session_access_metadata(mut result: Value, context: &SessionContext) -> Value {
    if !result.get("_meta").is_some_and(Value::is_object) {
        result["_meta"] = json!({});
    }
    if !result.pointer("/_meta/bears").is_some_and(Value::is_object) {
        result["_meta"]["bears"] = json!({});
    }
    result["_meta"]["bears"]["access"] = json!(context.access);
    result["_meta"]["bears"]["status"] = json!(session_access_status(context));
    if context
        .access
        .is_some_and(|access| access.state == SessionAccessState::ReadOnly)
    {
        result["configOptions"] = json!([]);
        result
            .as_object_mut()
            .expect("ACP lifecycle result object")
            .remove("modes");
    }
    result
}

pub(crate) fn validated_conversation_id(id: &str) -> Result<&str> {
    validate_opaque_id(id)?;
    Ok(id)
}

pub(super) async fn commit_restored_session(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    restore: &SessionRestoreGuard,
    den: &Value,
    mut context: SessionContext,
) -> Result<()> {
    let session_id = restore.session_id.as_str();
    let projection = DenSessionProjection::parse(session_id, den)?;
    context.interaction_reservation = restore.reservation.state.clone();
    {
        // Keep the final MCP install, cache replacement and eligibility reset
        // atomic with source updates. These are Tokio guards; the short sync
        // reservation guard is released before any await.
        let mut contexts = shared_state.session_contexts.lock().await;
        let mut prompted = shared_state.prompted_sessions.lock().await;
        {
            let state = restore
                .reservation
                .state
                .lock()
                .map_err(|_| anyhow!("session reservation poisoned"))?;
            restore.validate(&contexts, &prompted, &state)?;
        }
        context.raw["mcp"] = shared_state
            .mcp_registry
            .configure_session(session_id, context.mcp_sources.clone())
            .await?;
        ensure_session_context_capabilities(&mut context);
        projection.apply(&mut context, den);
        let mut state = restore
            .reservation
            .state
            .lock()
            .map_err(|_| anyhow!("session reservation poisoned"))?;
        restore.validate(&contexts, &prompted, &state)?;
        state.generation = Uuid::new_v4();
        contexts.insert(session_id.to_string(), context.clone());
        adapter_state
            .session_contexts
            .insert(session_id.to_string(), context);
        if projection.access.may_select_hat {
            prompted.remove(session_id);
        }
    }
    spawn_adapter_environment_publish(
        config.clone(),
        session_id.to_string(),
        adapter_state.clone(),
        None,
    );
    Ok(())
}

pub(super) async fn restore_session_from_den(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    params: &Value,
) -> Result<(&'static str, Option<Value>, Option<Value>)> {
    let session_id = session_id_from_config_params(params)?;
    let restore = begin_session_restore(shared_state, session_id).await?;
    let den = den_get_acp_session_for_lifecycle(http, config, session_id).await?;
    restore.check_current(shared_state).await?;
    DenSessionProjection::parse(session_id, &den)?;
    let context = session_context_from_den_session(params, &den)?;
    commit_restored_session(config, adapter_state, shared_state, &restore, &den, context).await?;
    Ok((
        infer_mode_from_den_session(&den),
        den.get("context_budget").cloned(),
        Some(den),
    ))
}

pub(super) async fn handle_session_load(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response_id: Value,
    params: &Value,
) -> Result<()> {
    let session_id = session_id_from_config_params(params)?;
    let restore = begin_session_restore(shared_state, session_id).await?;
    let den = den_get_acp_session_for_lifecycle(http, config, session_id).await?;
    restore.check_current(shared_state).await?;
    DenSessionProjection::parse(session_id, &den)?;
    let context = session_context_from_den_session(params, &den)?;
    // Fetch/project before committing state: failed history authorization or
    // transport must not replace a previously known binding with a new session.
    replay_history_for_den_session(
        http,
        config,
        session_id,
        &den,
        "session/load",
        shared_state,
        &restore,
    )
    .await?;
    restore.check_current(shared_state).await?;
    surface_submitted_plan_fallback(session_id, &den).await?;
    commit_restored_session(config, adapter_state, shared_state, &restore, &den, context).await?;
    send_available_commands_update(session_id).await?;
    if let Some(context_budget) = den.get("context_budget").filter(|value| !value.is_null()) {
        send_context_budget_usage_update(session_id, context_budget.clone()).await?;
    }
    let context = adapter_state
        .session_contexts
        .get(session_id)
        .context("restored session missing")?;
    let response = with_session_access_metadata(
        session_lifecycle_result(infer_mode_from_den_session(&den))?,
        context,
    );
    write_response(response_id, Ok(response)).await
}
