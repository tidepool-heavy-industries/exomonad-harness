use std::{collections::HashSet, net::TcpListener, process::Stdio, time::Duration};

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
        let (header, length) = timeout(Duration::from_secs(5), async {
            let mut header = [0; 2];
            stream.read_exact(&mut header).await?;
            let length = match header[1] & 0x7f {
                126 => {
                    let mut size = [0; 2];
                    stream.read_exact(&mut size).await?;
                    u16::from_be_bytes(size) as usize
                }
                127 => {
                    let mut size = [0; 8];
                    stream.read_exact(&mut size).await?;
                    u64::from_be_bytes(size) as usize
                }
                size => size as usize,
            };
            Ok::<_, std::io::Error>((header, length))
        })
        .await
        .expect("WebSocket response timeout")
        .expect("WebSocket frame read");
        let mut payload = vec![0; length];
        timeout(Duration::from_secs(5), stream.read_exact(&mut payload))
            .await
            .expect("WebSocket payload timeout")
            .expect("WebSocket payload read");
        if header[0] & 0x0f == 1 {
            return serde_json::from_slice(&payload).expect("JSON server frame");
        }
    }
}

fn all_records(snapshot: &Value) -> Vec<&Value> {
    ["conversations", "requests", "jobs", "envelopes"]
        .into_iter()
        .flat_map(|key| snapshot["snapshot"][key].as_array().into_iter().flatten())
        .collect()
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
    let initial = next_frame(&mut ws).await;
    assert_eq!(initial["type"], "snapshot");

    // A pending root command must not occupy the receiver: an independent
    // cancel command has to be accepted and produce a durable terminal state.
    let wait_id = Demo::submit_ws(&mut ws, "wait").await;
    let cancel_id = demo.submit("cancel").await;
    assert_ne!(wait_id, cancel_id);

    // Exercise a child identity and its parent-message/reply path.
    let child_id = demo.submit("child browser-child-proof").await;
    assert!(!child_id.is_empty());

    // A controlled failure must remain a distinct durable outcome and not
    // poison the next successful command.
    let failed_id = demo.submit("fail").await;
    let success_id = demo.submit("echo browser-recovery-proof").await;
    assert_ne!(failed_id, success_id);

    // Reconnect is a snapshot resynchronization, not command replay.
    Demo::close_ws(&mut ws).await;
    let mut reconnected = demo.websocket().await;
    let restored = next_frame(&mut reconnected).await;
    assert_eq!(restored["type"], "snapshot");
    let records = all_records(&restored);
    let ids: HashSet<&str> = records.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(
        ids.contains(wait_id.as_str()),
        "wait outcome missing from durable snapshot"
    );
    assert!(
        ids.contains(cancel_id.as_str()),
        "cancel outcome missing from durable snapshot"
    );
    assert!(
        ids.contains(failed_id.as_str()),
        "failure missing from durable snapshot"
    );
    assert!(
        ids.contains(success_id.as_str()),
        "subsequent success missing from durable snapshot"
    );
    assert!(
        records.iter().any(|r| r["state"] == "cancelled"),
        "cancel was not terminal"
    );
    assert!(
        records.iter().any(|r| r["state"] == "failed"),
        "controlled failure not represented"
    );
    assert!(
        records.iter().any(|r| r["state"] == "completed"),
        "recovery success not represented"
    );
}
