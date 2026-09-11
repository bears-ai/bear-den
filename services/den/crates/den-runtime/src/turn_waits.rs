use serde_json::{json, Value};
use uuid::Uuid;

use den_core::{
    client_tools::{client_tool_policy_json_for_provider, ClientToolName},
    DenError,
};

use bearwire_protocol::wire::{
    BearWireEvent, ExecutionTargetWire, ResourceRef, ToolCallRequestedWire, ToolCallWaitingWire,
    ToolPermissionWire,
};

use crate::agent_loop::RUNTIME_CHECKPOINT_TOOL_NAME;
use crate::runtime::bearwire_projection::wire::tool_call_wire;
use crate::{bearwire_events, turn_obligations, turn_runs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionOwner {
    Den,
    Armature,
}

impl ToolExecutionOwner {
    const fn to_wire(self) -> ExecutionTargetWire {
        match self {
            Self::Den => ExecutionTargetWire::Den,
            Self::Armature => ExecutionTargetWire::ArmatureLocal,
        }
    }
}

pub fn descriptor_resolution_failed(error: &DenError) -> bool {
    matches!(
        error,
        DenError::ValidationError(message)
            if message.starts_with("descriptor_resolution_failed:")
    )
}

pub fn resolve_tool_execution_owner(tool_name: &str) -> Result<ToolExecutionOwner, DenError> {
    if tool_name == RUNTIME_CHECKPOINT_TOOL_NAME {
        return Ok(ToolExecutionOwner::Den);
    }
    if let Some(descriptor) =
        den_core::tools::descriptor::builtin_den_tool_descriptor_for_provider_name(tool_name)
    {
        if !den_core::tools::dispatch::has_known_executor(descriptor.name) {
            return Err(DenError::ValidationError(format!(
                "descriptor_resolution_failed: Den tool `{tool_name}` has no registered executor"
            )));
        }
        return match descriptor.execution_target {
            "den" => Ok(ToolExecutionOwner::Den),
            other => Err(DenError::ValidationError(format!(
                "descriptor_resolution_failed: Den tool `{tool_name}` has unsupported execution_target `{other}`"
            ))),
        };
    }
    if ClientToolName::from_provider_alias(tool_name).is_some() {
        return Ok(ToolExecutionOwner::Armature);
    }
    Err(DenError::ValidationError(format!(
        "descriptor_resolution_failed: no execution owner descriptor for tool `{tool_name}`"
    )))
}

fn tool_call_policy(tool_name: &str, owner: ToolExecutionOwner) -> Value {
    match owner {
        ToolExecutionOwner::Den => json!({ "execution_target": "den" }),
        ToolExecutionOwner::Armature => client_tool_policy_json_for_provider(tool_name),
    }
}

#[derive(Debug, Clone)]
pub struct PersistToolCallWaitInput<'a> {
    pub process_epoch_id: Uuid,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub bear_id: Uuid,
    pub user_id: i32,
    pub request_id: Uuid,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub title: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub arguments: &'a Value,
    pub approval_request_id: Option<&'a str>,
    pub approval_required: bool,
    pub approval_reason: Option<&'a str>,
    pub event_run_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct PersistedToolCallWait {
    pub effective_approval_required: bool,
    pub turn_step_id: Uuid,
    pub obligation: Option<turn_obligations::TurnObligationRow>,
    pub event_sequence: i64,
}

#[derive(Debug, Clone)]
pub struct PersistSurfaceObligationInput<'a> {
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub kind: turn_obligations::TurnObligationKind,
    pub expected_responder_action: turn_obligations::ExpectedResponderAction,
    pub responder_ref_id: &'a str,
    pub request_payload: Value,
}

#[derive(Debug, Clone)]
pub struct PersistedSurfaceObligation {
    pub turn_step_id: Uuid,
    pub obligation: turn_obligations::TurnObligationRow,
}

