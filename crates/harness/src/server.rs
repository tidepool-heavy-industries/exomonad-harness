//! Headless HTTP boundary for submitting commands and following harness events.
//!
//! The scheduler owns the command receiver and publishes events through the
//! returned control handle. The module intentionally does not depend on a
//! scheduler/store implementation so the crate owner can wire it independently.
mod assets;
pub mod history;
mod ws_protocol;

pub use ws_protocol::{
    CommandControl, CommandReceipt, CommandReceiptOutcome, HostActorIdentity, HostActorKind,
    HostActorLifecycle, HostActorProjection, Snapshot, ToolJobRecord, ToolJobState, WsClientFrame,
    WsEvent, WsEventPayload, WsServerFrame,
};

pub use crate::embedding::{ClientOperationId, EmbeddedRoundId};
use crate::store::Store;
use axum::extract::ws::{Message, WebSocket};
use axum::{
    Json, Router,
    extract::{Request, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc};

const COMMAND_CAPACITY: usize = 128;
const EVENT_CAPACITY: usize = 512;
const COMMAND_RECEIPT_CAPACITY: usize = 128;

/// Stable command envelope accepted by `POST /api/commands`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Submit {
        command: String,
    },
    Host {
        operation_id: ClientOperationId,
        command: HostCommand,
    },
}

/// Exact host address supplied by the authenticated operator. The host still
/// validates live ownership and admission; this envelope grants no authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HostCommand {
    Input {
        target: HostActorIdentity,
        text: String,
    },
    Interrupt {
        target: HostActorIdentity,
        expected_round: EmbeddedRoundId,
    },
    Retire {
        target: HostActorIdentity,
    },
}

impl HostCommand {
    pub fn target(&self) -> &HostActorIdentity {
        match self {
            Self::Input { target, .. }
            | Self::Interrupt { target, .. }
            | Self::Retire { target } => target,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRefusal {
    InvalidCommand,
    Conflict,
    Unavailable,
    WrongRun,
}

#[derive(Clone, Debug, Serialize)]
struct RefusedCommand {
    code: CommandRefusal,
    reason: String,
}

fn refusal(code: CommandRefusal, reason: impl Into<String>) -> Response {
    let status = match code {
        CommandRefusal::InvalidCommand => StatusCode::BAD_REQUEST,
        CommandRefusal::Conflict => StatusCode::CONFLICT,
        CommandRefusal::WrongRun => StatusCode::FORBIDDEN,
        CommandRefusal::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(RefusedCommand {
            code,
            reason: reason.into(),
        }),
    )
        .into_response()
}

fn retain_host_command(
    state: &AppState,
    operation: ClientOperationId,
    command: &HostCommand,
) -> Result<(), (CommandRefusal, String)> {
    let run = state
        .snapshot
        .read()
        .map_err(|_| {
            (
                CommandRefusal::Unavailable,
                "host projection unavailable".into(),
            )
        })?
        .host_run
        .clone();
    if run.as_deref() != Some(command.target().run.as_str()) {
        return Err((
            CommandRefusal::WrongRun,
            "command target is not in this embedded run".into(),
        ));
    }
    let store = state.history_store.as_ref().ok_or((
        CommandRefusal::Unavailable,
        "command Store unavailable".into(),
    ))?;
    store
        .enqueue_embedded_command(operation, command)
        .map_err(|e| {
            (
                if matches!(e, crate::store::StoreError::ConflictingCommand) {
                    CommandRefusal::Conflict
                } else {
                    CommandRefusal::Unavailable
                },
                e.to_string(),
            )
        })?;
    // Notification only: the existing owner also drains queued Store rows at
    // startup and on its periodic wake, closing the commit/enqueue crash gap.
    let _ = state.commands.try_send(QueuedCommand {
        command_id: operation.to_string(),
        command: ClientCommand::Host {
            operation_id: operation,
            command: command.clone(),
        },
    });
    Ok(())
}

/// Stable acknowledgement for an accepted command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandAccepted {
    pub command_id: String,
}

/// Stable event envelope sent as JSON on `GET /api/events` (SSE).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerEvent {
    pub sequence: u64,
    pub event: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// A bearer credential, intentionally redacted from Debug output.
#[derive(Clone)]
pub struct BearerSecret(Arc<str>);

impl std::fmt::Debug for BearerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BearerSecret([REDACTED])")
    }
}

impl BearerSecret {
    /// Construct an API secret. At least 32 UTF-8 bytes are required.
    pub fn new(secret: impl Into<String>) -> Result<Self, &'static str> {
        let secret = secret.into();
        if secret.len() < 32 {
            return Err("API bearer secret must be at least 32 bytes");
        }
        if secret.bytes().any(|b| b.is_ascii_control()) {
            return Err("API bearer secret must not contain control bytes");
        }
        Ok(Self(Arc::from(secret)))
    }
}

/// Separate operator password for the optional local browser-login flow.
#[derive(Clone)]
pub struct SessionSecret(Arc<str>);

impl std::fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionSecret([REDACTED])")
    }
}

impl SessionSecret {
    /// Construct an operator secret. At least 32 UTF-8 bytes are required.
    pub fn new(secret: impl Into<String>) -> Result<Self, &'static str> {
        let secret = secret.into();
        if secret.len() < 32 {
            return Err("browser session secret must be at least 32 bytes");
        }
        if secret.bytes().any(|b| b.is_ascii_control()) {
            return Err("browser session secret must not contain control bytes");
        }
        Ok(Self(Arc::from(secret)))
    }
}

/// Explicit server configuration. Without a bearer secret or optional browser
/// session, protected API routes fail closed.
pub struct ServerConfig {
    pub asset_root: PathBuf,
    history_store: Option<Arc<Store>>,
    bearer_secret: Option<BearerSecret>,
    session_secret: Option<SessionSecret>,
    session_lifetime: Duration,
    public_origin_scheme: String,
}

impl ServerConfig {
    pub fn new(asset_root: PathBuf) -> Self {
        Self {
            asset_root,
            history_store: None,
            bearer_secret: None,
            session_secret: None,
            session_lifetime: Duration::from_secs(8 * 60 * 60),
            public_origin_scheme: "http".into(),
        }
    }

    /// Attach the opened durable Store for protected, bounded history reads.
    /// Without this attachment history endpoints must report unavailable.
    pub fn with_history_store(mut self, store: Arc<Store>) -> Self {
        self.history_store = Some(store);
        self
    }

    pub fn with_bearer_secret(mut self, secret: BearerSecret) -> Self {
        self.bearer_secret = Some(secret);
        self
    }

    /// Enable the optional local browser login using an independent operator
    /// secret. The cookie lifetime must be positive.
    pub fn with_browser_session(
        mut self,
        secret: SessionSecret,
        lifetime: Duration,
    ) -> Result<Self, &'static str> {
        if lifetime.is_zero() {
            return Err("browser session lifetime must be positive");
        }
        self.session_secret = Some(secret);
        self.session_lifetime = lifetime;
        Ok(self)
    }

    /// Set the browser-facing scheme used by the WebSocket same-origin check.
    /// This is explicit configuration, never inferred from proxy headers.
    pub fn with_public_origin_scheme(
        mut self,
        scheme: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let scheme = scheme.into();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err("public origin scheme must be http or https");
        }
        self.public_origin_scheme = scheme;
        Ok(self)
    }
}

