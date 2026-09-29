use std::{collections::HashSet, net::TcpListener, process::Stdio, time::Duration};

use harness::{model::RequestId, store::Store};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

const SESSION_SECRET: &str = "browser-journey-test-secret-at-least-32-bytes";
type BrowserSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Demo {
    child: Child,
    base: String,
    db: std::path::PathBuf,
    client: reqwest::Client,
    cookie: String,
}

impl Demo {
    fn spawn_server(db: &std::path::Path, address: &str) -> Child {
        Command::new(env!("CARGO_BIN_EXE_harness-demo"))
            .args(["--db", db.to_str().unwrap(), "--serve", address])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("HARNESS_DEMO_SESSION_SECRET", SESSION_SECRET)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start production --serve binary")
    }

    async fn wait_ready(&mut self) {
        let login_url = format!("{}/api/session", self.base);
        let mut ready = false;
        for _ in 0..100 {
            if let Ok(response) = self.client.get(&login_url).send().await {
                if response.status().is_success() {
                    ready = true;
                    break;
                }
            }
            if self
                .child
                .try_wait()
                .expect("check server process")
                .is_some()
            {
                panic!("production server exited before becoming ready");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ready, "production server did not become ready");
    }

    async fn restart(&mut self) {
        self.child
            .start_kill()
            .expect("stop original server process");
        self.child
            .wait()
            .await
            .expect("reap original server process");
        let address = self.base.strip_prefix("http://").unwrap();
        self.child = Self::spawn_server(&self.db, address);
        self.wait_ready().await;
        self.login().await;
    }

    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
        let address = listener.local_addr().unwrap();
        drop(listener);
        let base = format!("http://{address}");
        let db = std::env::temp_dir().join(format!(
            "harness-browser-journey-{}-{}.sqlite",
            std::process::id(),
            uuid_suffix()
        ));
        let client = reqwest::Client::new();
        let mut demo = Self {
            child: Self::spawn_server(&db, &address.to_string()),
            base,
            db,
            client,
            cookie: String::new(),
        };
        demo.wait_ready().await;
        demo.login().await;
        demo
    }

    async fn login(&mut self) {
        let login = self
            .client
            .post(&format!("{}/api/session", self.base))
            .header("origin", &self.base)
            .json(&json!({"secret": SESSION_SECRET}))
            .send()
            .await
            .unwrap();
        assert!(login.status().is_success(), "browser-session login failed");
        self.cookie = login
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
    }

    async fn websocket(&self) -> BrowserSocket {
        let url = self.base.replacen("http://", "ws://", 1) + "/api/ws";
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", self.base.parse().expect("valid origin header"));
        request
            .headers_mut()
            .insert("cookie", self.cookie.parse().expect("valid session cookie"));
        let (socket, _) = connect_async(request).await.expect("authenticated /api/ws");
        socket
    }