async fn lock_nonterminal_run_for_wait(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_id: &str,
    run_id: &str,
) -> Result<(), DenError> {
    let state = sqlx::query_scalar!(
        r#"
        SELECT state
        FROM turn_runs
        WHERE run_id = $1 AND session_id = $2
        FOR UPDATE
        "#,
        run_id,
        session_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let parsed = state
        .as_deref()
        .map(turn_runs::TurnRunState::try_from_storage)
        .transpose()?;
    if parsed.is_some_and(|state| !state.is_terminal()) {
        return Ok(());
    }
    Err(DenError::RunStateConflict {
        operation: "persist_client_wait",
        run_id: run_id.to_string(),
        expected_state: "nonterminal",
        actual_state: state,
    })
}

async fn ensure_waiting_turn_step(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: &str,
    mark_waiting: bool,
) -> Result<Uuid, DenError> {
    let turn_step_id = if let Some(row) = sqlx::query!(
        r#"
        SELECT id
        FROM turn_steps
        WHERE run_id = $1
          AND state IN ('streaming_model', 'waiting_for_client', 'ready_to_continue')
        ORDER BY step_index DESC
        LIMIT 1
        "#,
        run_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    {
        row.id
    } else {
        sqlx::query!(
            r#"
            WITH next_step AS (
                SELECT COALESCE(MAX(step_index), -1) + 1 AS step_index
                FROM turn_steps
                WHERE run_id = $1
            )
            INSERT INTO turn_steps (run_id, step_index, state)
            SELECT $1, step_index, 'streaming_model'
            FROM next_step
            RETURNING id
            "#,
            run_id,
        )
        .fetch_one(&mut **tx)
        .await?
        .id
    };
    if mark_waiting {
        sqlx::query!(
            r#"
            UPDATE turn_steps
            SET state = 'waiting_for_client'
            WHERE id = $1
              AND state IN ('streaming_model', 'waiting_for_client', 'ready_to_continue')
            "#,
            turn_step_id,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(turn_step_id)
}
pub async fn persist_surface_obligation_transactionally(
    pool: &sqlx::PgPool,
    input: PersistSurfaceObligationInput<'_>,
) -> Result<PersistedSurfaceObligation, DenError> {
    let mut tx = pool.begin().await?;
    lock_nonterminal_run_for_wait(&mut tx, input.session_id, input.run_id).await?;
    sqlx::query!(
        r#"
        UPDATE turn_runs
        SET state = $2, terminal_reason = NULL, updated_at = NOW()
        WHERE run_id = $1 AND session_id = $3
        "#,
        input.run_id,
        turn_runs::TurnRunState::WaitingForClient.as_str(),
        input.session_id,
    )
    .execute(&mut *tx)
    .await?;

    let turn_step_id = ensure_waiting_turn_step(&mut tx, input.run_id, true).await?;

    let row = sqlx::query_as!(turn_obligations::TurnObligationRow,
        r#"
        INSERT INTO turn_obligations (
            run_id, session_id, turn_step_id, kind, expected_responder_action,
            responder_ref_id, state, request_payload
        ) VALUES ($1, $2, $3, $4, $5, $6, 'waiting_for_client', $7)
        RETURNING id, run_id, session_id, kind, expected_responder_action, tool_call_id, permission_id, responder_ref_id, state, turn_step_id, request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
        "#,
        input.run_id,
        input.session_id,
        turn_step_id,
        input.kind.as_str(),
        input.expected_responder_action.as_str(),
        input.responder_ref_id,
        input.request_payload,
    )
    .fetch_one(&mut *tx)
    .await?;
    let obligation = row;
    tx.commit().await?;

    Ok(PersistedSurfaceObligation {
        turn_step_id,
        obligation,
    })
}

pub async fn persist_bearwire_tool_call_wait_transactionally(
    pool: &sqlx::PgPool,
    input: PersistToolCallWaitInput<'_>,
) -> Result<PersistedToolCallWait, DenError> {
    let has_permission_id = input
        .approval_request_id
        .map(str::trim)
        .is_some_and(|id| !id.is_empty());
    if input.approval_required && !has_permission_id {
        tracing::warn!(
            session_id = input.session_id,
            run_id = input.run_id,
            tool_call_id = input.tool_call_id,
            tool_name = input.tool_name,
            "runtime emitted approval_required tool call without approval_request_id; treating as tool result obligation"
        );
    }
    let effective_approval_required = input.approval_required && has_permission_id;
    let execution_owner = resolve_tool_execution_owner(input.tool_name)?;
    let den_owned = execution_owner == ToolExecutionOwner::Den;
    let run_state = turn_runs::TurnRunState::WaitingForClient;
    let execution_target = match execution_owner {
        ToolExecutionOwner::Den => "den",
        ToolExecutionOwner::Armature => "armature_local",
    };
    let policy = tool_call_policy(input.tool_name, execution_owner);
    let request_payload = json!({
        "den_process_epoch_id": input.process_epoch_id,
        "tool_call_id": input.tool_call_id,
        "tool_name": input.tool_name,
        "arguments": input.arguments,
        "approval_required": effective_approval_required,
        "approval_request_id": input.approval_request_id,
        "execution_target": execution_target,
        "policy": policy,
        "request_id": input.request_id,
    });

    let mut tx = pool.begin().await?;
    lock_nonterminal_run_for_wait(&mut tx, input.session_id, input.run_id).await?;
    if !den_owned || effective_approval_required {
        sqlx::query!(
            r#"
            UPDATE turn_runs
            SET state = $2, terminal_reason = NULL, updated_at = NOW()
            WHERE run_id = $1 AND session_id = $3
            "#,
            input.run_id,
            run_state.as_str(),
            input.session_id,
        )
        .execute(&mut *tx)
        .await?;
    }

    let turn_step_id = ensure_waiting_turn_step(
        &mut tx,
        input.run_id,
        !den_owned || effective_approval_required,
    )
    .await?;

    let obligation_row = if effective_approval_required {
        let permission_id = input.approval_request_id.unwrap_or_default();
        if let Some(row) = sqlx::query_as!(turn_obligations::TurnObligationRow,
            r#"
            UPDATE turn_obligations
            SET session_id = $2,
                turn_step_id = COALESCE($6, turn_step_id),
                kind = 'permission_decision',
                expected_responder_action = 'permission_decision',
                permission_id = $4,
                state = CASE
                    WHEN state IN ('result_received','continued','failed','cancelled') THEN state
                    ELSE 'waiting_for_client'
                END,
                request_payload = $5,
                updated_at = NOW()
            WHERE run_id = $1
              AND tool_call_id = $3
              AND (permission_id IS NULL OR permission_id = $4)
            RETURNING id, run_id, session_id, kind, expected_responder_action, tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
            "#,
            input.run_id,
            input.session_id,
            input.tool_call_id,
            permission_id,
            request_payload.clone(),
            turn_step_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        {
            Some(row)
        } else {
            Some(sqlx::query_as!(turn_obligations::TurnObligationRow,
                r#"
                INSERT INTO turn_obligations (
                    run_id, session_id, turn_step_id, kind, expected_responder_action,
                    tool_call_id, permission_id, state, request_payload
                ) VALUES ($1, $2, $3, 'permission_decision', 'permission_decision', $4, $5, 'waiting_for_client', $6)
                ON CONFLICT (run_id, permission_id) WHERE permission_id IS NOT NULL
                DO UPDATE SET session_id = EXCLUDED.session_id,
                              turn_step_id = COALESCE(EXCLUDED.turn_step_id, turn_obligations.turn_step_id),
                              tool_call_id = COALESCE(EXCLUDED.tool_call_id, turn_obligations.tool_call_id),
                              state = CASE
                                WHEN turn_obligations.state IN ('result_received','continued','failed','cancelled')
                                THEN turn_obligations.state
                                ELSE EXCLUDED.state
                              END,
                              request_payload = EXCLUDED.request_payload,
                              updated_at = NOW()
                RETURNING id, run_id, session_id, kind, expected_responder_action, tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
                "#,
                input.run_id,
                input.session_id,
                turn_step_id,
                input.tool_call_id,
                permission_id,
                request_payload.clone(),
            )
            .fetch_one(&mut *tx)
            .await?)
        }
    } else if den_owned {
        None
    } else {
        Some(sqlx::query_as!(turn_obligations::TurnObligationRow,
            r#"
            INSERT INTO turn_obligations (
                run_id, session_id, turn_step_id, kind, expected_responder_action,
                tool_call_id, permission_id, state, request_payload
            ) VALUES ($1, $2, $3, 'tool_result', 'tool_result', $4, $5, 'waiting_for_client', $6)
            ON CONFLICT (run_id, tool_call_id) WHERE tool_call_id IS NOT NULL
            DO UPDATE SET session_id = EXCLUDED.session_id,
                          turn_step_id = COALESCE(EXCLUDED.turn_step_id, turn_obligations.turn_step_id),
                          expected_responder_action = EXCLUDED.expected_responder_action,
                          permission_id = COALESCE(EXCLUDED.permission_id, turn_obligations.permission_id),
                          state = CASE
                            WHEN turn_obligations.state IN ('result_received','continued','failed','cancelled')
                            THEN turn_obligations.state
                            ELSE EXCLUDED.state
                          END,
                          request_payload = EXCLUDED.request_payload,
                          updated_at = NOW()
            RETURNING id, run_id, session_id, kind, expected_responder_action, tool_call_id, permission_id, NULL::text AS "responder_ref_id?", state, turn_step_id, request_payload, result_payload, created_at, updated_at, completed_at, lease_attempt_token_hash, claimed_at, lease_expires_at
            "#,
            input.run_id,
            input.session_id,
            turn_step_id,
            input.tool_call_id,
            input.approval_request_id,
            request_payload.clone(),
        )
        .fetch_one(&mut *tx)
        .await?)
    };
    let obligation = obligation_row;

    let effective_kind = input.kind.unwrap_or("function");
    let tool_call = tool_call_wire(
        input.tool_call_id,
        input.tool_name,
        input.title,
        effective_kind,
        input.arguments,
    );
    let mut event = if effective_approval_required {
        let obligation_ref = obligation.as_ref().ok_or_else(|| {
            DenError::ValidationError(format!(
                "approval-required tool call `{}` has no persisted permission obligation",
                input.tool_name
            ))
        })?;
        let permission_id = input.approval_request_id.unwrap_or_default().to_string();
        BearWireEvent::tool_call_waiting(ToolCallWaitingWire {
            expected_responder_action: Some("permission_decision".to_string()),
            expected_client_method: "client.permission.result".to_string(),
            obligation_id: Some(obligation_ref.id.to_string()),
            tool_call,
            permission: ToolPermissionWire {
                id: permission_id,
                reason: input.approval_reason.map(str::to_string),
                title: None,
                target: None,
            },
            approval_required: true,
            execution_target: execution_owner.to_wire(),
            policy: request_payload.get("policy").cloned(),
            turn_step_id: obligation_ref.turn_step_id.map(|id| id.to_string()),
        })
    } else {
        BearWireEvent::tool_call_requested(ToolCallRequestedWire {
            expected_responder_action: obligation.as_ref().map(|_| "tool_result".to_string()),
            obligation_id: obligation
                .as_ref()
                .map(|obligation| obligation.id.to_string()),
            policy: request_payload.get("policy").cloned(),
            tool_call,
            approval_required: false,
            execution_target: execution_owner.to_wire(),
            approval_request_id: input.approval_request_id.map(str::to_string),
            reason: input.approval_reason.map(str::to_string),
        })
    };
    event.bear_id = Some(input.bear_id.to_string());
    event.human_id = Some(input.user_id.to_string());
    event.session_id = Some(input.session_id.to_string());
    event.run_id = input
        .event_run_id
        .map(str::to_string)
        .or_else(|| Some(input.run_id.to_string()));
    event.subject = Some(format!("resource/tool_call/{}", input.tool_call_id));
    event
        .resource_refs
        .push(ResourceRef::new("run", input.run_id.to_string()));
    event.resource_refs.push(ResourceRef::new(
        "tool_call",
        input.tool_call_id.to_string(),
    ));
    if effective_approval_required {
        let obligation_ref = obligation.as_ref().ok_or_else(|| {
            DenError::ValidationError(format!(
                "approval-required tool call `{}` has no persisted permission obligation",
                input.tool_name
            ))
        })?;
        let permission_id = obligation_ref.permission_id.clone().unwrap_or_default();
        event
            .resource_refs
            .push(ResourceRef::new("permission_request", permission_id));
        event.resource_refs.push(ResourceRef::new(
            "client_obligation",
            obligation_ref.id.to_string(),
        ));
    }
    let persisted = bearwire_events::append_bearwire_event_on(
        &mut tx,
        input.session_id,
        Some(input.bear_id),
        Some(input.user_id),
        event,
    )
    .await?;
    tx.commit().await?;

    Ok(PersistedToolCallWait {
        effective_approval_required,
        turn_step_id,
        obligation,
        event_sequence: persisted.sequence_no,
    })
}