#[derive(Clone)]
struct AppState {
    commands: mpsc::Sender<QueuedCommand>,
    events: broadcast::Sender<ServerEvent>,
    asset_root: Arc<PathBuf>,
    snapshot: Arc<std::sync::RwLock<Snapshot>>,
    history_store: Option<Arc<Store>>,
    public_origin_scheme: Arc<str>,
    auth: Arc<ApiAuthPolicy>,
}

#[derive(Clone)]
struct ApiAuth {
    policy: Arc<ApiAuthPolicy>,
    public_origin_scheme: Arc<str>,
}

struct ApiAuthPolicy {
    bearer_secret: Option<BearerSecret>,
    browser_session: Option<BrowserSessions>,
}

struct BrowserSessions {
    secret: SessionSecret,
    lifetime: Duration,
    tokens: std::sync::Mutex<HashMap<String, Instant>>,
}

/// Command delivered to the owning scheduler.
#[derive(Debug)]
pub struct QueuedCommand {
    pub command_id: String,
    pub command: ClientCommand,
}

/// Producer-side API given to the scheduler.
#[derive(Clone)]
pub struct ServerControl {
    events: broadcast::Sender<ServerEvent>,
    next_sequence: Arc<std::sync::Mutex<u64>>,
    snapshot: Arc<std::sync::RwLock<Snapshot>>,
}

impl ServerControl {
    /// Publish an event to current SSE subscribers. Events are deliberately live
    /// only; the durable store remains the source of truth for replay/query APIs.
    pub fn publish(&self, event: impl Into<String>, payload: serde_json::Value) -> ServerEvent {
        // Keep sequence allocation and broadcast ordered together when several
        // scheduler tasks publish concurrently.
        let mut next_sequence = self.next_sequence.lock().expect("sequence lock poisoned");
        let sequence = *next_sequence;
        *next_sequence += 1;
        let event = ServerEvent {
            sequence,
            event: event.into(),
            payload,
        };
        self.snapshot.write().expect("snapshot lock poisoned").seq = sequence;
        let _ = self.events.send(event.clone());
        drop(next_sequence);
        event
    }

    /// Store and publish a bounded command handoff receipt atomically.
    ///
    /// Receipts describe admission or control routing, never command
    /// completion. Durable input records remain in the owning store.
    pub fn publish_command_receipt(&self, receipt: CommandReceipt) -> ServerEvent {
        let payload = serde_json::to_value(&receipt).expect("command receipt serializes to JSON");
        let mut next_sequence = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        if let Some(existing) = snapshot
            .command_receipts
            .iter()
            .position(|existing| existing.command_id == receipt.command_id)
        {
            snapshot.command_receipts.remove(existing);
        }
        snapshot.command_receipts.push(receipt);
        if snapshot.command_receipts.len() > COMMAND_RECEIPT_CAPACITY {
            let excess = snapshot.command_receipts.len() - COMMAND_RECEIPT_CAPACITY;
            snapshot.command_receipts.drain(..excess);
        }

        let sequence = *next_sequence;
        *next_sequence += 1;
        snapshot.seq = sequence;
        let event = ServerEvent {
            sequence,
            event: "command.receipt".into(),
            payload,
        };
        let _ = self.events.send(event.clone());
        event
    }

    /// Publish bounded completed model requests from their durable Store evidence.
    /// Request metadata and its event watermark become visible together. Reconnect
    /// retains the same bounded window of the last 128 model completion events;
    /// older requests leave the projection through explicit removal events.
    pub fn refresh_completed_model_requests(
        &self,
        store: &Store,
    ) -> Result<(), crate::store::StoreError> {
        let mut next_sequence = self.next_sequence.lock().expect("sequence lock poisoned");
        let requests = store.completed_model_requests(128)?;
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        let rows = requests
            .into_iter()
            .map(|request| {
                serde_json::json!({
                    "id": request.id.0,
                    "parentId": request.parent.map(|parent| parent.0),
                    "conversationId": request.branch,
                    "state": "completed",
                })
            })
            .collect::<Vec<_>>();
        let previous = conversation_rows_by_id(&snapshot.requests);
        let current = conversation_rows_by_id(&rows);
        let mut changes = Vec::new();
        for id in previous.keys().filter(|id| !current.contains_key(*id)) {
            changes.push((
                "entity.remove",
                serde_json::json!({"entity":"request", "id":id}),
            ));
        }
        for row in &rows {
            let id = row["id"].as_str().expect("request identity is a string");
            if previous.get(id) != Some(row) {
                changes.push(("request.upsert", row.clone()));
            }
        }
        snapshot.requests = rows;
        for (kind, payload) in changes {
            let sequence = *next_sequence;
            *next_sequence += 1;
            snapshot.seq = sequence;
            let _ = self.events.send(ServerEvent {
                sequence,
                event: kind.into(),
                payload,
            });
        }
        Ok(())
    }

    /// Replace the WebSocket snapshot with a store-backed view.
    ///
    /// The snapshot's `seq` is a watermark: subsequent published events have a
    /// strictly greater sequence. Use the current event sequence when injecting
    /// a store snapshot so reconnecting clients can resynchronize consistently.
    pub fn set_snapshot(&self, mut snapshot: Snapshot) {
        let mut next_sequence = self.next_sequence.lock().expect("sequence lock poisoned");
        let current_sequence = next_sequence.saturating_sub(1);
        snapshot.seq = snapshot.seq.max(current_sequence);
        *next_sequence = (*next_sequence).max(snapshot.seq.saturating_add(1));
        let mut current = self.snapshot.write().expect("snapshot lock poisoned");
        let mut receipts = std::mem::take(&mut current.command_receipts);
        for receipt in snapshot.command_receipts.drain(..) {
            if let Some(existing) = receipts
                .iter()
                .position(|existing| existing.command_id == receipt.command_id)
            {
                receipts.remove(existing);
            }
            receipts.push(receipt);
        }
        if receipts.len() > COMMAND_RECEIPT_CAPACITY {
            let excess = receipts.len() - COMMAND_RECEIPT_CAPACITY;
            receipts.drain(..excess);
        }
        snapshot.command_receipts = receipts;
        *current = snapshot;
    }

    /// Atomically replace the host-owned projection and publish its deltas.
    ///
    /// The event watermark is committed with the corresponding rows before
    /// subscribers can observe those events. A reconnect therefore cannot see
    /// a sequence that claims to include projection changes absent from its
    /// snapshot. Requests, jobs, and envelopes remain owned by their existing
    /// producers and are retained unchanged.
    pub fn update_host_projection(
        &self,
        run: String,
        actors: Vec<HostActorProjection>,
        conversations: Vec<serde_json::Value>,
    ) {
        debug_assert!(actors.iter().all(|actor| actor.identity.run == run));

        let mut next_sequence = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");

        let previous_actors: BTreeMap<_, _> = snapshot
            .actors
            .iter()
            .map(|actor| (actor.identity.wire_key(), actor.clone()))
            .collect();
        let next_actors: BTreeMap<_, _> = actors
            .iter()
            .map(|actor| (actor.identity.wire_key(), actor.clone()))
            .collect();
        let previous_conversations = conversation_rows_by_id(&snapshot.conversations);
        let next_conversations = conversation_rows_by_id(&conversations);
        let mut changes = Vec::<(String, serde_json::Value)>::new();

        if snapshot.host_run.as_deref() != Some(run.as_str()) {
            changes.push(("host_run.upsert".into(), serde_json::json!({"run":run})));
        }

        for (id, _) in previous_actors
            .iter()
            .filter(|(id, _)| !next_actors.contains_key(*id))
        {
            changes.push((
                "entity.remove".into(),
                serde_json::json!({"entity":"actor", "id":id}),
            ));
        }
        for (id, actor) in &next_actors {
            if previous_actors.get(id).is_some_and(|old| old == actor) {
                continue;
            }
            changes.push((
                "actor.upsert".into(),
                serde_json::to_value(actor).expect("host actor projection serializes"),
            ));
        }
        for (id, _) in previous_conversations
            .iter()
            .filter(|(id, _)| !next_conversations.contains_key(*id))
        {
            changes.push((
                "entity.remove".into(),
                serde_json::json!({"entity":"conversation", "id":id}),
            ));
        }
        for (id, conversation) in &next_conversations {
            if previous_conversations
                .get(id)
                .is_some_and(|old| old == conversation)
            {
                continue;
            }
            changes.push(("conversation.upsert".into(), conversation.clone()));
        }

        let mut events = Vec::with_capacity(changes.len());
        let mut watermark = next_sequence.saturating_sub(1);
        for (kind, payload) in changes {
            let sequence = *next_sequence;
            *next_sequence += 1;
            watermark = sequence;
            events.push(ServerEvent {
                sequence,
                event: kind,
                payload,
            });
        }

        snapshot.host_run = Some(run);
        snapshot.actors = actors;
        snapshot.conversations = conversations;
        snapshot.seq = snapshot.seq.max(watermark);
        for event in events {
            let _ = self.events.send(event);
        }
    }
}

