//! Headless HTTP boundary for submitting commands and following harness events.
//!
//! The scheduler owns the command receiver and publishes events through the
//! returned control handle. The module intentionally does not depend on a
//! scheduler/store implementation so the crate owner can wire it independently.
mod assets;
mod ws_protocol;

pub use ws_protocol::{Snapshot, WsClientFrame, WsEvent, WsEventPayload, WsServerFrame};

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode, Uri},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{broadcast, mpsc};

const COMMAND_CAPACITY: usize = 128;
const EVENT_CAPACITY: usize = 512;

/// Stable command envelope accepted by `POST /api/commands`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Submit { command: String },
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

#[derive(Clone)]
struct AppState {
    commands: mpsc::Sender<QueuedCommand>,
    events: broadcast::Sender<ServerEvent>,
    asset_root: Arc<PathBuf>,
    snapshot: Arc<std::sync::RwLock<Snapshot>>,
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
        *self.snapshot.write().expect("snapshot lock poisoned") = snapshot;
    }
}

/// Build the HTTP router, scheduler command receiver and event producer.
///
/// `asset_root` points at a future web build directory. `/` and `/assets/*`
/// are served from it; the helper rejects traversal and falls back to index.
pub fn server(asset_root: PathBuf) -> (Router, ServerControl, mpsc::Receiver<QueuedCommand>) {
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    let state = AppState {
        commands,
        events: events.clone(),
        asset_root: Arc::new(asset_root),
        snapshot: Arc::new(std::sync::RwLock::new(Snapshot::default())),
    };
    let snapshot = state.snapshot.clone();
    let control = ServerControl {
        events,
        next_sequence: Arc::new(std::sync::Mutex::new(1)),
        snapshot,
    };
    let router = Router::new()
        .route("/api/commands", post(submit_command))
        .route("/api/events", get(event_stream))
        .route("/api/ws", get(websocket))
        .route("/", get(index_asset))
        .route("/{*path}", get(static_asset))
        .with_state(state);
    (router, control, receiver)
}

async fn websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, StatusCode> {
    if !same_origin(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    // Subscribe before upgrading, so events published during the handshake are
    // buffered and delivered after the initial snapshot.
    let receiver = state.events.subscribe();
    Ok(upgrade.on_upgrade(move |socket| websocket_session(socket, state, receiver)))
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        // Native clients commonly omit Origin. Browser requests always include it.
        return true;
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
    let transport_scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    if !matches!(scheme, "http" | "https") || scheme != transport_scheme {
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
) {
    let mut last_sent = send_snapshot(&mut socket, &state).await.unwrap_or(0);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(frame) = serde_json::from_str::<WsClientFrame>(&text) else { continue };
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
    Json(command): Json<ClientCommand>,
) -> Result<(StatusCode, Json<CommandAccepted>), StatusCode> {
    let command_id = uuid::Uuid::new_v4().to_string();
    state
        .commands
        .send(QueuedCommand {
            command_id: command_id.clone(),
            command,
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok((StatusCode::ACCEPTED, Json(CommandAccepted { command_id })))
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
    use super::*;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

    fn websocket(address: std::net::SocketAddr, origin: &str) -> (TcpStream, String) {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            socket,
            "GET /api/ws HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\n\r\n"
        )
        .unwrap();
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

    #[tokio::test]
    async fn command_route_queues_submission_and_event_route_streams_events() {
        let (app, control, mut commands) = server(PathBuf::from("."));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();

        let response = client
            .post(format!("http://{address}/api/commands"))
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
        let (app, _, _) = server(root.clone());
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
        let (app, control, mut commands) = server(PathBuf::from("."));
        control.set_snapshot(Snapshot {
            seq: 4,
            conversations: vec![json!({"id":"root"})],
            ..Snapshot::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (mut socket, handshake) = websocket(address, &format!("http://{address}"));
        assert!(handshake.starts_with("HTTP/1.1 101"), "{handshake}");
        assert_eq!(
            read_ws_text(&mut socket),
            json!({"type":"snapshot","snapshot":{
                "seq":4,"conversations":[{"id":"root"}],"requests":[],"jobs":[],"envelopes":[]
            }})
        );

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
        let (app, _, _) = server(PathBuf::from("."));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_socket, response) = websocket(address, "http://attacker.invalid");
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
        server_task.abort();
    }

    #[test]
    fn origin_check_matches_host_and_proxy_transport_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "example.test".parse().unwrap());
        headers.insert(
            axum::http::header::ORIGIN,
            "https://example.test".parse().unwrap(),
        );
        assert!(!same_origin(&headers));
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(same_origin(&headers));
        headers.insert(
            axum::http::header::ORIGIN,
            "https://other.test".parse().unwrap(),
        );
        assert!(!same_origin(&headers));
    }
}
