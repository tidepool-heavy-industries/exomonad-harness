use std::{net::TcpListener, process::Stdio, time::Duration};

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
    client: reqwest::Client,
    cookie: String,
}

impl Demo {
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_harness-demo"))
            .args([
                "--db",
                db.to_str().unwrap(),
                "--serve",
                &address.to_string(),
            ])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("HARNESS_DEMO_SESSION_SECRET", SESSION_SECRET)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start production --serve binary");
        let client = reqwest::Client::new();
        let login_url = format!("{base}/api/session");
        let mut ready = false;
        for _ in 0..100 {
            if let Ok(response) = client.get(&login_url).send().await {
                if response.status().is_success() {
                    ready = true;
                    break;
                }
            }
            if child.try_wait().expect("check server process").is_some() {
                panic!("production server exited before becoming ready");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ready, "production server did not become ready");
        let login = client
            .post(&login_url)
            .header("origin", &base)
            .json(&json!({"secret": SESSION_SECRET}))
            .send()
            .await
            .unwrap();
        assert!(login.status().is_success(), "browser-session login failed");
        let cookie = login
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        Self {
            child,
            base,
            client,
            cookie,
        }
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
    timeout(Duration::from_secs(10), async {
        loop {
            let frame = next_frame(socket).await;
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
    .expect("command request did not reach required outcome before bounded observation deadline")
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