fn conversation_rows_by_id(rows: &[serde_json::Value]) -> BTreeMap<String, serde_json::Value> {
    rows.iter()
        .map(|row| {
            let id = row
                .get("id")
                .and_then(serde_json::Value::as_str)
                .expect("host conversation projection has a string id");
            (id.to_owned(), row.clone())
        })
        .collect()
}

/// Build a fail-closed server: public static assets work, but protected `/api/*`
/// routes return 401 until explicit authentication is configured.
///
/// `asset_root` points at a future web build directory. `/` and `/assets/*`
/// are served from it; the helper rejects traversal and falls back to index.
pub fn server(asset_root: PathBuf) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
    server_with_config(ServerConfig::new(asset_root))
}

/// Build the server with explicit access policy and asset root.
pub fn server_with_config(
    config: ServerConfig,
) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
    let ServerConfig {
        asset_root,
        history_store,
        bearer_secret,
        session_secret,
        session_lifetime,
        public_origin_scheme,
    } = config;
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    let state = AppState {
        commands,
        events: events.clone(),
        asset_root: Arc::new(asset_root),
        snapshot: Arc::new(std::sync::RwLock::new(Snapshot::default())),
        history_store,
        public_origin_scheme: Arc::from(public_origin_scheme),
        auth: Arc::new(ApiAuthPolicy {
            bearer_secret: bearer_secret.clone(),
            browser_session: session_secret.map(|secret| BrowserSessions {
                secret,
                lifetime: session_lifetime,
                tokens: std::sync::Mutex::new(HashMap::new()),
            }),
        }),
    };
    let snapshot = state.snapshot.clone();
    let control = ServerControl {
        events,
        next_sequence: Arc::new(std::sync::Mutex::new(1)),
        snapshot,
    };
    let auth = ApiAuth {
        policy: state.auth.clone(),
        public_origin_scheme: state.public_origin_scheme.clone(),
    };
    let protected_api = Router::new()
        .route("/commands", post(submit_command))
        .route("/commands/{operation_id}", get(command_status))
        .route("/events", get(event_stream))
        .route("/history/{request_id}", get(history::request_history))
        .route("/ws", get(websocket))
        .route_layer(middleware::from_fn_with_state(auth, authorize))
        .with_state(state.clone());
    let session_api = Router::new()
        .route(
            "/session",
            get(session_status)
                .post(session_login)
                .delete(session_logout),
        )
        .with_state(state.clone());
    let api = protected_api.merge(session_api);
    let router = Router::new()
        .nest("/api", api)
        .route("/", get(index_asset))
        .route("/{*path}", get(static_asset))
        .with_state(state);
    (router, control, receiver)
}

async fn authorize(State(auth): State<ApiAuth>, request: Request, next: Next) -> Response {
    let session_id = valid_session_from_headers(request.headers(), &auth.policy);
    let bearer = bearer_authorized(request.headers(), &auth.policy);
    if !bearer && session_id.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "API authentication required",
        )
            .into_response();
    }
    let submitting =
        request.method() == axum::http::Method::POST && request.uri().path() == "/commands";
    let command_lookup_with_origin = request.method() == axum::http::Method::GET
        && request.uri().path().starts_with("/commands/")
        && request.headers().contains_key(header::ORIGIN);
    if (submitting || command_lookup_with_origin)
        && session_id.is_some()
        && !same_origin(request.headers(), &auth.public_origin_scheme)
    {
        return (StatusCode::FORBIDDEN, "same-origin request required").into_response();
    }
    next.run(request).await
}

fn bearer_authorized(headers: &HeaderMap, policy: &ApiAuthPolicy) -> bool {
    policy.bearer_secret.as_ref().is_some_and(|secret| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|provided| constant_time_eq(provided.as_bytes(), secret.0.as_bytes()))
    })
}

fn valid_session_from_headers(headers: &HeaderMap, policy: &ApiAuthPolicy) -> Option<String> {
    let sessions = policy.browser_session.as_ref()?;
    let session_id = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == "harness_session" && !value.is_empty()).then(|| value.to_owned())
        })?;
    let mut active = sessions.tokens.lock().ok()?;
    match active.get(&session_id) {
        Some(expiry) if *expiry > Instant::now() => Some(session_id),
        _ => {
            active.remove(&session_id);
            None
        }
    }
}

