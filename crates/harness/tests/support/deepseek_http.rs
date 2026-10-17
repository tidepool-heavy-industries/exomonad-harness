//! Loopback HTTP fixture. Tests drive the real ResponsesClient and answer its
//! captured POSTs; this module never constructs provider dispatch evidence.

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    routing::post,
};
use futures_util::stream;
use serde_json::Value;
use std::{io, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};

const REQUEST_LIMIT: usize = 4 * 1024 * 1024;

pub struct LocalResponses {
    pub endpoint: String,
    requests: mpsc::UnboundedReceiver<PendingRequest>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<io::Result<()>>>,
}

pub struct CapturedRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
}

impl CapturedRequest {
    pub fn json(&self) -> Value {
        let bytes = match self.headers.get("content-encoding") {
            None => self.bytes.clone(),
            Some(encoding) if encoding == "zstd" => {
                zstd::stream::decode_all(self.bytes.as_slice()).expect("decode OpenAI request body")
            }
            Some(encoding) => panic!("unexpected request content encoding: {encoding:?}"),
        };
        serde_json::from_slice(&bytes).expect("client POST contains valid UTF-8 JSON")
    }
}

pub struct PendingRequest {
    pub captured: CapturedRequest,
    reply: oneshot::Sender<HttpReply>,
}

pub struct HttpReply {
    status: StatusCode,
    content_type: &'static str,
    body: Body,
}

impl PendingRequest {
    pub fn respond(self, reply: HttpReply) {
        assert!(
            self.reply.send(reply).is_ok(),
            "client disconnected before reply"
        );
    }
}

impl HttpReply {
    pub fn json(status: StatusCode, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: Body::from(serde_json::to_vec(&body).unwrap()),
        }
    }

    /// Each element is an HTTP body chunk. Splitting bytes inside UTF-8 is
    /// intentional; semantic events are encoded before this chunking step.
    pub fn fragmented_sse(bytes: Vec<u8>, widths: &[usize]) -> Self {
        assert!(!widths.is_empty());
        assert!(widths.iter().all(|width| *width > 0));
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let end = (offset + widths[chunks.len() % widths.len()]).min(bytes.len());
            chunks.push(Ok::<_, io::Error>(Bytes::copy_from_slice(
                &bytes[offset..end],
            )));
            offset = end;
        }
        Self {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: Body::from_stream(stream::iter(chunks)),
        }
    }

    /// The test controls exact chunk delivery, terminal-before-EOF, and read
    /// failure schedules without replacing the HTTP client or SSE decoder.
    pub fn controlled_sse() -> (Self, mpsc::Sender<Result<Bytes, io::Error>>) {
        let (sender, receiver) = mpsc::channel(4);
        let chunks = stream::unfold(receiver, |mut receiver| async {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });
        (
            Self {
                status: StatusCode::OK,
                content_type: "text/event-stream",
                body: Body::from_stream(chunks),
            },
            sender,
        )
    }
}

pub fn semantic_events(events: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events {
        bytes.extend_from_slice(b"data: ");
        serde_json::to_writer(&mut bytes, event).unwrap();
        bytes.extend_from_slice(b"\n\n");
    }
    bytes
}

async fn receive(
    State(requests): State<Arc<mpsc::UnboundedSender<PendingRequest>>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, REQUEST_LIMIT)
        .await
        .expect("fixture client request must fit declared capture budget");
    let (reply, response) = oneshot::channel();
    let pending = PendingRequest {
        captured: CapturedRequest {
            method: parts.method,
            uri: parts.uri,
            headers: parts.headers,
            bytes: bytes.to_vec(),
        },
        reply,
    };
    if requests.send(pending).is_err() {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::empty())
            .unwrap();
    }
    match response.await {
        Ok(reply) => Response::builder()
            .status(reply.status)
            .header("content-type", reply.content_type)
            .body(reply.body)
            .unwrap(),
        Err(_) => Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::empty())
            .unwrap(),
    }
}

impl LocalResponses {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback fixture");
        let address = listener.local_addr().unwrap();
        let (requests, received) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/responses", post(receive))
            .route("/v1/responses", post(receive))
            .with_state(Arc::new(requests));
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            endpoint: format!("http://{address}/responses"),
            requests: received,
            stop: Some(stop),
            task: Some(task),
        }
    }

    pub async fn next(&mut self) -> PendingRequest {
        tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .expect("production client must reach fixture within its local request budget")
            .expect("fixture request owner remains open")
    }

    pub async fn finish(mut self) {
        assert!(
            self.requests.try_recv().is_err(),
            "unexpected extra HTTP request"
        );
        self.requests.close();
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(5), self.task.as_mut().unwrap())
            .await
            .expect("fixture HTTP connections must retire")
            .expect("fixture server task")
            .expect("fixture server IO");
        self.task.take();
    }
}

impl Drop for LocalResponses {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
