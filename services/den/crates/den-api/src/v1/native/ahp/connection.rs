use super::super::{access, error::Error};
use super::{
    projection::{self, LiveTurn},
    turn,
    wire::{
        self, Action, Channel, Dispatch, Initialize, InputAction, Origin, Reconnect, Request,
        Subscribe,
    },
};
use axum::{
    extract::ws::{Message, WebSocket},
    http::HeaderMap,
};
use den_oauth::oauth::OAuthScope;
use den_service::DenState;
use futures::SinkExt;

use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn next_sequence() -> u64 {
    SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1
}
fn sequence() -> u64 {
    SEQUENCE.load(Ordering::SeqCst)
}

struct Running {
    input: turn::Input,
    submitted: Value,
    cancel: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
    cancel_origin: Option<Origin>,
}
struct Connection {
    state: DenState,
    headers: HeaderMap,
    user: i32,
    client: Option<String>,
    subscriptions: HashSet<Channel>,
    last_client_seq: u64,
    live: Option<LiveTurn>,
    running: Option<Running>,
    events: mpsc::Sender<turn::Event>,
}

pub(super) async fn serve(mut socket: WebSocket, state: DenState, headers: HeaderMap, user: i32) {
    let (tx, mut rx) = mpsc::channel(16);
    let mut connection = Connection {
        state,
        headers,
        user,
        client: None,
        subscriptions: HashSet::new(),
        last_client_seq: 0,
        live: None,
        running: None,
        events: tx,
    };
    let mut auth_tick = tokio::time::interval(Duration::from_secs(15));
    loop {
        let messages = tokio::select! {
            frame=socket.recv()=>match frame {
                Some(Ok(Message::Text(text)))=>connection.receive(&text).await,
                Some(Ok(Message::Ping(data)))=>{if socket.send(Message::Pong(data)).await.is_err(){break;}continue;},
                Some(Ok(Message::Pong(_)))=>continue,
                Some(Ok(Message::Binary(_)))=>vec![wire::rpc_error(None,-32600,"Only JSON text frames are supported.")],
                _=>break,
            },
            event=rx.recv()=>match event {Some(event)=>connection.project(event).await,None=>break},
            _=auth_tick.tick()=>{
                if access::authenticate(&connection.state,&connection.headers,OAuthScope::DataRead).await.is_err(){break;}
                continue;
            }
        };
        let mut failed = false;
        for value in messages {
            let body = value.to_string();
            if body.len() > 2 * 1024 * 1024
                || !matches!(
                    tokio::time::timeout(
                        Duration::from_secs(10),
                        socket.send(Message::Text(body.into()))
                    )
                    .await,
                    Ok(Ok(()))
                )
            {
                failed = true;
                break;
            }
        }
        if failed {
            break;
        }
    }
    // Finish partial canonical persistence after socket loss; never restart work.
    drop(rx);
    if let Some(mut running) = connection.running.take() {
        let _ = running.cancel.send(true);
        if tokio::time::timeout(Duration::from_secs(10), &mut running.handle)
            .await
            .is_err()
        {
            // Leave the cancelled task alive to finish awaited canonical writes.
            // Its source lease remains held until that finalizer has settled.
            tracing::warn!("channel cancellation is still finalizing after socket loss");
        }
    }
    let _ = socket.close().await;
}