fn constant_time_eq(provided: &[u8], expected: &[u8]) -> bool {
    let mut difference = provided.len() ^ expected.len();
    for index in 0..provided.len().max(expected.len()) {
        difference |= usize::from(
            provided.get(index).copied().unwrap_or(0) ^ expected.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

async fn websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    if !same_origin(&headers, &state.public_origin_scheme) {
        return Err(StatusCode::FORBIDDEN);
    }
    // Subscribe before upgrading, so events published during the handshake are
    // buffered and delivered after the initial snapshot.
    let receiver = state.events.subscribe();
    Ok(upgrade.on_upgrade(move |socket| websocket_session(socket, state, receiver, headers)))
}

fn same_origin(headers: &HeaderMap, expected_scheme: &str) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return false;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(origin) = origin.parse::<Uri>() else {
        return false;
    };
    let Some(scheme) = origin.scheme_str() else {
        return false;
    };
    // Authorization is always the configured bearer credential. Forwarded
    // headers are intentionally not consulted; scheme comes from explicit
    // public-server configuration, not an untrusted x-forwarded-proto header.
    if scheme != expected_scheme {
        return false;
    }
    let Some(authority) = origin.authority() else {
        return false;
    };
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    !authority.as_str().contains('@') && authority.as_str().eq_ignore_ascii_case(host)
}

async fn websocket_session(
    mut socket: WebSocket,
    state: AppState,
    mut receiver: broadcast::Receiver<ServerEvent>,
    headers: HeaderMap,
) {
    let mut last_sent = send_snapshot(&mut socket, &state).await.unwrap_or(0);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !bearer_authorized(&headers, &state.auth) && valid_session_from_headers(&headers, &state.auth).is_none() { break; }
                        let frame = match serde_json::from_str::<WsClientFrame>(&text) {
                            Ok(frame) => frame,
                            Err(_) => {
                                let reply = WsServerFrame::CommandRefused { operation_id: None, code: CommandRefusal::InvalidCommand, reason: "Malformed command: host operation UUID and interrupt round UUID are required.".into() };
                                if send_ws_frame(&mut socket,&reply).await.is_err() { break; }
                                continue;
                            }
                        };
                        match frame {
                            WsClientFrame::SnapshotRequest => {
                                last_sent = send_snapshot(&mut socket, &state).await.unwrap_or(last_sent);
                            }
                            WsClientFrame::Command { command } => {
                                let command_id = uuid::Uuid::new_v4().to_string();
                                if state.commands.send(QueuedCommand {
                                    command_id: command_id.clone(),
                                    command: ClientCommand::Submit { command },
                                }).await.is_err() {
                                    break;
                                }
                                let reply = WsServerFrame::CommandAccepted { command_id };
                                if send_ws_frame(&mut socket, &reply).await.is_err() {
                                    break;
                                }
                            }
                            WsClientFrame::HostCommand { operation_id, command } => {
                                let reply = match retain_host_command(&state, operation_id, &command) {
                                    Ok(()) => WsServerFrame::CommandAccepted { command_id: operation_id.to_string() },
                                    Err((code,reason)) => WsServerFrame::CommandRefused { operation_id: Some(operation_id), code, reason },
                                };
                                if send_ws_frame(&mut socket, &reply).await.is_err() { break; }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                }
            }
            received = receiver.recv() => {
                match received {
                    Ok(event) if event.sequence > last_sent => {
                        let frame = WsServerFrame::Event {
                            event: WsEvent {
                                seq: event.sequence,
                                event: WsEventPayload { kind: event.event, value: event.payload },
                            },
                        };
                        if send_ws_frame(&mut socket, &frame).await.is_err() {
                            break;
                        }
                        last_sent = event.sequence;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        match send_snapshot(&mut socket, &state).await {
                            Ok(seq) => last_sent = seq,
                            Err(_) => break,
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn send_snapshot(socket: &mut WebSocket, state: &AppState) -> Result<u64, ()> {
    let snapshot = state.snapshot.read().map_err(|_| ())?.clone();
    let seq = snapshot.seq;
    send_ws_frame(socket, &WsServerFrame::Snapshot { snapshot }).await?;
    Ok(seq)
}

async fn send_ws_frame(socket: &mut WebSocket, frame: &WsServerFrame) -> Result<(), ()> {
    let encoded = serde_json::to_string(frame).map_err(|_| ())?;
    socket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(|_| ())
}

async fn submit_command(
    State(state): State<AppState>,
    command: Result<Json<ClientCommand>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let command = match command {
        Ok(Json(command)) => command,
        Err(_) => {
            return refusal(
                CommandRefusal::InvalidCommand,
                "Malformed command: host operation UUID and interrupt round UUID are required.",
            );
        }
    };
    let command_id = match &command {
        ClientCommand::Host {
            operation_id,
            command,
        } => {
            if let Err((code, reason)) = retain_host_command(&state, *operation_id, command) {
                return refusal(code, reason);
            }
            operation_id.to_string()
        }
        ClientCommand::Submit { .. } => {
            let command_id = uuid::Uuid::new_v4().to_string();
            if state
                .commands
                .send(QueuedCommand {
                    command_id: command_id.clone(),
                    command,
                })
                .await
                .is_err()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            command_id
        }
    };
    (StatusCode::ACCEPTED, Json(CommandAccepted { command_id })).into_response()
}

async fn command_status(
    State(state): State<AppState>,
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> Response {
    let operation = match uuid::Uuid::parse_str(&raw) {
        Ok(id) => ClientOperationId(id),
        Err(_) => return refusal(CommandRefusal::InvalidCommand, "invalid operation UUID"),
    };
    let Some(store) = &state.history_store else {
        return refusal(CommandRefusal::Unavailable, "command Store unavailable");
    };
    let run = state.snapshot.read().ok().and_then(|s| s.host_run.clone());
    let Some(run) = run else {
        return refusal(CommandRefusal::Unavailable, "embedded run unavailable");
    };
    match store.embedded_command(&run, operation) {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => refusal(CommandRefusal::Unavailable, "command Store read failed"),
    }
}

#[derive(Deserialize)]
struct SessionLoginRequest {
    secret: String,
}

#[derive(Serialize)]
struct SessionStatus {
    authenticated: bool,
}

async fn session_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let authenticated = bearer_authorized(&headers, &state.auth)
        || valid_session_from_headers(&headers, &state.auth).is_some();
    let mut response = Json(SessionStatus { authenticated }).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn session_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SessionLoginRequest>,
) -> Result<Response, StatusCode> {
    if !same_origin(&headers, &state.public_origin_scheme) {
        return Err(StatusCode::FORBIDDEN);
    }
    let sessions = state
        .auth
        .browser_session
        .as_ref()
        .ok_or(StatusCode::NOT_FOUND)?;
    if !constant_time_eq(input.secret.as_bytes(), sessions.secret.0.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Two v4 UUIDs provide a 244-bit unpredictable opaque session id. It is
    // held only in memory and the HttpOnly cookie, never in a URL or response
    // body.
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let now = Instant::now();
    let expires_at = now + sessions.lifetime;
    let mut active = sessions
        .tokens
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    active.retain(|_, expiry| *expiry > now);
    active.insert(token.clone(), expires_at);
    drop(active);

    let max_age = sessions.lifetime.as_secs().max(1);
    let secure = if state.public_origin_scheme.as_ref() == "https" {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "harness_session={token}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    );
    let mut response = Json(SessionStatus {
        authenticated: true,
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

async fn session_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if !same_origin(&headers, &state.public_origin_scheme) {
        return Err(StatusCode::FORBIDDEN);
    }
    let sessions = state
        .auth
        .browser_session
        .as_ref()
        .ok_or(StatusCode::NOT_FOUND)?;
    if let Some(session_id) = session_id_from_cookie(&headers) {
        sessions
            .tokens
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .remove(&session_id);
    }
    let secure = if state.public_origin_scheme.as_ref() == "https" {
        "; Secure"
    } else {
        ""
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("harness_session=; Path=/api; HttpOnly; SameSite=Strict; Max-Age=0{secure}")
            .parse()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

fn session_id_from_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == "harness_session" && !value.is_empty()).then(|| value.to_owned())
        })
}

async fn event_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).expect("ServerEvent serializes");
                    return Some((
                        Ok(SseEvent::default().event(event.event).data(data)),
                        receiver,
                    ));
                }
                // A lagged client resumes at the oldest still-buffered event.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn index_asset(State(state): State<AppState>) -> Response<axum::body::Body> {
    assets::asset_response(state.asset_root.as_path(), "index.html").await
}

async fn static_asset(
    State(state): State<AppState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response<axum::body::Body> {
    assets::asset_response(state.asset_root.as_path(), &path).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn retained_model_requests_install_index_when_reopening_current_database() {
        let path =
            std::env::temp_dir().join(format!("harness-request-index-{}.db", uuid::Uuid::new_v4()));
        {
            let store = Store::open(&path).unwrap();
            let request = crate::model::RequestId("retained".into());
            store.create_request(&request, None, "/root").unwrap();
            store
                .record_event(Some(&request), "model_turn", &serde_json::json!({}))
                .unwrap();
            store
                .lock()
                .execute_batch("DROP INDEX events_model_turn_recent")
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let index: bool = store.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='events_model_turn_recent')",
            [], |row| row.get(0),
        ).unwrap();
        assert!(
            index,
            "current-version databases also receive the additive index"
        );
        let (_, control, _) = server(PathBuf::from("."));
        control.refresh_completed_model_requests(&store).unwrap();
        assert_eq!(
            control.snapshot.read().unwrap().requests[0]["id"],
            "retained"
        );
        let conn = store.lock();
        let mut query = conn.prepare(
            "EXPLAIN QUERY PLAN SELECT id,request_id FROM events WHERE kind='model_turn' ORDER BY id DESC LIMIT 128",
        ).unwrap();
        let plans = query
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plans
                .iter()
                .any(|plan| plan.contains("events_model_turn_recent")),
            "{plans:?}"
        );
        drop(query);
        drop(conn);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn retained_model_requests_follow_durable_completions_and_survive_resync() {
        use crate::{
            model::{Effort, RequestId},
            transport::{ResponsesRequest, ResponsesTurn},
        };
        let store = Store::memory().unwrap();
        let (_, control, _) = server(PathBuf::from("."));
        let mut events = control.events.subscribe();
        let pending = RequestId("pending".into());
        store.create_request(&pending, None, "/root").unwrap();
        control.refresh_completed_model_requests(&store).unwrap();
        assert!(control.snapshot.read().unwrap().requests.is_empty());
        let model_request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![],
            tools_allowed: None,
            model: "offline".into(),
            pinned_effort: Effort::Low,
            session_id: "session".into(),
        };
        for (id, branch) in [("z-first", "/root"), ("a-last", "/root/child")] {
            let request = RequestId(id.into());
            store
                .create_request(&request, Some(&pending), branch)
                .unwrap();
            store
                .record_replay_turn(
                    &request,
                    &model_request,
                    &ResponsesTurn {
                        response_id: id.into(),
                        items: vec![],
                        usage: Default::default(),
                    },
                )
                .unwrap();
        }
        control.refresh_completed_model_requests(&store).unwrap();
        let first = events.try_recv().unwrap();
        let last = events.try_recv().unwrap();
        assert_eq!(first.event, "request.upsert");
        assert_eq!(first.payload["id"], "z-first");
        assert_eq!(last.payload["id"], "a-last");
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(snapshot.seq, last.sequence);
        assert_eq!(snapshot.requests.len(), 2);
        assert_eq!(snapshot.requests[1]["conversationId"], "/root/child");
        assert_eq!(snapshot.requests[1]["state"], "completed");
        control.refresh_completed_model_requests(&store).unwrap();
        assert!(
            events.try_recv().is_err(),
            "unchanged evidence emits no duplicate updates"
        );
        let (_, fresh_control, _) = server(PathBuf::from("."));
        fresh_control
            .refresh_completed_model_requests(&store)
            .unwrap();
        assert_eq!(
            fresh_control.snapshot.read().unwrap().requests,
            snapshot.requests
        );
        for index in 0..130 {
            let request = RequestId(format!("bounded-{index:03}"));
            store.create_request(&request, None, "/root").unwrap();
            store
                .record_replay_turn(
                    &request,
                    &model_request,
                    &ResponsesTurn {
                        response_id: request.0.clone(),
                        items: vec![],
                        usage: Default::default(),
                    },
                )
                .unwrap();
        }
        control.refresh_completed_model_requests(&store).unwrap();
        let snapshot = control.snapshot.read().unwrap();
        assert_eq!(snapshot.requests.len(), 128);
        assert_eq!(snapshot.requests[0]["id"], "bounded-002");
        assert_eq!(snapshot.requests[127]["id"], "bounded-129");
        assert!(
            snapshot
                .requests
                .iter()
                .all(|row| row.get("input").is_none())
        );
    }
    use super::*;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

    const TEST_SECRET: &str = "test-only-secret-for-server-auth-checks";
    const TEST_SESSION_SECRET: &str = "distinct-operator-secret-for-tests-only";

    fn authorized_server(
        asset_root: PathBuf,
    ) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
        let secret = BearerSecret::new(TEST_SECRET).unwrap();
        server_with_config(
            ServerConfig::new(asset_root)
                .with_bearer_secret(secret)
                .with_public_origin_scheme("http")
                .unwrap(),
        )
    }

    fn browser_session_server(
        asset_root: PathBuf,
        lifetime: Duration,
        public_scheme: &str,
    ) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
        let secret = SessionSecret::new(TEST_SESSION_SECRET).unwrap();
        let config = ServerConfig::new(asset_root)
            .with_browser_session(secret, lifetime)
            .unwrap()
            .with_public_origin_scheme(public_scheme)
            .unwrap();
        server_with_config(config)
    }

    fn projected_actor(run: &str, actor: &str, incarnation: &str) -> HostActorProjection {
        HostActorProjection {
            identity: HostActorIdentity {
                run: run.into(),
                actor: crate::model::AgentPath(actor.into()),
                incarnation: incarnation.into(),
            },
            parent: None,
            kind: HostActorKind::Workflow,
            lifecycle: HostActorLifecycle::Waiting,
            model_conversation: None,
            active_round: None,
        }
    }

    fn websocket(
        address: std::net::SocketAddr,
        origin: Option<&str>,
        authorization: Option<&str>,
    ) -> (TcpStream, String) {
        websocket_with_cookie(address, origin, authorization, None)
    }

    fn websocket_with_cookie(
        address: std::net::SocketAddr,
        origin: Option<&str>,
        authorization: Option<&str>,
        cookie: Option<&str>,
    ) -> (TcpStream, String) {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let authorization = authorization
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let origin = origin
            .map(|origin| format!("Origin: {origin}\r\n"))
            .unwrap_or_default();
        let cookie = cookie
            .map(|cookie| format!("Cookie: {cookie}\r\n"))
            .unwrap_or_default();
        write!(socket, "GET /api/ws HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{origin}{authorization}{cookie}\r\n").unwrap();
        let mut response = Vec::new();
        loop {
            let mut byte = [0; 1];
            socket.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
            if response.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        (socket, String::from_utf8(response).unwrap())
    }

    fn read_ws_text(socket: &mut TcpStream) -> serde_json::Value {
        let mut header = [0; 2];
        socket.read_exact(&mut header).unwrap();
        assert_eq!(header[0] & 0x0f, 1, "expected text frame");
        let mut length = usize::from(header[1] & 0x7f);
        if length == 126 {
            let mut ext = [0; 2];
            socket.read_exact(&mut ext).unwrap();
            length = usize::from(u16::from_be_bytes(ext));
        } else if length == 127 {
            let mut ext = [0; 8];
            socket.read_exact(&mut ext).unwrap();
            length = u64::from_be_bytes(ext) as usize;
        }
        assert_eq!(header[1] & 0x80, 0, "server frame must not be masked");
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn write_ws_text(socket: &mut TcpStream, text: &str) {
        let bytes = text.as_bytes();
        let mask = [0x13, 0x57, 0x9b, 0xdf];
        let mut frame = vec![0x81];
        match bytes.len() {
            0..=125 => frame.push(0x80 | bytes.len() as u8),
            126..=65535 => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            }
            _ => panic!("test frame unexpectedly large"),
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            bytes
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % mask.len()]),
        );
        socket.write_all(&frame).unwrap();
    }

    #[test]
    fn protocol_types_have_stable_json_shapes() {
        let command = ClientCommand::Submit {
            command: "start".into(),
        };
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({"type":"submit","command":"start"})
        );
        let event = ServerEvent {
            sequence: 7,
            event: "job_started".into(),
            payload: json!({"handle":"job-1"}),
        };
        assert_eq!(
            serde_json::from_str::<ServerEvent>(&serde_json::to_string(&event).unwrap()).unwrap(),
            event
        );
    }

    #[test]
    fn host_projection_diffs_rows_and_preserves_other_snapshot_owners() {
        let (_, control, _) = authorized_server(PathBuf::from("."));
        let old_actor = projected_actor("run-old", "/root/worker", "inc-1");
        control.set_snapshot(Snapshot {
            seq: 7,
            host_run: Some("run-old".into()),
            command_receipts: vec![],
            actors: vec![old_actor.clone()],
            conversations: vec![json!({"id":"conversation-old","path":"/root/worker"})],
            requests: vec![json!({"id":"request-1"})],
            jobs: vec![json!({"id":"job-1"})],
            envelopes: vec![json!({"id":"envelope-1"})],
        });
        let mut events = control.events.subscribe();

        let new_actor = projected_actor("run-new", "/root/worker", "inc-2");
        let new_conversation = json!({"id":"conversation-new", "path":"/root/worker"});
        control.update_host_projection(
            "run-new".into(),
            vec![new_actor.clone()],
            vec![new_conversation.clone()],
        );

        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(snapshot.seq, 12);
        assert_eq!(snapshot.host_run.as_deref(), Some("run-new"));
        assert_eq!(snapshot.actors, vec![new_actor.clone()]);
        assert_eq!(snapshot.conversations, vec![new_conversation]);
        assert_eq!(snapshot.requests, vec![json!({"id":"request-1"})]);
        assert_eq!(snapshot.jobs, vec![json!({"id":"job-1"})]);
        assert_eq!(snapshot.envelopes, vec![json!({"id":"envelope-1"})]);

        let observed: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert_eq!(
            observed
                .iter()
                .map(|event| (event.sequence, event.event.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (8, "host_run.upsert"),
                (9, "entity.remove"),
                (10, "actor.upsert"),
                (11, "entity.remove"),
                (12, "conversation.upsert"),
            ]
        );
        assert_eq!(observed[0].payload, json!({"run":"run-new"}));
        assert_eq!(
            observed[1].payload,
            json!({"entity":"actor", "id":old_actor.identity.wire_key()})
        );
        assert_eq!(observed[2].payload["identity"]["incarnation"], "inc-2");
        assert_eq!(
            observed[3].payload,
            json!({"entity":"conversation", "id":"conversation-old"})
        );
        assert_eq!(observed[4].payload["id"], "conversation-new");
        assert_eq!(snapshot.seq, observed.last().unwrap().sequence);
    }

    #[test]
    fn command_receipts_update_snapshot_and_publish_a_coherent_watermark() {
        let (_, control, _) = authorized_server(PathBuf::from("."));
        let mut events = control.events.subscribe();
        let target = HostActorIdentity {
            run: "run-1".into(),
            actor: crate::model::AgentPath("/root/worker".into()),
            incarnation: "inc-2".into(),
        };
        let receipt = CommandReceipt {
            command_id: "cmd-1".into(),
            outcome: CommandReceiptOutcome::Admitted {
                target: Some(target.clone()),
                envelope_id: "envelope-4".into(),
                wake_error: None,
            },
        };
        let event = control.publish_command_receipt(receipt.clone());
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(event.event, "command.receipt");
        assert_eq!(event.sequence, snapshot.seq);
        assert_eq!(snapshot.command_receipts, vec![receipt]);
        assert_eq!(event.payload["outcome"], "admitted");
        assert_eq!(event.payload["envelopeId"], "envelope-4");

        control.set_snapshot(Snapshot {
            jobs: vec![json!({"id":"job-after-command"})],
            ..Snapshot::default()
        });
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(snapshot.command_receipts.len(), 1);
        assert_eq!(snapshot.jobs, vec![json!({"id":"job-after-command"})]);
        let wire = serde_json::to_value(WsServerFrame::Snapshot { snapshot }).unwrap();
        assert_eq!(wire["snapshot"]["commandReceipts"][0]["commandId"], "cmd-1");

        let requested = CommandReceipt {
            command_id: "cmd-control".into(),
            outcome: CommandReceiptOutcome::ControlRequested {
                target: target.clone(),
                control: CommandControl::Interrupt,
            },
        };
        let event = control.publish_command_receipt(requested.clone());
        let snapshot = control.snapshot.read().unwrap();
        assert_eq!(snapshot.seq, event.sequence);
        assert_eq!(snapshot.command_receipts.last(), Some(&requested));
        assert_eq!(event.payload["outcome"], "control_requested");
        assert_eq!(event.payload["control"], "interrupt");
        assert_ne!(event.payload["outcome"], "completed");
        drop(snapshot);

        for index in 0..=COMMAND_RECEIPT_CAPACITY {
            control.publish_command_receipt(CommandReceipt {
                command_id: format!("cmd-{index}"),
                outcome: CommandReceiptOutcome::Refused {
                    target: None,
                    reason: "not admitted".into(),
                },
            });
        }
        let snapshot = control.snapshot.read().unwrap();
        assert_eq!(snapshot.command_receipts.len(), COMMAND_RECEIPT_CAPACITY);
        assert_eq!(
            snapshot.command_receipts.first().unwrap().command_id,
            "cmd-1"
        );
        assert_eq!(
            snapshot.command_receipts.last().unwrap().command_id,
            "cmd-128"
        );
        assert_eq!(
            std::iter::from_fn(|| events.try_recv().ok())
                .last()
                .unwrap()
                .sequence,
            snapshot.seq
        );
    }

    #[test]
    fn reconnect_snapshot_reads_never_observe_a_partial_host_projection() {
        let (_, control, _) = authorized_server(PathBuf::from("."));
        let mut events = control.events.subscribe();
        let writer_control = control.clone();
        let start = Arc::new(std::sync::Barrier::new(2));
        let writer_start = start.clone();
        let writer = std::thread::spawn(move || {
            writer_start.wait();
            for cycle in 0..64 {
                let run = if cycle % 2 == 0 { "run-a" } else { "run-b" };
                writer_control.update_host_projection(
                    run.into(),
                    vec![projected_actor(run, "/root/worker", &format!("inc-{cycle}"))],
                    vec![json!({"id":format!("conversation-{run}"), "projectionRun":run, "cycle":cycle})],
                );
                std::thread::yield_now();
            }
        });
        start.wait();
        while !writer.is_finished() {
            let snapshot = control.snapshot.read().unwrap().clone();
            if let Some(run) = snapshot.host_run.as_deref() {
                assert_eq!(snapshot.actors.len(), 1);
                assert_eq!(snapshot.actors[0].identity.run, run);
                assert_eq!(snapshot.conversations.len(), 1);
                assert_eq!(snapshot.conversations[0]["projectionRun"], run);
            }
            std::thread::yield_now();
        }
        writer.join().unwrap();

        let snapshot = control.snapshot.read().unwrap().clone();
        let last_event = std::iter::from_fn(|| events.try_recv().ok())
            .last()
            .expect("host projection publishes events");
        assert_eq!(snapshot.seq, last_event.sequence);
        assert_eq!(snapshot.actors[0].identity.run, snapshot.host_run.unwrap());
        assert_eq!(
            snapshot.conversations[0]["projectionRun"],
            snapshot.actors[0].identity.run
        );
    }

    #[test]
    fn bearer_secret_is_strong_enough_and_redacted() {
        assert!(BearerSecret::new("short").is_err());
        let secret = BearerSecret::new(TEST_SECRET).unwrap();
        assert_eq!(format!("{secret:?}"), "BearerSecret([REDACTED])");
        assert!(SessionSecret::new("short").is_err());
        let operator = SessionSecret::new(TEST_SESSION_SECRET).unwrap();
        assert_eq!(format!("{operator:?}"), "SessionSecret([REDACTED])");
        assert!(
            ServerConfig::new(PathBuf::from("."))
                .with_browser_session(operator, Duration::ZERO)
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_fails_closed_without_explicit_bearer_configuration() {
        let (app, _, _) = server(PathBuf::from("."));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();

        let post = client
            .post(format!("http://{address}/api/commands"))
            .json(&ClientCommand::Submit {
                command: "start".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            client
                .get(format!("http://{address}/api/events"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let (_socket, handshake) = websocket(address, Some(&format!("http://{address}")), None);
        assert!(handshake.starts_with("HTTP/1.1 401"), "{handshake}");

        let (_socket, response) = websocket(address, None, Some("incorrect-bearer-secret"));
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_session_login_cookie_command_ws_and_logout() {
        let (app, _, mut commands) =
            browser_session_server(PathBuf::from("."), Duration::from_secs(60), "http");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let origin = format!("http://{address}");
        let client = reqwest::Client::new();

        let bad_origin = client
            .post(format!("http://{address}/api/session"))
            .header(header::ORIGIN, "http://attacker.invalid")
            .json(&serde_json::json!({"secret":TEST_SESSION_SECRET}))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
        let bad_secret = client
            .post(format!("http://{address}/api/session"))
            .header(header::ORIGIN, &origin)
            .json(&serde_json::json!({"secret":"wrong"}))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_secret.status(), StatusCode::UNAUTHORIZED);

        let login = client
            .post(format!("http://{address}/api/session"))
            .header(header::ORIGIN, &origin)
            .json(&serde_json::json!({"secret":TEST_SESSION_SECRET}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let set_cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            login.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({"authenticated":true})
        );
        assert!(set_cookie.contains("Path=/api"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Strict"));
        assert!(!set_cookie.contains("Secure"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();
        assert!(cookie.starts_with("harness_session="));
        let status = client
            .get(format!("http://{address}/api/session"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(status, serde_json::json!({"authenticated":true}));

        let csrf = client
            .post(format!("http://{address}/api/commands"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://attacker.invalid")
            .json(&ClientCommand::Submit {
                command: "csrf".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
        let logout_csrf = client
            .delete(format!("http://{address}/api/session"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "http://attacker.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(logout_csrf.status(), StatusCode::FORBIDDEN);

        let accepted = client
            .post(format!("http://{address}/api/commands"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &origin)
            .json(&ClientCommand::Submit {
                command: "browser-command".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert_eq!(
            commands.recv().await.unwrap().command,
            ClientCommand::Submit {
                command: "browser-command".into()
            }
        );

        let (mut ws, handshake) =
            websocket_with_cookie(address, Some(&origin), None, Some(&cookie));
        assert!(handshake.starts_with("HTTP/1.1 101"), "{handshake}");
        assert_eq!(read_ws_text(&mut ws)["type"], "snapshot");
        drop(ws);

        let logged_out = client
            .delete(format!("http://{address}/api/session"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &origin)
            .send()
            .await
            .unwrap();
        assert_eq!(logged_out.status(), StatusCode::NO_CONTENT);
        assert!(
            logged_out
                .headers()
                .get(header::SET_COOKIE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        let rejected = client
            .post(format!("http://{address}/api/commands"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &origin)
            .json(&ClientCommand::Submit {
                command: "after-logout".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_session_expiry_and_https_cookie_flags() {
        let (app, _, _) =
            browser_session_server(PathBuf::from("."), Duration::from_millis(50), "https");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let origin = format!("https://{address}");
        let client = reqwest::Client::new();

        let login = client
            .post(format!("http://{address}/api/session"))
            .header(header::ORIGIN, &origin)
            .json(&serde_json::json!({"secret":TEST_SESSION_SECRET}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let set_cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(set_cookie.contains("; Secure"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();

        tokio::time::sleep(Duration::from_millis(80)).await;
        let expired = client
            .post(format!("http://{address}/api/commands"))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, &origin)
            .json(&ClientCommand::Submit {
                command: "expired".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
        let status = client
            .get(format!("http://{address}/api/session"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(status, serde_json::json!({"authenticated":false}));
        server_task.abort();
    }

    #[tokio::test]
    async fn command_route_queues_submission_and_event_route_streams_events() {
        let (app, control, mut commands) = authorized_server(PathBuf::from("."));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();

        let response = client
            .post(format!("http://{address}/api/commands"))
            .bearer_auth(TEST_SECRET)
            .json(&ClientCommand::Submit {
                command: "start".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let accepted: CommandAccepted = response.json().await.unwrap();
        let queued = commands.recv().await.unwrap();
        assert_eq!(queued.command_id, accepted.command_id);
        assert_eq!(
            queued.command,
            ClientCommand::Submit {
                command: "start".into()
            }
        );

        let mut events = client
            .get(format!("http://{address}/api/events"))
            .bearer_auth(TEST_SECRET)
            .send()
            .await
            .unwrap()
            .bytes_stream();
        control.publish("job_started", json!({"handle":"job-1"}));
        use futures_util::StreamExt;
        let frame = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = String::from_utf8_lossy(&frame);
        assert!(text.contains("event:"));
        assert!(text.contains("\"event\":\"job_started\""));
        server_task.abort();
    }

    #[tokio::test]
    async fn static_routes_serve_index_and_assets() {
        let root = std::env::temp_dir().join(format!("harness-web-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("assets"))
            .await
            .unwrap();
        tokio::fs::write(root.join("index.html"), "<main>app</main>")
            .await
            .unwrap();
        tokio::fs::write(root.join("assets/app.css"), "body{}")
            .await
            .unwrap();
        let (app, _, _) = authorized_server(root.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let index = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(index.text().await.unwrap(), "<main>app</main>");
        let asset = client
            .get(format!("http://{address}/assets/app.css"))
            .send()
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(
            asset.headers()[reqwest::header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(asset.text().await.unwrap(), "body{}");
        let missing = client
            .get(format!("http://{address}/assets/missing.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        server_task.abort();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn websocket_handshake_snapshot_resync_and_command() {
        let (app, control, mut commands) = server_with_config(
            ServerConfig::new(PathBuf::from("."))
                .with_history_store(Arc::new(Store::memory().unwrap()))
                .with_bearer_secret(BearerSecret::new(TEST_SECRET).unwrap()),
        );
        control.set_snapshot(Snapshot {
            seq: 4,
            conversations: vec![json!({"id":"root"})],
            ..Snapshot::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (mut socket, handshake) = websocket(
            address,
            Some(&format!("http://{address}")),
            Some(TEST_SECRET),
        );
        assert!(handshake.starts_with("HTTP/1.1 101"), "{handshake}");
        assert_eq!(
            read_ws_text(&mut socket),
            json!({"type":"snapshot","snapshot":{
                "seq":4,"conversations":[{"id":"root"}],"requests":[],"jobs":[],"envelopes":[]
            }})
        );
        let (originless_socket, originless_handshake) = websocket(address, None, Some(TEST_SECRET));
        assert!(
            originless_handshake.starts_with("HTTP/1.1 403"),
            "{originless_handshake}"
        );
        drop(originless_socket);

        control.set_snapshot(Snapshot {
            seq: 8,
            jobs: vec![json!({"id":"job-1"})],
            ..Snapshot::default()
        });
        write_ws_text(&mut socket, r#"{"type":"snapshot.request"}"#);
        assert_eq!(
            read_ws_text(&mut socket),
            json!({"type":"snapshot","snapshot":{
                "seq":8,"conversations":[],"requests":[],"jobs":[{"id":"job-1"}],"envelopes":[]
            }})
        );

        write_ws_text(&mut socket, r#"{"type":"command","command":"resume"}"#);
        let accepted = read_ws_text(&mut socket);
        assert_eq!(accepted["type"], "command.accepted");
        let queued = commands.recv().await.unwrap();
        assert_eq!(
            queued.command,
            ClientCommand::Submit {
                command: "resume".into()
            }
        );
        assert_eq!(accepted["command_id"], queued.command_id);

        let mut snapshot = control.snapshot.read().unwrap().clone();
        snapshot.host_run = Some("run-7".into());
        control.set_snapshot(snapshot);
        let target = HostActorIdentity {
            run: "run-7".into(),
            actor: crate::model::AgentPath("/root/child".into()),
            incarnation: "second".into(),
        };
        let input = HostCommand::Input {
            target: target.clone(),
            text: "  resume child  ".into(),
        };
        write_ws_text(
            &mut socket,
            &serde_json::to_string(&json!({"type":"host_command","command":input})).unwrap(),
        );
        let refused = read_ws_text(&mut socket);
        assert_eq!(refused["type"], "command.refused");
        assert_eq!(refused["code"], "invalid_command");
        assert!(commands.try_recv().is_err());
        write_ws_text(
            &mut socket,
            &serde_json::to_string(&WsClientFrame::HostCommand {
                operation_id: ClientOperationId(uuid::Uuid::new_v4()),
                command: input.clone(),
            })
            .unwrap(),
        );
        let accepted = read_ws_text(&mut socket);
        let queued = commands.recv().await.unwrap();
        assert_eq!(accepted["command_id"], queued.command_id);
        assert!(matches!(queued.command, ClientCommand::Host { command, .. } if command == input));

        control.publish("job.started", json!({"id":"job-1"}));
        assert_eq!(
            read_ws_text(&mut socket),
            json!({"type":"event","event":{
                "seq":9,"event":{"kind":"job.started","value":{"id":"job-1"}}
            }})
        );
        drop(socket);
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn websocket_rejects_cross_origin_handshake() {
        let (app, _, _) = authorized_server(PathBuf::from("."));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_socket, response) =
            websocket(address, Some("http://attacker.invalid"), Some(TEST_SECRET));
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
        server_task.abort();
    }

    #[test]
    fn origin_guard_uses_host_and_ignores_untrusted_forwarded_proto() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "example.test".parse().unwrap());
        headers.insert(
            axum::http::header::ORIGIN,
            "https://example.test".parse().unwrap(),
        );
        assert!(same_origin(&headers, "https"));
        headers.insert("x-forwarded-proto", "attacker-controlled".parse().unwrap());
        assert!(same_origin(&headers, "https"));
        assert!(!same_origin(&headers, "http"));
        headers.insert(
            axum::http::header::ORIGIN,
            "https://other.test".parse().unwrap(),
        );
        assert!(!same_origin(&headers, "https"));
    }
}

#[cfg(test)]
mod durable_command_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn host_http_lost_ack_lookup_conflict_and_receipt_eviction() {
        let store = Arc::new(Store::memory().unwrap());
        let secret = "retained-command-tests-only-secret-value";
        let (router, control, _commands) = server_with_config(
            ServerConfig::new(PathBuf::from("."))
                .with_history_store(store.clone())
                .with_browser_session(
                    SessionSecret::new("retained-command-session-only-secret-value").unwrap(),
                    Duration::from_secs(60),
                )
                .unwrap()
                .with_bearer_secret(BearerSecret::new(secret).unwrap()),
        );
        control.set_snapshot(Snapshot {
            host_run: Some("run".into()),
            ..Snapshot::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/api/commands");
        let target = HostActorIdentity {
            run: "run".into(),
            actor: crate::model::AgentPath("/root".into()),
            incarnation: "first".into(),
        };
        let operation = ClientOperationId(uuid::Uuid::new_v4());
        let command = ClientCommand::Host {
            operation_id: operation,
            command: HostCommand::Input {
                target: target.clone(),
                text: "héllo".into(),
            },
        };
        let origin = format!("http://{address}");
        let login = client
            .post(format!("{origin}/api/session"))
            .header("Origin", &origin)
            .json(&json!({"secret":"retained-command-session-only-secret-value"}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let foreign = client
            .post(&endpoint)
            .header("Cookie", &cookie)
            .header("Origin", "http://foreign.example")
            .json(&command)
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert!(store.embedded_command("run", operation).unwrap().is_none());
        let unauthorized = client.post(&endpoint).json(&command).send().await.unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert!(store.embedded_command("run", operation).unwrap().is_none());
        // The first acceptance is deliberately discarded, modeling lost ACK.
        assert_eq!(
            client
                .post(&endpoint)
                .bearer_auth(secret)
                .json(&command)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
        let duplicate: CommandAccepted = client
            .post(&endpoint)
            .bearer_auth(secret)
            .json(&command)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(duplicate.command_id, operation.to_string());
        assert_eq!(store.queued_embedded_commands("run").unwrap().len(), 1);
        let mut different = serde_json::to_value(&command).unwrap();
        different["command"]["text"] = json!("changed");
        let conflict = client
            .post(&endpoint)
            .bearer_auth(secret)
            .json(&different)
            .send()
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            conflict.json::<serde_json::Value>().await.unwrap()["code"],
            "conflict"
        );
        let lookup = format!("{endpoint}/{operation}");
        assert_eq!(
            client.get(&lookup).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&lookup)
                .header("Cookie", &cookie)
                .header("Origin", "http://foreign.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        // The operation ID is independent of the session credential. A valid
        // cookie can observe and resubmit the bearer-issued exact operation.
        assert_eq!(
            client
                .post(&endpoint)
                .header("Cookie", &cookie)
                .header("Origin", &origin)
                .json(&command)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
        let record: crate::store::EmbeddedCommandRecord = client
            .get(&lookup)
            .bearer_auth(secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            record.command,
            match command.clone() {
                ClientCommand::Host { command, .. } => command,
                _ => unreachable!(),
            }
        );
        for raw in [
            json!({"type":"host","command":{"action":"input","target":target,"text":"missing ID"}}),
            json!({"type":"host","operation_id":operation,"command":{"action":"interrupt","target":target}}),
        ] {
            let response = client
                .post(&endpoint)
                .bearer_auth(secret)
                .json(&raw)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap()["code"],
                "invalid_command"
            );
        }
        store
            .claim_embedded_command("run", operation)
            .unwrap()
            .unwrap();
        let receipt = store
            .settle_embedded_command(
                "run",
                operation,
                CommandReceiptOutcome::Refused {
                    target: Some(target.clone()),
                    reason: "retired".into(),
                },
            )
            .unwrap();
        control.publish_command_receipt(receipt.clone());
        for _ in 0..129 {
            control.publish_command_receipt(CommandReceipt {
                command_id: uuid::Uuid::new_v4().to_string(),
                outcome: CommandReceiptOutcome::Refused {
                    target: Some(target.clone()),
                    reason: "another operation".into(),
                },
            });
        }
        assert_eq!(control.snapshot.read().unwrap().command_receipts.len(), 128);
        assert!(
            !control
                .snapshot
                .read()
                .unwrap()
                .command_receipts
                .iter()
                .any(|r| r.command_id == operation.to_string())
        );
        let record: crate::store::EmbeddedCommandRecord = client
            .get(&lookup)
            .bearer_auth(secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(record.receipt, Some(receipt));
        server.abort();
    }
}
