//! Headless HTTP boundary for submitting commands and following harness events.
//!
//! The scheduler owns the command receiver and publishes events through the
//! returned control handle. The module intentionally does not depend on a
//! scheduler/store implementation so the crate owner can wire it independently.
mod assets;
pub mod history;
mod output;
mod ws_protocol;
pub use output::{HistoryRevision, LiveOutput};

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
    extract::{ConnectInfo, Extension, Request, State, WebSocketUpgrade},
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
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc};

const COMMAND_CAPACITY: usize = 128;
const EVENT_CAPACITY: usize = 512;
const COMMAND_RECEIPT_CAPACITY: usize = 128;
const PEER_REVALIDATION_INTERVAL: Duration = Duration::from_secs(30);

/// Verifies the actual socket peer. The embedding owns identity policy and
/// must bound every authentication attempt, including unavailable services.
#[async_trait::async_trait]
pub trait BrowserPeerAuthenticator: Send + Sync {
    async fn authenticate(&self, peer: SocketAddr) -> Result<(), PeerAuthError>;
}

/// A peer is either denied by policy or cannot currently be verified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PeerAuthError {
    #[error("browser peer denied")]
    Denied,
    #[error("browser peer authentication unavailable")]
    Unavailable,
}

impl PeerAuthError {
    fn status(self) -> StatusCode {
        match self {
            Self::Denied => StatusCode::UNAUTHORIZED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

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
    browser_auth: BrowserAuthentication,
    public_origin_scheme: String,
    public_origin_authority: Option<String>,
}

impl ServerConfig {
    pub fn new(asset_root: PathBuf) -> Self {
        Self {
            asset_root,
            history_store: None,
            bearer_secret: None,
            browser_auth: BrowserAuthentication::Disabled,
            public_origin_scheme: "http".into(),
            public_origin_authority: None,
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
        if matches!(self.browser_auth, BrowserAuthentication::Peer(_)) {
            return Err("browser peer and secret authentication are exclusive");
        }
        self.browser_auth = BrowserAuthentication::Secret(BrowserSessions {
            secret,
            lifetime,
            tokens: std::sync::Mutex::new(HashMap::new()),
        });
        Ok(self)
    }

    /// Authenticate browsers through the transport peer rather than cookies.
    /// The listener must supply Axum `ConnectInfo<SocketAddr>`; missing peer
    /// information fails closed. Bearer credentials do not bypass peer policy.
    pub fn with_browser_peer_auth(
        mut self,
        authenticator: Arc<dyn BrowserPeerAuthenticator>,
    ) -> Result<Self, &'static str> {
        if matches!(self.browser_auth, BrowserAuthentication::Secret(_)) {
            return Err("browser peer and secret authentication are exclusive");
        }
        self.browser_auth = BrowserAuthentication::Peer(authenticator);
        Ok(self)
    }

    /// Pin the complete browser-facing origin. Peer authentication requires
    /// this authority on every API request to prevent DNS rebinding.
    pub fn with_public_origin(mut self, origin: impl Into<String>) -> Result<Self, &'static str> {
        let origin = origin.into();
        let uri = origin.parse::<Uri>().map_err(|_| "invalid public origin")?;
        let scheme = uri.scheme_str().ok_or("public origin requires a scheme")?;
        let authority = uri
            .authority()
            .ok_or("public origin requires an authority")?;
        if !matches!(scheme, "http" | "https")
            || authority.as_str().contains('@')
            || uri.path() != "/" && !uri.path().is_empty()
            || uri.query().is_some()
            || origin.contains('#')
        {
            return Err(
                "public origin must be an http or https origin without credentials, path or query",
            );
        }
        self.public_origin_scheme = scheme.into();
        self.public_origin_authority = Some(authority.as_str().into());
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
    public_origin_authority: Option<Arc<str>>,
    auth: Arc<ApiAuthPolicy>,
}

#[derive(Clone)]
struct ApiAuth {
    policy: Arc<ApiAuthPolicy>,
    public_origin_scheme: Arc<str>,
    public_origin_authority: Option<Arc<str>>,
}

struct ApiAuthPolicy {
    bearer_secret: Option<BearerSecret>,
    browser_auth: BrowserAuthentication,
}

enum BrowserAuthentication {
    Disabled,
    Secret(BrowserSessions),
    Peer(Arc<dyn BrowserPeerAuthenticator>),
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

    /// Publish bounded settled model requests from their durable Store evidence.
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
            .map(|settled| {
                let request = settled.request;
                serde_json::json!({
                    "id": request.id.0,
                    "parentId": request.parent.map(|parent| parent.0),
                    "conversationId": request.branch,
                    "state": if settled.failure.is_some() { "failed" } else { "completed" },
                    "failure": settled.failure,
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
        snapshot.live_output = std::mem::take(&mut current.live_output);
        snapshot.history_revisions = std::mem::take(&mut current.history_revisions);
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
        browser_auth,
        public_origin_scheme,
        public_origin_authority,
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
        public_origin_authority: public_origin_authority.map(Arc::from),
        auth: Arc::new(ApiAuthPolicy {
            bearer_secret: bearer_secret.clone(),
            browser_auth,
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
        public_origin_authority: state.public_origin_authority.clone(),
    };
    let protected_api = Router::new()
        .route("/commands", post(submit_command))
        .route("/commands/{operation_id}", get(command_status))
        .route("/events", get(event_stream))
        .route("/history/{request_id}", get(history::request_history))
        .route("/ws", get(websocket))
        .route_layer(middleware::from_fn_with_state(auth, authorize))
        .with_state(state.clone());
    let session_route = if matches!(state.auth.browser_auth, BrowserAuthentication::Peer(_)) {
        get(session_status)
    } else {
        get(session_status)
            .post(session_login)
            .delete(session_logout)
    };
    let session_api = Router::new()
        .route("/session", session_route)
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
    if matches!(auth.policy.browser_auth, BrowserAuthentication::Peer(_))
        && !pinned_authority_matches(request.headers(), auth.public_origin_authority.as_deref())
    {
        return (
            StatusCode::FORBIDDEN,
            "configured browser authority required",
        )
            .into_response();
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    if let Err(error) = authenticate_request(request.headers(), peer, &auth.policy).await {
        let mut response = (error.status(), "API authentication required").into_response();
        if !matches!(auth.policy.browser_auth, BrowserAuthentication::Peer(_)) {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }
        return response;
    }
    let ambient = matches!(auth.policy.browser_auth, BrowserAuthentication::Peer(_))
        || valid_session_from_headers(request.headers(), &auth.policy).is_some();
    let submitting =
        request.method() == axum::http::Method::POST && request.uri().path() == "/commands";
    let command_lookup_with_origin = request.method() == axum::http::Method::GET
        && request.uri().path().starts_with("/commands/")
        && request.headers().contains_key(header::ORIGIN);
    if (submitting || command_lookup_with_origin)
        && ambient
        && !same_origin(
            request.headers(),
            &auth.public_origin_scheme,
            auth.public_origin_authority.as_deref(),
        )
    {
        return (StatusCode::FORBIDDEN, "same-origin request required").into_response();
    }
    next.run(request).await
}

async fn authenticate_request(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    policy: &ApiAuthPolicy,
) -> Result<(), PeerAuthError> {
    match &policy.browser_auth {
        BrowserAuthentication::Peer(authenticator) => {
            authenticator
                .authenticate(peer.ok_or(PeerAuthError::Denied)?)
                .await
        }
        _ if bearer_authorized(headers, policy)
            || valid_session_from_headers(headers, policy).is_some() =>
        {
            Ok(())
        }
        _ => Err(PeerAuthError::Denied),
    }
}

fn bearer_authorized(headers: &HeaderMap, policy: &ApiAuthPolicy) -> bool {
    if matches!(policy.browser_auth, BrowserAuthentication::Peer(_)) {
        return false;
    }
    policy.bearer_secret.as_ref().is_some_and(|secret| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|provided| constant_time_eq(provided.as_bytes(), secret.0.as_bytes()))
    })
}

fn valid_session_from_headers(headers: &HeaderMap, policy: &ApiAuthPolicy) -> Option<String> {
    let BrowserAuthentication::Secret(sessions) = &policy.browser_auth else {
        return None;
    };
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
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    if !same_origin(
        &headers,
        &state.public_origin_scheme,
        state.public_origin_authority.as_deref(),
    ) {
        return Err(StatusCode::FORBIDDEN);
    }
    // Subscribe before upgrading, so events published during the handshake are
    // buffered and delivered after the initial snapshot.
    let receiver = state.events.subscribe();
    let peer = peer.map(|Extension(ConnectInfo(peer))| peer);
    Ok(upgrade.on_upgrade(move |socket| websocket_session(socket, state, receiver, headers, peer)))
}

fn pinned_authority_matches(headers: &HeaderMap, authority: Option<&str>) -> bool {
    match (
        authority,
        headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok()),
    ) {
        (Some(authority), Some(host)) => authority.eq_ignore_ascii_case(host),
        _ => false,
    }
}

fn same_origin(
    headers: &HeaderMap,
    expected_scheme: &str,
    expected_authority: Option<&str>,
) -> bool {
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
    // Scheme comes from explicit configuration, never forwarding headers.
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
    !authority.as_str().contains('@')
        && expected_authority.is_none_or(|expected| expected.eq_ignore_ascii_case(host))
        && authority
            .as_str()
            .eq_ignore_ascii_case(expected_authority.unwrap_or(host))
}

async fn websocket_session(
    mut socket: WebSocket,
    state: AppState,
    mut receiver: broadcast::Receiver<ServerEvent>,
    headers: HeaderMap,
    peer: Option<SocketAddr>,
) {
    if authenticate_request(&headers, peer, &state.auth)
        .await
        .is_err()
    {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    let mut revalidation = tokio::time::interval(PEER_REVALIDATION_INTERVAL);
    revalidation.tick().await;
    let mut last_sent = send_snapshot(&mut socket, &state).await.unwrap_or(0);
    loop {
        tokio::select! {
            _ = revalidation.tick(), if matches!(state.auth.browser_auth, BrowserAuthentication::Peer(_)) => {
                if authenticate_request(&headers, peer, &state.auth).await.is_err() { break; }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if authenticate_request(&headers, peer, &state.auth).await.is_err() { break; }
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
                                let command_id = match enqueue_submit(&state, &headers, peer, command).await {
                                    Ok(command_id) => command_id,
                                    Err(_) => break,
                                };
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
                        if authenticate_request(&headers, peer, &state.auth).await.is_err() { break; }
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
    let _ = socket.send(Message::Close(None)).await;
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

async fn enqueue_submit(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    command: String,
) -> Result<String, PeerAuthError> {
    let permit = state
        .commands
        .reserve()
        .await
        .map_err(|_| PeerAuthError::Unavailable)?;
    // Waiting for queue capacity must not retain admission after revocation.
    authenticate_request(headers, peer, &state.auth).await?;
    let command_id = uuid::Uuid::new_v4().to_string();
    permit.send(QueuedCommand {
        command_id: command_id.clone(),
        command: ClientCommand::Submit { command },
    });
    Ok(command_id)
}

async fn submit_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
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
    let peer = peer.map(|Extension(ConnectInfo(peer))| peer);
    // Axum may wait for a request body after middleware authorization.
    if let Err(error) = authenticate_request(&headers, peer, &state.auth).await {
        return error.status().into_response();
    }
    let command_id = match command {
        ClientCommand::Host {
            operation_id,
            command,
        } => {
            if let Err((code, reason)) = retain_host_command(&state, operation_id, &command) {
                return refusal(code, reason);
            }
            operation_id.to_string()
        }
        ClientCommand::Submit { command } => {
            match enqueue_submit(&state, &headers, peer, command).await {
                Ok(command_id) => command_id,
                Err(error) => return error.status().into_response(),
            }
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
#[serde(rename_all = "lowercase")]
enum AuthenticationMode {
    Secret,
    Tailscale,
    Disabled,
}

#[derive(Serialize)]
struct SessionStatus {
    authenticated: bool,
    authentication: AuthenticationMode,
    available: bool,
}

async fn session_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Response {
    let result = if matches!(state.auth.browser_auth, BrowserAuthentication::Peer(_))
        && !pinned_authority_matches(&headers, state.public_origin_authority.as_deref())
    {
        Err(PeerAuthError::Denied)
    } else {
        authenticate_request(
            &headers,
            peer.map(|Extension(ConnectInfo(peer))| peer),
            &state.auth,
        )
        .await
    };
    let authentication = match &state.auth.browser_auth {
        BrowserAuthentication::Disabled => AuthenticationMode::Disabled,
        BrowserAuthentication::Secret(_) => AuthenticationMode::Secret,
        BrowserAuthentication::Peer(_) => AuthenticationMode::Tailscale,
    };
    let available = !matches!(state.auth.browser_auth, BrowserAuthentication::Disabled)
        && result != Err(PeerAuthError::Unavailable);
    let mut response = Json(SessionStatus {
        authenticated: result.is_ok(),
        authentication,
        available,
    })
    .into_response();
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
    if !same_origin(
        &headers,
        &state.public_origin_scheme,
        state.public_origin_authority.as_deref(),
    ) {
        return Err(StatusCode::FORBIDDEN);
    }
    let BrowserAuthentication::Secret(sessions) = &state.auth.browser_auth else {
        return Err(StatusCode::NOT_FOUND);
    };
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
        authentication: AuthenticationMode::Secret,
        available: true,
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
    if !same_origin(
        &headers,
        &state.public_origin_scheme,
        state.public_origin_authority.as_deref(),
    ) {
        return Err(StatusCode::FORBIDDEN);
    }
    let BrowserAuthentication::Secret(sessions) = &state.auth.browser_auth else {
        return Err(StatusCode::NOT_FOUND);
    };
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
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let receiver = state.events.subscribe();
    let peer = peer.map(|Extension(ConnectInfo(peer))| peer);
    let mut revalidation = tokio::time::interval(PEER_REVALIDATION_INTERVAL);
    revalidation.tick().await;
    let stream = stream::unfold(
        (receiver, revalidation, state.auth, headers, peer),
        |(mut receiver, mut revalidation, auth, headers, peer)| async move {
            loop {
                tokio::select! {
                    _ = revalidation.tick(), if matches!(auth.browser_auth, BrowserAuthentication::Peer(_)) => {
                        if authenticate_request(&headers, peer, &auth).await.is_err() { return None; }
                    }
                    received = receiver.recv() => {
                        match received {
                            Ok(event) => {
                                let data = serde_json::to_string(&event).expect("ServerEvent serializes");
                                return Some((Ok(SseEvent::default().event(event.event).data(data)), (receiver, revalidation, auth, headers, peer)));
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn index_asset(State(state): State<AppState>) -> Response<axum::body::Body> {
    assets::asset_response(state.asset_root.as_path(), "index.html").await
}

async fn static_asset(
    State(state): State<AppState>,
    uri: Uri,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response<axum::body::Body> {
    if is_frontend_page_path(uri.path()) {
        return index_asset(State(state)).await;
    }
    assets::asset_response(state.asset_root.as_path(), &path).await
}

fn is_frontend_page_path(path: &str) -> bool {
    matches!(
        path,
        "/tree" | "/timeline" | "/inbox" | "/host" | "/command" | "/chat"
    ) || path.strip_prefix("/chat/").is_some_and(|actor_path| {
        !actor_path.is_empty()
            && !actor_path.starts_with('/')
            && !actor_path.ends_with('/')
            && !actor_path.contains("//")
            && !actor_path.to_ascii_lowercase().contains("%2e")
            && !actor_path.to_ascii_lowercase().contains("%2f")
            && !actor_path.to_ascii_lowercase().contains("%5c")
            && !actor_path.to_ascii_lowercase().contains("%00")
            && actor_path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_known_frontend_paths_receive_index_fallback() {
        for path in [
            "/tree",
            "/timeline",
            "/inbox",
            "/host",
            "/command",
            "/chat",
            "/chat/root/worker",
        ] {
            assert!(super::is_frontend_page_path(path), "{path}");
        }
        for path in [
            "/api/unknown",
            "/assets/missing.js",
            "/unknown",
            "/chat/",
            "/chat/root//worker",
            "/chat/%2e%2e/secret",
            "/chat/root%2fworker",
        ] {
            assert!(!super::is_frontend_page_path(path), "{path}");
        }
    }

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
    fn retired_actor_head_survives_global_activity_eviction_and_fresh_snapshot() {
        let store = Store::memory().unwrap();
        let (_, control, _) = server(PathBuf::from("."));
        let head = crate::model::RequestId("retired-head".into());
        store.create_request(&head, None, "/root/old").unwrap();
        store
            .append_items(
                &head,
                &[crate::item::Item(serde_json::json!({
                    "type":"message", "role":"assistant", "content":"retained worker reply"
                }))],
            )
            .unwrap();
        store
            .record_event(Some(&head), "model_turn", &serde_json::json!({}))
            .unwrap();
        let mut actor = projected_actor("run", "/root/old", "1");
        actor.kind = HostActorKind::Model;
        actor.lifecycle = HostActorLifecycle::Retired;
        actor.model_conversation = Some("/root/old".into());
        actor.model_head_request = Some(head.0.clone());
        control.update_host_projection("run".into(), vec![actor.clone()], vec![]);
        control.refresh_completed_model_requests(&store).unwrap();
        for index in 0..130 {
            let request = crate::model::RequestId(format!("sibling-{index}"));
            store
                .create_request(&request, None, "/root/sibling")
                .unwrap();
            store
                .record_event(Some(&request), "model_turn", &serde_json::json!({}))
                .unwrap();
        }
        control.refresh_completed_model_requests(&store).unwrap();
        let fresh_snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(fresh_snapshot.requests.len(), 128);
        assert!(
            fresh_snapshot
                .requests
                .iter()
                .all(|request| request["id"] != head.0)
        );
        assert_eq!(fresh_snapshot.actors, vec![actor]);
        let wire = serde_json::to_value(WsServerFrame::Snapshot {
            snapshot: fresh_snapshot,
        })
        .unwrap();
        assert_eq!(wire["snapshot"]["actors"][0]["modelHeadRequest"], head.0);
        let page = store.history_page(&head, 0, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].item.0["content"], "retained worker reply");
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
            tools: vec![].into(),
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
            model_head_request: None,
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
            live_output: Vec::new(),
            history_revisions: Vec::new(),
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

    #[derive(Default)]
    struct TestPeerAuth {
        outcome: std::sync::atomic::AtomicU8,
        peers: std::sync::Mutex<Vec<SocketAddr>>,
    }

    #[async_trait::async_trait]
    impl BrowserPeerAuthenticator for TestPeerAuth {
        async fn authenticate(&self, peer: SocketAddr) -> Result<(), PeerAuthError> {
            self.peers.lock().unwrap().push(peer);
            match self.outcome.load(std::sync::atomic::Ordering::SeqCst) {
                0 => Ok(()),
                1 => Err(PeerAuthError::Denied),
                _ => Err(PeerAuthError::Unavailable),
            }
        }
    }

    fn peer_server(
        auth: Arc<TestPeerAuth>,
        origin: String,
    ) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
        server_with_config(
            ServerConfig::new(PathBuf::from("."))
                .with_bearer_secret(BearerSecret::new(TEST_SECRET).unwrap())
                .with_browser_peer_auth(auth)
                .unwrap()
                .with_public_origin(origin)
                .unwrap(),
        )
    }

    #[test]
    fn browser_peer_auth_is_exclusive_with_secret_in_both_builder_orders() {
        let auth = Arc::new(TestPeerAuth::default());
        assert!(
            ServerConfig::new(PathBuf::from("."))
                .with_browser_peer_auth(auth.clone())
                .unwrap()
                .with_browser_session(
                    SessionSecret::new(TEST_SESSION_SECRET).unwrap(),
                    Duration::from_secs(60)
                )
                .is_err()
        );
        assert!(
            ServerConfig::new(PathBuf::from("."))
                .with_browser_session(
                    SessionSecret::new(TEST_SESSION_SECRET).unwrap(),
                    Duration::from_secs(60)
                )
                .unwrap()
                .with_browser_peer_auth(auth)
                .is_err()
        );
    }

    #[test]
    fn browser_peer_auth_public_origin_requires_exact_http_authority() {
        for origin in [
            "http://harness.example:1234",
            "https://harness.example",
            "http://[::1]:1234/",
        ] {
            assert!(
                ServerConfig::new(PathBuf::from("."))
                    .with_public_origin(origin)
                    .is_ok(),
                "{origin}"
            );
        }
        for origin in [
            "harness.example",
            "ftp://harness.example",
            "https://operator@harness.example",
            "https://harness.example/path",
            "https://harness.example?query",
            "https://harness.example/#fragment",
        ] {
            assert!(
                ServerConfig::new(PathBuf::from("."))
                    .with_public_origin(origin)
                    .is_err(),
                "{origin}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_rejects_rebound_host_and_unpinned_authority() {
        let auth = Arc::new(TestPeerAuth::default());
        for pinned in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let config = ServerConfig::new(PathBuf::from("."))
                .with_browser_peer_auth(auth.clone())
                .unwrap();
            let config = if pinned {
                config
                    .with_public_origin(format!("http://{address}"))
                    .unwrap()
            } else {
                config
            };
            let (app, _, mut commands) = server_with_config(config);
            let server_task = tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                .unwrap()
            });
            let client = reqwest::Client::new();
            let response = client
                .post(format!("http://{address}/api/commands"))
                .header(header::HOST, "attacker.invalid")
                .header(header::ORIGIN, "http://attacker.invalid")
                .json(&ClientCommand::Submit {
                    command: "rebound".into(),
                })
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                client
                    .get(format!("http://{address}/api/history/any"))
                    .header(header::HOST, "attacker.invalid")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
            let status = client
                .get(format!("http://{address}/api/session"))
                .header(header::HOST, "attacker.invalid")
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap();
            assert_eq!(status["authenticated"], false);
            assert!(commands.try_recv().is_err());
            server_task.abort();
        }
        assert!(auth.peers.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_revalidates_after_delayed_command_body() {
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, mut commands) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        for outcome in [1, 2] {
            auth.outcome.store(0, std::sync::atomic::Ordering::SeqCst);
            let observed_before = auth.peers.lock().unwrap().len();
            let body = serde_json::to_string(&ClientCommand::Submit {
                command: "revoked after headers".into(),
            })
            .unwrap();
            let mut socket = TcpStream::connect(address).unwrap();
            let actual_peer = socket.local_addr().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(socket, "POST /api/commands HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while auth.peers.lock().unwrap().len() == observed_before {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(auth.peers.lock().unwrap().last(), Some(&actual_peer));
            auth.outcome
                .store(outcome, std::sync::atomic::Ordering::SeqCst);
            socket.write_all(body.as_bytes()).unwrap();
            let response = tokio::task::spawn_blocking(move || {
                let mut response = String::new();
                socket.read_to_string(&mut response).unwrap();
                response
            })
            .await
            .unwrap();
            let expected = if outcome == 1 {
                "HTTP/1.1 401"
            } else {
                "HTTP/1.1 503"
            };
            assert!(response.starts_with(expected), "{response}");
            assert!(commands.try_recv().is_err());
        }
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_revalidates_after_waiting_for_command_capacity() {
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, mut commands) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let client = reqwest::Client::new();
        let origin = format!("http://{address}");
        for index in 0..COMMAND_CAPACITY {
            assert_eq!(
                client
                    .post(format!("{origin}/api/commands"))
                    .header(header::ORIGIN, &origin)
                    .json(&ClientCommand::Submit {
                        command: format!("fill-{index}")
                    })
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::ACCEPTED
            );
        }
        let observed_before = auth.peers.lock().unwrap().len();
        let request_origin = origin.clone();
        let waiting = tokio::spawn(async move {
            client
                .post(format!("{request_origin}/api/commands"))
                .header(header::ORIGIN, &request_origin)
                .json(&ClientCommand::Submit {
                    command: "revoked while waiting".into(),
                })
                .send()
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while auth.peers.lock().unwrap().len() < observed_before + 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        auth.outcome.store(1, std::sync::atomic::Ordering::SeqCst);
        commands.recv().await.unwrap();
        assert_eq!(waiting.await.unwrap().status(), StatusCode::UNAUTHORIZED);
        for _ in 1..COMMAND_CAPACITY {
            commands.recv().await.unwrap();
        }
        assert!(commands.try_recv().is_err());
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_uses_transport_peer_and_retains_origin_checks() {
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, mut commands) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let client = reqwest::Client::new();
        let origin = format!("http://{address}");
        let status = client
            .get(format!("{origin}/api/session"))
            .header("x-forwarded-for", "100.100.100.100:443")
            .header("forwarded", "for=100.100.100.100")
            .header("tailscale-user-login", "forged@example.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        assert_eq!(status.headers()[header::CACHE_CONTROL], "no-store");
        assert!(status.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            status.json::<serde_json::Value>().await.unwrap(),
            json!({"authenticated":true,"authentication":"tailscale","available":true})
        );
        let observed = auth.peers.lock().unwrap().clone();
        assert_eq!(observed.len(), 1);
        assert!(observed[0].ip().is_loopback());
        assert_ne!(observed[0].port(), address.port());

        for method in [reqwest::Method::POST, reqwest::Method::DELETE] {
            let response = client
                .request(method, format!("{origin}/api/session"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert!(response.headers().get(header::SET_COOKIE).is_none());
        }
        for requested_origin in [
            None,
            Some("http://attacker.invalid"),
            Some("https://invalid.example"),
        ] {
            let mut request = client
                .post(format!("{origin}/api/commands"))
                .header("x-forwarded-proto", "https")
                .json(&ClientCommand::Submit {
                    command: "csrf".into(),
                });
            if let Some(origin) = requested_origin {
                request = request.header(header::ORIGIN, origin);
            }
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        let accepted = client
            .post(format!("{origin}/api/commands"))
            .header(header::ORIGIN, &origin)
            .json(&ClientCommand::Submit {
                command: "accepted".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert!(
            matches!(commands.recv().await.unwrap().command, ClientCommand::Submit { command } if command == "accepted")
        );
        assert_eq!(
            client
                .get(format!("{origin}/api/commands/{}", uuid::Uuid::new_v4()))
                .header(header::ORIGIN, "http://attacker.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        for ws_origin in [None, Some("http://attacker.invalid")] {
            let (_, handshake) = websocket(address, ws_origin, Some(TEST_SECRET));
            assert!(handshake.starts_with("HTTP/1.1 403"), "{handshake}");
        }
        auth.outcome.store(1, std::sync::atomic::Ordering::SeqCst);
        for path in ["events", "history/any", "ws"] {
            let denied = client
                .get(format!("{origin}/api/{path}"))
                .bearer_auth(TEST_SECRET)
                .header(header::COOKIE, "harness_session=forged")
                .header("x-forwarded-for", "100.100.100.100:443")
                .header("tailscale-user-login", "forged@example.invalid")
                .send()
                .await
                .unwrap();
            assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        }
        let denied = client
            .get(format!("{origin}/api/session"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.json::<serde_json::Value>().await.unwrap(),
            json!({"authenticated":false,"authentication":"tailscale","available":true})
        );
        auth.outcome.store(2, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            client
                .get(format!("{origin}/api/history/any"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let unavailable = client
            .get(format!("{origin}/api/session"))
            .send()
            .await
            .unwrap();
        assert_eq!(unavailable.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            unavailable.json::<serde_json::Value>().await.unwrap(),
            json!({"authenticated":false,"authentication":"tailscale","available":false})
        );
        assert!(
            auth.peers
                .lock()
                .unwrap()
                .iter()
                .all(|peer| peer.ip().is_loopback())
        );
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_missing_connect_info_fails_closed() {
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, _) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{address}/api/history/any"))
            .bearer_auth(TEST_SECRET)
            .header(header::COOKIE, "harness_session=forged")
            .header("x-forwarded-for", "127.0.0.1:12345")
            .header("forwarded", "for=127.0.0.1")
            .header("tailscale-user-login", "forged@example.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(auth.peers.lock().unwrap().is_empty());
        assert_eq!(
            client
                .get(format!("http://{address}/api/session"))
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap(),
            json!({"authenticated":false,"authentication":"tailscale","available":true})
        );
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_revalidates_websocket_commands_and_snapshots() {
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, mut commands) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        for frame in [
            json!({"type":"command","command":"revoked"}),
            json!({"type":"snapshot.request"}),
        ] {
            auth.outcome.store(0, std::sync::atomic::Ordering::SeqCst);
            let (mut ws, handshake) = websocket(address, Some(&format!("http://{address}")), None);
            assert!(handshake.starts_with("HTTP/1.1 101"), "{handshake}");
            assert_eq!(read_ws_text(&mut ws)["type"], "snapshot");
            auth.outcome.store(1, std::sync::atomic::Ordering::SeqCst);
            write_ws_text(&mut ws, &frame.to_string());
            let mut close = [0u8; 2];
            ws.read_exact(&mut close).unwrap();
            assert_eq!(close[0] & 0x0f, 8, "revoked socket must close");
        }
        assert!(commands.try_recv().is_err());
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_revokes_idle_websocket_and_sse() {
        use futures_util::StreamExt;
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, _, _) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let (mut ws, handshake) = websocket(address, Some(&format!("http://{address}")), None);
        assert!(handshake.starts_with("HTTP/1.1 101"), "{handshake}");
        assert_eq!(read_ws_text(&mut ws)["type"], "snapshot");
        ws.set_read_timeout(Some(Duration::from_secs(35))).unwrap();
        let response = reqwest::get(format!("http://{address}/api/events"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut events = response.bytes_stream();
        auth.outcome.store(2, std::sync::atomic::Ordering::SeqCst);
        let ws_closed = tokio::task::spawn_blocking(move || {
            let mut close = [0u8; 2];
            ws.read_exact(&mut close).unwrap();
            assert_eq!(close[0] & 0x0f, 8);
        });
        tokio::time::timeout(Duration::from_secs(35), async {
            while let Some(chunk) = events.next().await {
                let chunk = chunk.unwrap();
                assert!(!String::from_utf8_lossy(&chunk).contains("data:"));
            }
            ws_closed.await.unwrap();
        })
        .await
        .expect("idle streams revoked within their 30 second interval");
        server_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn browser_peer_auth_revokes_busy_sse_without_verifying_every_event() {
        use futures_util::StreamExt;
        let auth = Arc::new(TestPeerAuth::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (app, control, _) = peer_server(auth.clone(), format!("http://{address}"));
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let response = reqwest::get(format!("http://{address}/api/events"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(auth.peers.lock().unwrap().len(), 1);
        let mut events = response.bytes_stream();
        auth.outcome.store(1, std::sync::atomic::Ordering::SeqCst);
        let publisher = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;
                control.publish("token", json!({"text":"live output"}));
            }
        });
        let mut chunks = 0;
        tokio::time::timeout(Duration::from_secs(35), async {
            while let Some(chunk) = events.next().await {
                chunk.unwrap();
                chunks += 1;
            }
        })
        .await
        .expect("periodic peer revalidation must run while events remain ready");
        assert!(chunks > 0);
        assert_eq!(
            auth.peers.lock().unwrap().len(),
            2,
            "initial and periodic checks only"
        );
        publisher.abort();
        server_task.abort();
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
            serde_json::json!({"authenticated":true,"authentication":"secret","available":true})
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
        assert_eq!(
            status,
            serde_json::json!({"authenticated":true,"authentication":"secret","available":true})
        );

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
        assert_eq!(
            status,
            serde_json::json!({"authenticated":false,"authentication":"secret","available":true})
        );
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
    async fn frontend_static_routes_serve_pages_without_swallowing_assets_or_api_errors() {
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
        for page in ["/tree", "/chat", "/chat/root/worker?run=r&incarnation=i"] {
            let response = client
                .get(format!("http://{address}{page}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{page}");
            assert_eq!(response.text().await.unwrap(), "<main>app</main>");
        }
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
        let unknown = client
            .get(format!("http://{address}/unknown"))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let unknown_api = client
            .get(format!("http://{address}/api/unknown"))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_api.status(), StatusCode::NOT_FOUND);
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
        assert!(same_origin(&headers, "https", None));
        headers.insert("x-forwarded-proto", "attacker-controlled".parse().unwrap());
        assert!(same_origin(&headers, "https", None));
        assert!(!same_origin(&headers, "http", None));
        headers.insert(
            axum::http::header::ORIGIN,
            "https://other.test".parse().unwrap(),
        );
        assert!(!same_origin(&headers, "https", None));
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

#[cfg(test)]
mod rejection_projection_tests {
    use super::*;

    #[test]
    fn rejected_request_survives_projection_refresh_and_reconnect() {
        let store = Store::memory().unwrap();
        let request = crate::model::RequestId("rejected-request".into());
        store.create_request(&request, None, "/root").unwrap();
        let failure = serde_json::json!({"kind":"http", "status":400, "diagnostic":{"code":"invalid_function_parameters"}});
        store
            .record_event(Some(&request), "request_failed", &failure)
            .unwrap();
        let (_, control, _) = server(PathBuf::from("."));
        let mut events = control.events.subscribe();
        control.refresh_completed_model_requests(&store).unwrap();
        let event = events.try_recv().unwrap();
        assert_eq!(event.event, "request.upsert");
        assert_eq!(event.payload["id"], request.0);
        assert_eq!(event.payload["state"], "failed");
        assert_eq!(event.payload["failure"], failure);
        assert_eq!(control.snapshot.read().unwrap().requests[0], event.payload);
        control.refresh_completed_model_requests(&store).unwrap();
        assert!(
            events.try_recv().is_err(),
            "failure was republished without change"
        );
    }
}