    async fn submit_ws(socket: &mut BrowserSocket, command: &str) -> String {
        let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_mut() else {
            panic!("loopback WebSocket unexpectedly negotiated TLS");
        };
        let text = json!({"type":"command","command":command}).to_string();
        let bytes = text.as_bytes();
        assert!(bytes.len() < 126, "test WebSocket command too large");
        let mask = [0x35, 0xa1, 0x6c, 0x09];
        let mut frame = vec![0x81, 0x80 | bytes.len() as u8];
        frame.extend_from_slice(&mask);
        frame.extend(bytes.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        stream.write_all(&frame).await.unwrap();
        loop {
            let frame = next_frame(socket).await;
            if frame["type"] == "command.accepted" {
                return frame["command_id"].as_str().unwrap().to_owned();
            }
        }
    }

    async fn close_ws(socket: &mut BrowserSocket) {
        let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_mut() else {
            panic!("loopback WebSocket unexpectedly negotiated TLS");
        };
        stream
            .write_all(&[0x88, 0x80, 0x35, 0xa1, 0x6c, 0x09])
            .await
            .unwrap();
    }

    async fn request_snapshot(socket: &mut BrowserSocket) {
        let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_mut() else {
            panic!("loopback WebSocket unexpectedly negotiated TLS");
        };
        let text = json!({"type":"snapshot.request"}).to_string();
        let bytes = text.as_bytes();
        let mask = [0x35, 0xa1, 0x6c, 0x09];
        let mut frame = vec![0x81, 0x80 | bytes.len() as u8];
        frame.extend_from_slice(&mask);
        frame.extend(bytes.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        stream.write_all(&frame).await.unwrap();
    }

    async fn submit(&self, command: &str) -> String {
        let response = self
            .client
            .post(format!("{}/api/commands", self.base))
            .header("origin", &self.base)
            .header("cookie", &self.cookie)
            .json(&json!({"type":"submit","command":command}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        response.json::<Value>().await.unwrap()["command_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn uuid_suffix() -> String {
    // A per-test database name without adding an unrelated random-number API.
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn next_frame(socket: &mut BrowserSocket) -> Value {
    loop {
        let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_mut() else {
            panic!("loopback WebSocket unexpectedly negotiated TLS");
        };
        let mut header = [0; 2];
        stream
            .read_exact(&mut header)
            .await
            .expect("WebSocket frame header");
        let length = match header[1] & 0x7f {
            126 => {
                let mut size = [0; 2];
                stream
                    .read_exact(&mut size)
                    .await
                    .expect("WebSocket frame length");
                u16::from_be_bytes(size) as usize
            }
            127 => {
                let mut size = [0; 8];
                stream
                    .read_exact(&mut size)
                    .await
                    .expect("WebSocket frame length");
                u64::from_be_bytes(size) as usize
            }
            size => size as usize,
        };
        let mut payload = vec![0; length];
        stream
            .read_exact(&mut payload)
            .await
            .expect("WebSocket payload read");
        if header[0] & 0x0f == 1 {
            return serde_json::from_slice(&payload).expect("JSON server frame");
        }
    }
}

fn command_record<'a>(snapshot: &'a Value, command_id: &str) -> Option<&'a Value> {
    snapshot["snapshot"]["requests"]
        .as_array()?
        .iter()
        .find(|request| request["commandId"] == command_id)
}

async fn wait_for_outcome(
    socket: &mut BrowserSocket,
    command_id: &str,
    outcomes: &[&str],
) -> Value {
    Demo::request_snapshot(socket).await;
    let mut last = Value::Null;
    timeout(Duration::from_secs(10), async {
        loop {
            let frame = next_frame(socket).await;
            last = frame.clone();
            if frame["type"] == "snapshot" {
                if let Some(request) = command_record(&frame, command_id) {
                    if outcomes.contains(&request["outcome"].as_str().unwrap_or_default()) {
                        return frame;
                    }
                }
            } else if frame["type"] == "event"
                && frame["event"]["event"]["kind"] == "request.upsert"
            {
                let request = &frame["event"]["event"]["value"];
                assert!(
                    request["commandId"].is_string() && request["outcome"].is_string(),
                    "request.upsert omitted the contract's commandId/outcome fields"
                );
                if request["commandId"] == command_id
                    && outcomes.contains(&request["outcome"].as_str().unwrap_or_default())
                {
                    return json!({"snapshot":{"requests":[request.clone()]}});
                }
            }
        }
    })
    .await
    .unwrap_or_else(|e| panic!("command {command_id} expected {outcomes:?}: {e}; last={last}"))
}

fn custom_started_call_id(row: &Value, expected_input_len: usize) -> Option<String> {
    if row["type"] != "PROGRESS" {
        return None;
    }
    let payload = serde_json::from_str::<Value>(row["payload"].as_str()?).ok()?;
    if payload["event"] == "custom_started" && payload["inputLength"] == json!(expected_input_len) {
        payload["callId"].as_str().map(ToOwned::to_owned)
    } else {
        None
    }
}

fn same_branch_custom_outputs(
    store: &Store,
    emitting_request: &RequestId,
    call_id: &str,
) -> Vec<harness::item::Item> {
    let emitting = store
        .request(emitting_request)
        .expect("read emitting custom request")
        .expect("emitting custom request missing");
    let mut pending = vec![emitting.id.clone()];
    let mut visited = HashSet::new();
    let mut outputs = Vec::new();
    while let Some(request) = pending.pop() {
        assert!(
            visited.insert(request.0.clone()),
            "same-branch request lineage contains a cycle"
        );
        assert!(
            visited.len() <= 128,
            "custom journey request lineage exceeded bound"
        );
        outputs.extend(
            store
                .items(&request)
                .expect("read same-branch completion items")
                .into_iter()
                .filter(|item| {
                    item.0["type"] == "custom_tool_call_output" && item.0["call_id"] == call_id
                }),
        );
        for child in store
            .children_of(&request)
            .expect("walk same-branch custom descendants")
        {
            if child.branch == emitting.branch {
                pending.push(child.id);
            }
        }
    }
    outputs
}

async fn wait_for_custom_start(socket: &mut BrowserSocket, expected_input_len: usize) -> String {
    Demo::request_snapshot(socket).await;
    timeout(Duration::from_secs(10), async {
        loop {
            let frame = next_frame(socket).await;
            if frame["type"] == "snapshot" {
                if let Some(call_id) = frame["snapshot"]["envelopes"].as_array().and_then(|rows| {
                    rows.iter()
                        .find_map(|row| custom_started_call_id(row, expected_input_len))
                }) {
                    return call_id;
                }
            } else if frame["type"] == "event"
                && frame["event"]["event"]["kind"] == "envelope.upsert"
            {
                if let Some(call_id) =
                    custom_started_call_id(&frame["event"]["event"]["value"], expected_input_len)
                {
                    return call_id;
                }
            }
        }
    })
    .await
    .expect("custom evaluator did not publish its call-scoped start barrier")
}

async fn fresh_snapshot(socket: &mut BrowserSocket) -> Value {
    Demo::request_snapshot(socket).await;
    timeout(Duration::from_secs(5), async {
        loop {
            let frame = next_frame(socket).await;
            if frame["type"] == "snapshot" {
                return frame;
            }
        }
    })
    .await
    .expect("fresh WebSocket snapshot timeout")
}

#[tokio::test]
async fn browser_journey_auth_pending_cancel_child_failure_and_reconnect() {
    let demo = Demo::start().await;

    // The browser-session cookie protects both the real HTTP command endpoint
    // and the existing WebSocket snapshot/event stream.
    let unauthenticated = demo
        .client
        .get(format!("{}/api/session", demo.base))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::OK);
    assert_eq!(
        unauthenticated.json::<Value>().await.unwrap()["authenticated"],
        false
    );
    let denied = demo
        .client
        .post(format!("{}/api/commands", demo.base))
        .json(&json!({"type":"submit","command":"echo must-not-run"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);

    let mut ws = demo.websocket().await;
    let initial = timeout(Duration::from_secs(5), next_frame(&mut ws))
        .await
        .expect("initial WebSocket snapshot timeout");
    assert_eq!(initial["type"], "snapshot");

    // A pending root command must not occupy the receiver: an independent
    // cancel command has to be accepted and produce a durable terminal state.
    let wait_id = Demo::submit_ws(&mut ws, "wait").await;
    let pending = wait_for_outcome(&mut ws, &wait_id, &["pending"]).await;
    assert_eq!(
        command_record(&pending, &wait_id).unwrap()["command"],
        "wait"
    );
    let message_id = demo.submit("message while-wait-is-pending").await;
    let message = wait_for_outcome(&mut ws, &message_id, &["queued", "presented", "acted"]).await;
    assert_eq!(
        command_record(&message, &message_id).unwrap()["command"],
        "message while-wait-is-pending"
    );
    // Cancellation is a server command, not a connection-lifetime side effect.
    // Drop the submitting socket while wait is pending, then cancel over HTTP.
    Demo::close_ws(&mut ws).await;
    drop(ws);
    let cancel_id = demo.submit("cancel").await;
    assert_ne!(wait_id, cancel_id);
    let mut ws = demo.websocket().await;
    let after_cancel_disconnect = timeout(Duration::from_secs(5), next_frame(&mut ws))
        .await
        .expect("post-cancel reconnect snapshot timeout");
    assert_eq!(after_cancel_disconnect["type"], "snapshot");

    // Exercise a child identity and its parent-message/reply path.
    let child_id = demo.submit("child browser-child-proof").await;
    assert!(!child_id.is_empty());

    // A controlled failure must remain a distinct durable outcome and not
    // poison the next successful command.
    let failed_id = demo.submit("fail").await;
    let success_id = demo.submit("echo browser-recovery-proof").await;
    assert_ne!(failed_id, success_id);

    // Observe server-published request transitions, with a bounded frame
    // timeout. The snapshot request is an explicit resync, not a sleep-based
    // assumption that queued work has completed.
    let _ = wait_for_outcome(&mut ws, &wait_id, &["cancelled"]).await;
    let _ = wait_for_outcome(&mut ws, &cancel_id, &["completed"]).await;
    let _ = wait_for_outcome(&mut ws, &child_id, &["completed"]).await;
    let failed = wait_for_outcome(&mut ws, &failed_id, &["failed"]).await;
    assert_eq!(
        command_record(&failed, &failed_id).unwrap()["command"],
        "fail"
    );
    let success = wait_for_outcome(&mut ws, &success_id, &["completed"]).await;
    assert_eq!(
        command_record(&success, &success_id).unwrap()["command"],
        "echo browser-recovery-proof"
    );

    // Reconnect is a snapshot resynchronization, not command replay.
    Demo::close_ws(&mut ws).await;
    let mut reconnected = demo.websocket().await;
    let restored = timeout(Duration::from_secs(5), next_frame(&mut reconnected))
        .await
        .expect("reconnect WebSocket snapshot timeout");
    assert_eq!(restored["type"], "snapshot");
    let requests = restored["snapshot"]["requests"].as_array().unwrap();
    let by_id = |id: &str| requests.iter().find(|r| r["commandId"] == id).unwrap();
    assert_eq!(by_id(&wait_id)["outcome"], "cancelled");
    assert_eq!(by_id(&wait_id)["state"], "failed");
    let jobs = restored["snapshot"]["jobs"].as_array().unwrap();
    let wait_job = jobs
        .iter()
        .find(|job| job["id"] == wait_id)
        .expect("cancelled wait has no associated durable job");
    assert_eq!(wait_job["state"], "cancelled");
    assert!(
        ["queued", "presented", "acted"].contains(&by_id(&message_id)["outcome"].as_str().unwrap())
    );
    assert_eq!(by_id(&cancel_id)["outcome"], "completed");
    assert_eq!(by_id(&child_id)["outcome"], "completed");
    assert_eq!(by_id(&failed_id)["outcome"], "failed");
    assert_eq!(by_id(&success_id)["outcome"], "completed");
    assert_eq!(by_id(&failed_id)["command"], "fail");
    assert_ne!(
        by_id(&failed_id)["commandId"],
        by_id(&success_id)["commandId"]
    );

    let conversations = restored["snapshot"]["conversations"].as_array().unwrap();
    let child_conversation = conversations
        .iter()
        .find(|row| {
            row["id"] != "conversation/root"
                && row["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("/root/"))
        })
        .expect("child command did not create a distinct child conversation");
    let envelopes = restored["snapshot"]["envelopes"].as_array().unwrap();
    assert!(
        envelopes
            .iter()
            .any(|row| { row["type"] == "PROGRESS" && row["ordinal"].as_u64().is_some() }),
        "echo did not persist ordered progress"
    );
    assert!(
        envelopes.iter().any(|row| {
            row["type"] == "FINAL_ANSWER"
                && row["payload"]
                    .as_str()
                    .is_some_and(|text| text.contains("browser-recovery-proof"))
        }),
        "echo final answer did not include its submitted text"
    );
    let ordinals: Vec<u64> = envelopes
        .iter()
        .filter_map(|row| row["ordinal"].as_u64())
        .collect();
    assert!(ordinals.windows(2).all(|pair| pair[0] < pair[1]));
    let child_path = child_conversation["path"].as_str().unwrap();
    let parent_message = envelopes
        .iter()
        .find(|row| row["type"] == "MESSAGE" && row["recipient"] == child_path)
        .expect("child is missing the delivered parent message");
    let child_reply = envelopes
        .iter()
        .find(|row| {
            (row["type"] == "MESSAGE" || row["type"] == "FINAL_ANSWER")
                && row["sender"] == child_path
        })
        .expect("child is missing its sent reply");
    assert!(
        !parent_message["sender"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(
        parent_message["sender"], child_reply["recipient"],
        "child reply is not addressed to the actual parent sender"
    );
    assert!(
        !parent_message["recipient"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !child_reply["sender"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        !child_reply["recipient"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        parent_message["ordinal"].as_u64() < child_reply["ordinal"].as_u64(),
        "child reply appeared before its parent message"
    );
}

#[tokio::test]
async fn browser_journey_custom_call_is_retained_in_server_snapshot_and_store() {
    let mut demo = Demo::start().await;
    let mut ws = demo.websocket().await;
    let initial = timeout(Duration::from_secs(5), next_frame(&mut ws))
        .await
        .expect("initial WebSocket snapshot timeout");
    assert_eq!(initial["type"], "snapshot");

    // The deterministic server fixture emits one raw custom `run` call for
    // this command. Submit through the authenticated production HTTP route,
    // then observe the existing WebSocket projection rather than a test-only
    // endpoint or direct Engine invocation.
    let command_id = demo.submit("custom browser-journey-proof").await;
    let completed = wait_for_outcome(&mut ws, &command_id, &["completed"]).await;
    assert_eq!(
        command_record(&completed, &command_id).unwrap()["command"],
        "custom browser-journey-proof"
    );

    // A reconnect is a read/resync only. The settled job and the original
    // custom call/output pair must still be present in durable evidence.
    Demo::close_ws(&mut ws).await;
    drop(ws);
    demo.restart().await;
    let mut reopened = demo.websocket().await;
    let restored = timeout(Duration::from_secs(5), next_frame(&mut reopened))
        .await
        .expect("reopened WebSocket snapshot timeout");
    assert_eq!(restored["type"], "snapshot");
    let jobs = restored["snapshot"]["jobs"].as_array().unwrap();
    let custom_jobs: Vec<_> = jobs.iter().filter(|job| job["toolName"] == "run").collect();
    assert_eq!(custom_jobs.len(), 1, "custom job was replayed or lost");
    let job = custom_jobs[0];
    let call_id = job["callId"].as_str().expect("custom job callId");
    assert_eq!(job["toolKind"], "custom");
    assert_eq!(job["state"], "settled");
    assert!(
        !job["output"].is_null(),
        "settled custom job has no retained output: {job:#?}"
    );
    let progress = restored["snapshot"]["envelopes"]
        .as_array()
        .expect("server snapshot envelopes");
    // The request acknowledgement proves serviceability, not evaluator progress.
    assert!(
        progress.iter().any(|row| {
            row["type"] == "PROGRESS"
                && row["ordinal"].as_u64().is_some()
                && row["payload"]
                    .as_str()
                    .is_some_and(|text| text.contains(&format!("Request {command_id} accepted")))
        }),
        "custom command did not retain its request-scoped server acknowledgement"
    );
    assert!(
        progress.iter().any(|row| {
            if row["type"] != "PROGRESS" {
                return false;
            }
            row["payload"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .is_some_and(|payload| {
                    payload["event"] == "custom_started"
                        && payload["callId"] == call_id
                        && payload["inputLength"] == json!("browser-journey-proof".len())
                })
        }),
        "original custom call has no retained evaluator start progress"
    );

    let store = Store::open(&demo.db).expect("reopen durable server Store");
    let request_id = RequestId(job["requestId"].as_str().unwrap().to_owned());
    assert!(
        store.request(&request_id).unwrap().is_some(),
        "tool job request provenance must survive process-level Store reopen"
    );
    let items = store
        .items(&request_id)
        .expect("read retained Engine items");
    assert!(
        items.iter().any(|item| {
            item.0["type"] == "custom_tool_call"
                && item.0["name"] == "run"
                && item.0["call_id"] == call_id
                && item.0["input"] == "browser-journey-proof"
        }),
        "Store lost the exact raw custom input for call {call_id}: {items:#?}"
    );
    let outputs = same_branch_custom_outputs(&store, &request_id, call_id);
    assert_eq!(
        outputs.len(),
        1,
        "same-branch completion lost or duplicated custom output for {call_id}: {outputs:#?}"
    );
    Demo::request_snapshot(&mut reopened).await;
    let after_reopen = timeout(Duration::from_secs(5), next_frame(&mut reopened))
        .await
        .expect("WebSocket did not remain serviceable after resync");
    assert_eq!(after_reopen["type"], "snapshot");
    let after_resync_jobs = after_reopen["snapshot"]["jobs"]
        .as_array()
        .expect("resync jobs");
    assert_eq!(
        after_resync_jobs
            .iter()
            .filter(|row| row["callId"] == call_id)
            .count(),
        1,
        "resync must not create a second custom job"
    );
}

#[tokio::test]
async fn browser_journey_custom_raw_input_survives_release_and_process_reopen() {
    const RAW: &str = "printf 'line\nquote\"slash\\雪'";
    let expected_output = format!("custom completed: {RAW}");
    let mut demo = Demo::start().await;
    let mut ws = demo.websocket().await;
    let initial = timeout(Duration::from_secs(5), next_frame(&mut ws))
        .await
        .expect("initial custom raw WebSocket snapshot timeout");
    assert_eq!(initial["type"], "snapshot");

    let command_id = demo.submit(&format!("custom {RAW}")).await;
    let call_id = wait_for_custom_start(&mut ws, RAW.len()).await;
    let started = fresh_snapshot(&mut ws).await;
    let start_event = started["snapshot"]["envelopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| custom_started_call_id(row, RAW.len()).as_deref() == Some(call_id.as_str()))
        .expect("call-scoped custom start envelope missing before reopen");
    let start_id = start_event["id"].clone();
    let start_payload = start_event["payload"].clone();
    assert!(start_id.is_string());
    assert!(start_payload.is_string());
    let _ = wait_for_outcome(&mut ws, &command_id, &["completed"]).await;

    Demo::close_ws(&mut ws).await;
    drop(ws);
    demo.restart().await;
    let mut reopened = demo.websocket().await;
    let restored = timeout(Duration::from_secs(5), next_frame(&mut reopened))
        .await
        .expect("custom raw reopen snapshot timeout");
    assert_eq!(restored["type"], "snapshot");
    let retained_envelopes = restored["snapshot"]["envelopes"].as_array().unwrap();
    assert_eq!(
        retained_envelopes
            .iter()
            .filter(|row| row["id"] == start_id)
            .count(),
        1,
        "reopen duplicated the custom progress envelope"
    );
    let retained_start = retained_envelopes
        .iter()
        .find(|row| row["id"] == start_id)
        .expect("original custom start envelope lost after process reopen");
    assert_eq!(retained_start["payload"], start_payload);
    assert_eq!(
        custom_started_call_id(retained_start, RAW.len()).as_deref(),
        Some(call_id.as_str())
    );
    let jobs = restored["snapshot"]["jobs"].as_array().unwrap();
    assert!(
        jobs.iter().any(|row| {
            row["toolName"] == "echo" && row["state"] == "settled" && row["delivered"] == true
        }),
        "function B was not delivered before the custom result settled"
    );
    assert_eq!(
        jobs.iter().filter(|row| row["callId"] == call_id).count(),
        1,
        "reopen duplicated the original custom job"
    );
    let job = jobs
        .iter()
        .find(|row| row["callId"] == call_id)
        .expect("original custom call missing after process reopen");
    assert_eq!(job["toolKind"], "custom");
    assert_eq!(job["toolName"], "run");
    assert_eq!(job["state"], "settled");
    assert_eq!(job["output"].as_str(), Some(expected_output.as_str()));

    let store = Store::open(&demo.db).expect("reopen durable custom Store");
    let request_id = RequestId(job["requestId"].as_str().unwrap().to_owned());
    let items = store.items(&request_id).expect("retained custom items");
    let calls: Vec<_> = items
        .iter()
        .filter(|item| item.0["type"] == "custom_tool_call" && item.0["call_id"] == call_id)
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "custom call was replayed or lost: {items:#?}"
    );
    assert_eq!(calls[0].0["input"], RAW);
    let outputs = same_branch_custom_outputs(&store, &request_id, &call_id);
    assert_eq!(
        outputs.len(),
        1,
        "custom output was duplicated or lost: {items:#?}"
    );
    assert_eq!(
        outputs[0].0["output"].as_str(),
        Some(expected_output.as_str())
    );
}

#[tokio::test]
async fn browser_journey_custom_cancel_is_retained_after_process_reopen() {
    const RAW: &str = "hold-for-cancel";
    let mut demo = Demo::start().await;
    let mut ws = demo.websocket().await;
    let initial = timeout(Duration::from_secs(5), next_frame(&mut ws))
        .await
        .expect("initial custom cancel WebSocket snapshot timeout");
    assert_eq!(initial["type"], "snapshot");

    let command_id = demo.submit(&format!("custom {RAW}")).await;
    let started_call_id = wait_for_custom_start(&mut ws, RAW.len()).await;
    // An independent command is serviceable only after the server has passed
    // its native A-start and function-B-delivery barriers. It then gives us a
    // bounded, event-driven point to read the production running A row.
    let echo_id = demo.submit("echo browser-cancel-barrier").await;
    let _ = wait_for_outcome(&mut ws, &echo_id, &["completed"]).await;
    let started = fresh_snapshot(&mut ws).await;
    let root = started["snapshot"]["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "conversation/root")
        .expect("root conversation missing while custom call is pending");
    assert_eq!(root["state"], "requesting");
    let running = started["snapshot"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["toolName"] == "run" && row["state"] == "running")
        .expect("custom call has no running production job");
    let call_id = running["callId"]
        .as_str()
        .expect("running custom callId")
        .to_owned();
    assert_eq!(call_id, started_call_id);
    assert_eq!(running["toolKind"], "custom");
    assert_eq!(running["state"], "running");
    let request_id = RequestId(running["requestId"].as_str().unwrap().to_owned());

    let cancel_id = demo.submit("custom cancel").await;
    let _ = wait_for_outcome(&mut ws, &cancel_id, &["completed"]).await;
    let cancelled = fresh_snapshot(&mut ws).await;
    let jobs = cancelled["snapshot"]["jobs"].as_array().unwrap();
    let cancelled_job = jobs
        .iter()
        .find(|row| row["callId"] == call_id)
        .expect("custom job disappeared after cancellation");
    assert_eq!(cancelled_job["toolKind"], "custom");
    assert_eq!(cancelled_job["state"], "cancelled");
    assert!(cancelled_job["output"].is_null());

    Demo::close_ws(&mut ws).await;
    drop(ws);
    demo.restart().await;
    let mut reopened = demo.websocket().await;
    let restored = timeout(Duration::from_secs(5), next_frame(&mut reopened))
        .await
        .expect("cancelled custom reopen snapshot timeout");
    assert_eq!(restored["type"], "snapshot");
    let jobs = restored["snapshot"]["jobs"].as_array().unwrap();
    assert_eq!(
        jobs.iter().filter(|row| row["callId"] == call_id).count(),
        1,
        "reopen reexecuted or lost cancelled custom call"
    );
    let retained = jobs.iter().find(|row| row["callId"] == call_id).unwrap();
    assert_eq!(retained["toolKind"], "custom");
    assert_eq!(retained["state"], "cancelled");
    assert!(retained["output"].is_null());
    let requests = restored["snapshot"]["requests"].as_array().unwrap();
    assert!(
        requests.iter().any(|row| row["commandId"] == command_id),
        "reopen lost original custom command"
    );

    let store = Store::open(&demo.db).expect("reopen cancelled custom Store");
    let items = store
        .items(&request_id)
        .expect("retained cancelled custom items");
    let calls: Vec<_> = items
        .iter()
        .filter(|item| item.0["type"] == "custom_tool_call" && item.0["call_id"] == call_id)
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "cancelled custom call was replayed or lost: {items:#?}"
    );
    assert_eq!(calls[0].0["input"], RAW);
    let outputs = same_branch_custom_outputs(&store, &request_id, &call_id);
    assert!(
        outputs.len() <= 1,
        "reopen duplicated cancelled custom output: {items:#?}"
    );
    for output in outputs {
        let value: Value = serde_json::from_str(
            output.0["output"]
                .as_str()
                .expect("typed cancelled custom output"),
        )
        .expect("cancelled custom output JSON");
        assert_eq!(
            value,
            json!({"error":"job cancelled"}),
            "cancelled custom call retained a non-cancellation output"
        );
    }
}