impl Connection {
    async fn receive(&mut self, text: &str) -> Vec<Value> {
        if text.len() > wire::MAX_FRAME {
            return vec![wire::rpc_error(
                None,
                -32600,
                "Frame exceeds the input limit.",
            )];
        }
        let request: Request = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(_) => return vec![wire::rpc_error(None, -32700, "Invalid JSON-RPC request.")],
        };
        if request.jsonrpc != "2.0"
            || request
                .id
                .as_ref()
                .is_some_and(|id| !id.is_string() && !id.is_number())
        {
            return vec![wire::rpc_error(
                request.id,
                -32600,
                "Invalid JSON-RPC envelope.",
            )];
        }
        if let Err(error) =
            access::authenticate(&self.state, &self.headers, OAuthScope::DataRead).await
        {
            return vec![wire::rpc_error(request.id, -32001, &error.message)];
        }
        if request.method == "dispatchAction" {
            return self.dispatch(request.params).await;
        }
        let id = request.id.clone();
        let result = self.command(&request.method, request.params).await;
        match (id, result) {
            (Some(id), Ok(result)) => vec![wire::rpc_ok(id, result)],
            (Some(id), Err((code, message))) => {
                let mut error = wire::rpc_error(Some(id), code, &message);
                if code == -32005 {
                    error["error"]["data"] = json!({"supportedVersions":["1.0.0"]});
                }
                vec![error]
            }
            _ => Vec::new(),
        }
    }

    async fn command(&mut self, method: &str, params: Value) -> Result<Value, (i32, String)> {
        let result: Result<Value, Error> = match method {
            "initialize" => {
                let params: Initialize = parse(params)?;
                if self.client.is_some() {
                    return Err((-32600, "Connection is already initialized.".into()));
                }
                if !params
                    .protocol_versions
                    .iter()
                    .any(|version| version == "1.0.0")
                {
                    return Err((-32005, "Only AHP 1.0.0 is supported.".into()));
                }
                self.initialize(&params.channel,&params.client_id,&params.initial_subscriptions).await.map(|snapshots|json!({"protocolVersion":"1.0.0","serverSeq":sequence(),"serverInfo":{"name":"den","version":env!("CARGO_PKG_VERSION")},"snapshots":snapshots,"_meta":wire::feature_metadata()}))
            }
            "reconnect" => {
                let params: Reconnect = parse(params)?;
                if self.client.is_some() {
                    return Err((-32600, "Reconnect requires a fresh connection.".into()));
                }

                self.initialize(&params.channel, &params.client_id, &params.subscriptions)
                    .await
                    .map(|snapshots| json!({"type":"snapshot","snapshots":snapshots}))
            }
            "subscribe" => {
                self.require_initialized()?;
                let params: Subscribe = parse(params)?;
                match Channel::parse(&params.channel) {
                    Ok(channel) => {
                        if self.subscriptions.len() >= 32 && !self.subscriptions.contains(&channel)
                        {
                            Err(Error::invalid("At most 32 subscriptions are supported."))
                        } else {
                            match projection::snapshot(
                                &self.state,
                                self.user,
                                channel,
                                sequence(),
                                self.live.as_ref(),
                            )
                            .await
                            {
                                Ok(snapshot) => {
                                    self.subscriptions.insert(channel);
                                    Ok(json!({"snapshot":snapshot}))
                                }
                                Err(error) => Err(error),
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            "unsubscribe" => {
                self.require_initialized()?;
                let params: Subscribe = parse(params)?;
                match Channel::parse(&params.channel) {
                    Ok(channel) => {
                        self.subscriptions.remove(&channel);
                        Ok(json!({}))
                    }
                    Err(error) => Err(error),
                }
            }

            _ => {
                return Err((
                    -32601,
                    "Method is unavailable in this text-only AHP subset.".into(),
                ))
            }
        };
        result.map_err(|error| (-32000, error.message))
    }

    fn require_initialized(&self) -> Result<(), (i32, String)> {
        if self.client.is_some() {
            Ok(())
        } else {
            Err((-32600, "Initialize the connection first.".into()))
        }
    }
    async fn initialize(
        &mut self,
        channel: &str,
        client: &str,
        subscriptions: &[String],
    ) -> Result<Vec<projection::Snapshot>, Error> {
        if channel != wire::ROOT
            || client.is_empty()
            || client.len() > 128
            || client.chars().any(char::is_control)
            || subscriptions.len() > 32
        {
            return Err(Error::invalid(
                "Invalid root channel, clientId or subscriptions.",
            ));
        }
        let mut snapshots = Vec::new();
        let mut channels = HashSet::new();
        for uri in subscriptions {
            let channel = Channel::parse(uri)?;
            if channels.insert(channel) {
                snapshots.push(
                    projection::snapshot(&self.state, self.user, channel, sequence(), None).await?,
                );
            }
        }
        self.client = Some(client.into());
        self.subscriptions = channels;
        Ok(snapshots)
    }

    async fn dispatch(&mut self, params: Value) -> Vec<Value> {
        let dispatch: Dispatch = match serde_json::from_value(params) {
            Ok(dispatch) => dispatch,
            Err(_) => {
                return vec![wire::rpc_error(
                    None,
                    -32602,
                    "Invalid dispatchAction envelope.",
                )]
            }
        };
        let Some(client) = self.client.clone() else {
            return vec![wire::rpc_error(
                None,
                -32600,
                "Initialize the connection first.",
            )];
        };
        let channel = match Channel::parse(&dispatch.channel) {
            Ok(channel) => channel,
            Err(_) => return vec![wire::rpc_error(None, -32602, "Unsupported channel URI.")],
        };
        let origin = Origin {
            client_id: client,
            client_seq: dispatch.client_seq,
        };
        let rejection = |reason: &str| {
            vec![wire::notification(
                channel,
                dispatch.action.clone(),
                next_sequence(),
                Some(&origin),
                Some(reason),
            )]
        };
        if dispatch.client_seq == 0
            || dispatch.client_seq > 9_007_199_254_740_991
            || dispatch.client_seq <= self.last_client_seq
        {
            return rejection(
                "Duplicate or stale clientSeq; refresh snapshots, never replay sends.",
            );
        }
        self.last_client_seq = dispatch.client_seq;
        let action:InputAction=match serde_json::from_value(dispatch.action.clone()){Ok(action)=>action,Err(_)=>return rejection("Unsupported action or fields. Only text user turnStarted and turnCancelled are accepted.")};
        if let Err(error) = action.validate() {
            return rejection(&error.message);
        }
        let Channel::Chat(conversation) = channel else {
            return rejection("Only chat channels accept these actions.");
        };
        if !self.subscriptions.contains(&channel) {
            return rejection("Subscribe to the chat before dispatching.");
        }
        if access::authenticate(&self.state, &self.headers, OAuthScope::DataWrite)
            .await
            .is_err()
        {
            return rejection("data:write scope and an active login are required.");
        }
        match action {
            InputAction::Start {
                turn_id, message, ..
            } => {
                if self.running.is_some() {
                    return rejection("This connection already has a running turn.");
                }
                let input = turn::Input {
                    conversation,
                    turn: turn_id,
                    message,
                    origin,
                };
                let (cancel, rx) = watch::channel(false);
                let handle = tokio::spawn(turn::run(
                    self.state.clone(),
                    self.headers.clone(),
                    input.clone(),
                    rx,
                    self.events.clone(),
                ));
                self.running = Some(Running {
                    input,
                    submitted: dispatch.action,
                    cancel,
                    handle,
                    cancel_origin: None,
                });
                Vec::new()
            }
            InputAction::Cancel { turn_id, .. } => {
                let Some(running) = self.running.as_mut() else {
                    return rejection("No active turn exists on this connection.");
                };
                if running.input.turn != turn_id || running.input.conversation != conversation {
                    return rejection(
                        "The cancellation does not name this connection's active turn.",
                    );
                }
                running.cancel_origin = Some(origin);
                let _ = running.cancel.send(true);
                Vec::new()
            }
        }
    }

    async fn project(&mut self, event: turn::Event) -> Vec<Value> {
        match event {
            turn::Event::Finished => {
                self.running = None;
                self.live = None;
                Vec::new()
            }
            turn::Event::Rejected(reason) => self
                .running
                .as_ref()
                .map(|running| {
                    vec![wire::notification(
                        Channel::Chat(running.input.conversation),
                        running.submitted.clone(),
                        next_sequence(),
                        Some(&running.input.origin),
                        Some(&reason),
                    )]
                })
                .unwrap_or_default(),
            turn::Event::Action(action, mut origin) => {
                let Some(running) = self.running.as_ref() else {
                    return Vec::new();
                };
                let id = running.input.conversation;
                let terminal = matches!(
                    action,
                    Action::Complete { .. } | Action::Cancelled { .. } | Action::Error { .. }
                );
                if matches!(action, Action::Cancelled { .. }) {
                    origin.clone_from(&running.cancel_origin);
                }
                match &action {
                    Action::Started {
                        turn_id,
                        started_at,
                        message,
                    } => {
                        self.live = Some(LiveTurn {
                            conversation: id,
                            turn: json!({"id":turn_id,"startedAt":started_at,"message":message,"responseParts":[]}),
                        });
                    }
                    _ => {
                        if let Some(live) = self.live.as_mut() {
                            live.apply(&action);
                        }
                    }
                }
                let mut messages = Vec::new();
                if self.subscriptions.contains(&Channel::Chat(id)) {
                    messages.push(wire::notification(
                        Channel::Chat(id),
                        json!(action),
                        next_sequence(),
                        origin.as_ref(),
                        None,
                    ));
                }
                if terminal {
                    self.live = None;
                }
                if matches!(action, Action::Started { .. }) || terminal {
                    if let Ok(row) = access::readable(&self.state, self.user, id).await {
                        let can_send = access::can_send(&self.state, self.user, &row)
                            .await
                            .unwrap_or(false);
                        let summary = projection::chat_summary(&row, can_send, !terminal);
                        if self.subscriptions.contains(&Channel::Session(id)) {
                            messages.push(wire::notification(Channel::Session(id),json!({"type":"session/chatUpdated","chat":Channel::Chat(id).uri(),"changes":{"title":summary["title"],"status":summary["status"],"modifiedAt":summary["modifiedAt"],"interactivity":summary["interactivity"]}}),next_sequence(),None,None));
                        }
                        if self.subscriptions.contains(&Channel::Root) {
                            messages.push(json!({"jsonrpc":"2.0","method":"root/sessionSummaryChanged","params":{"channel":wire::ROOT,"session":Channel::Session(id).uri(),"changes":{"title":summary["title"],"status":summary["status"],"modifiedAt":summary["modifiedAt"],"chats":[{"resource":Channel::Chat(id).uri(),"title":summary["title"],"status":summary["status"]}]}}}));
                        }
                    }
                }
                messages
            }
        }
    }
}
#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, (i32, String)> {
    serde_json::from_value(value).map_err(|_| (-32602, "Invalid method parameters.".into()))
}
