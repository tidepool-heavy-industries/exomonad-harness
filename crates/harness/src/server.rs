//! Headless HTTP boundary for submitting commands and following harness events.
//!
//! The scheduler owns the command receiver and publishes events through the
//! returned control handle. The module intentionally does not depend on a
//! scheduler/store implementation so the crate owner can wire it independently.
mod assets;

use axum::{
    extract::State,
    http::StatusCode,
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
        let _ = self.events.send(event.clone());
        drop(next_sequence);
        event
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
    };
    let control = ServerControl {
        events,
        next_sequence: Arc::new(std::sync::Mutex::new(1)),
    };
    let router = Router::new()
        .route("/api/commands", post(submit_command))
        .route("/api/events", get(event_stream))
        .route("/", get(index_asset))
        .route("/{*path}", get(static_asset))
        .with_state(state);
    (router, control, receiver)
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
}
