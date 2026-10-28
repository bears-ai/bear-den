use super::{
    super::{access, conversations::timestamp, error::Error},
    wire::{self, Action, Channel},
};
use den_service::{
    conversation::persistence::{ConversationRecord, PersistedConversationMessage},
    DenState,
};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(super) struct LiveTurn {
    pub conversation: Uuid,
    pub turn: Value,
}
impl LiveTurn {
    pub fn apply(&mut self, action: &Action) {
        match action {
            Action::Part { part, .. } => self.turn["responseParts"]
                .as_array_mut()
                .expect("live response array")
                .push(json!(part)),
            Action::Delta {
                part_id, content, ..
            } => {
                if let Some(part) = self.turn["responseParts"]
                    .as_array_mut()
                    .and_then(|parts| parts.iter_mut().find(|p| p["id"].as_str() == Some(part_id)))
                {
                    let mut text = part["content"].as_str().unwrap_or_default().to_string();
                    text.push_str(content);
                    part["content"] = json!(text);
                }
            }
            _ => {}
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snapshot {
    pub resource: String,
    pub state: Value,
    pub from_seq: u64,
}

pub(super) async fn all_rows(
    state: &DenState,
    id: Uuid,
) -> Result<Vec<PersistedConversationMessage>, Error> {
    let mut rows = Vec::new();
    let mut bytes = 0_usize;
    let mut before = None;
    for _ in 0..100 {
        let page = den_runtime::channel_lifecycle::interaction_history_page(
            &state.sqlx_pool,
            id,
            before,
            100,
        )
        .await?;
        bytes = bytes.saturating_add(page.iter().map(|row| row.content_text.len()).sum::<usize>());
        if bytes > 1024 * 1024 {
            return Err(Error::conflict(
                "This source exceeds the AHP MVP snapshot byte limit; use paged JSON history.",
            ));
        }
        let end = page.len() < 100;
        before = page.last().map(|r| r.sequence_no);
        rows.extend(page);
        if end {
            rows.reverse();
            return Ok(rows);
        }
    }
    Err(Error::conflict(
        "This source exceeds the AHP MVP snapshot bound; use paged JSON history.",
    ))
}

pub(super) fn turns(id: Uuid, rows: &[PersistedConversationMessage]) -> Vec<Value> {
    let mut turns: Vec<Value> = Vec::new();
    let mut user_sequences: std::collections::HashMap<i64, usize> =
        std::collections::HashMap::new();
    for row in rows {
        if let Some(terminal) = den_runtime::channel_lifecycle::terminal_record(row) {
            if let Some(turn) = user_sequences
                .get(&terminal.user_message_sequence)
                .and_then(|index| turns.get_mut(*index))
            {
                use den_runtime::channel_lifecycle::ChannelTurnOutcome;
                turn["state"] = json!(match terminal.outcome {
                    ChannelTurnOutcome::Completed => "complete",
                    ChannelTurnOutcome::Cancelled => "cancelled",
                    ChannelTurnOutcome::Failed => "error",
                });
                turn["duration"] = json!(terminal.duration_ms);
                if terminal.outcome == ChannelTurnOutcome::Failed {
                    turn["responseParts"].as_array_mut().expect("turn response array").push(json!({"kind":"error","error":{"errorType":"turn_failed","message":terminal.error_message.unwrap_or_else(|| "The turn failed.".into())},"resumable":false}));
                }
            }
            continue;
        }
        let Some(message) = row.to_user_history_record() else {
            continue;
        };
        // Role/kind parsing is confined to this canonical projection boundary.
        if message.kind != "message" {
            continue;
        }
        let text = den_runtime::agent_assist::sanitize_visible_transcript_text(&message.content);
        match message.role.as_str() {
            "user" => {
                let turn = row
                    .provider_message_id
                    .clone()
                    .or_else(|| {
                        row.content_json
                            .get("request_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| format!("{id}:{}", row.sequence_no));
                user_sequences.insert(row.sequence_no, turns.len());
                turns.push(json!({"id":turn,"startedAt":timestamp(row.created_at),"message":{"text":text,"origin":{"kind":"user"}},"responseParts":[],"state":"error"}));
            }
            "assistant" => {
                let target = row
                    .content_json
                    .get("request_id")
                    .or_else(|| row.content_json.get("ahp_turn_id"))
                    .and_then(Value::as_str);
                let index = target
                    .and_then(|target| {
                        turns
                            .iter()
                            .position(|turn| turn["id"].as_str() == Some(target))
                    })
                    .or_else(|| turns.len().checked_sub(1));
                let turn = index.and_then(|index| turns.get_mut(index));
                if let Some(turn) = turn {
                    let part_id = row
                        .provider_message_id
                        .clone()
                        .unwrap_or_else(|| format!("{id}:{}", row.sequence_no));
                    turn["responseParts"]
                        .as_array_mut()
                        .expect("turn response array")
                        .push(json!({"kind":"markdown","id":part_id,"content":text}));
                    turn["state"] = json!(row
                        .content_json
                        .get("ahp_state")
                        .and_then(Value::as_str)
                        .filter(|s| matches!(*s, "complete" | "cancelled" | "error"))
                        .unwrap_or("complete"));
                }
            }
            _ => {}
        }
    }
    turns
}

pub(super) fn chat_summary(row: &ConversationRecord, can_send: bool, running: bool) -> Value {
    json!({"resource":Channel::Chat(row.id).uri(),"title":row.current_title.as_deref().filter(|s|!s.is_empty()).unwrap_or("Conversation"),"status":if running {8}else{1},"modifiedAt":timestamp(row.updated_at),"interactivity":if can_send {"full"}else{"read-only"}})
}

pub(super) async fn snapshot(
    state: &DenState,
    user: i32,
    channel: Channel,
    sequence: u64,
    live: Option<&LiveTurn>,
) -> Result<Snapshot, Error> {
    let resource = channel.uri();
    let value = match channel {
        Channel::Root => {
            json!({"agents":[{"provider":"den","displayName":"Bears","description":"Hat-bound Den conversations","models":[],"capabilities":{"multipleChats":{"fork":false,"sideChat":false}}}],"_meta":wire::feature_metadata()})
        }
        Channel::Session(id) | Channel::Chat(id) => {
            let row = access::readable(state, user, id).await?;
            let can_send = access::can_send(state, user, &row).await?;
            let live = live.filter(|live| live.conversation == id);
            let summary = chat_summary(&row, can_send, live.is_some());
            match channel {
                Channel::Session(_) => {
                    json!({"provider":"den","title":summary["title"],"status":summary["status"],"lifecycle":"ready","activeClients":[],"chats":[summary],"defaultChat":Channel::Chat(id).uri(),"_meta":wire::feature_metadata()})
                }
                Channel::Chat(_) => {
                    let rows = all_rows(state, id).await?;
                    let mut turns = turns(id, &rows);
                    if let Some(live) = live {
                        turns.retain(|turn| turn["id"] != live.turn["id"]);
                    }
                    let mut value = summary;
                    value["turns"] = json!(turns);
                    value["_meta"] = wire::feature_metadata();
                    if let Some(live) = live {
                        value["activeTurn"] = live.turn.clone();
                    }
                    value
                }
                Channel::Root => unreachable!(),
            }
        }
    };
    Ok(Snapshot {
        resource,
        state: value,
        from_seq: sequence,
    })
}
