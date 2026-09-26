use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

const SECRET: &str = "isolated-standalone-test-secret-32-bytes";
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn port() -> u16 {
    loop {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let chosen = listener.local_addr().unwrap().port();
        if chosen != 4600 {
            return chosen;
        }
    }
}

fn temp_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("harness-standalone-{}-{nonce}", std::process::id()))
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_harness-demo")
}

fn spawn(db: &Path, assets: &Path, port: u16) -> Child {
    Command::new(binary())
        .args([
            "--db",
            db.to_str().unwrap(),
            "--serve",
            &format!("127.0.0.1:{port}"),
            "--assets",
            assets.to_str().unwrap(),
        ])
        .current_dir(std::env::temp_dir())
        .env("HARNESS_DEMO_SESSION_SECRET", SECRET)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn ready(child: &mut Child, base: &str, client: &reqwest::Client) {
    timeout(Duration::from_secs(10), async {
        loop {
            if client
                .get(format!("{base}/api/session"))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "server exited before ready"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("session route readiness timeout");
}

async fn login(base: &str, client: &reqwest::Client) -> String {
    let response = client
        .post(format!("{base}/api/session"))
        .header("origin", base)
        .json(&json!({"secret":SECRET}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn snapshot(base: &str, cookie: &str) -> Value {
    let mut request = (base.replacen("http://", "ws://", 1) + "/api/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", base.parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let value = timeout(Duration::from_secs(5), read_json(&mut socket))
        .await
        .unwrap();
    assert_eq!(value["type"], "snapshot");
    value
}

async fn read_json(socket: &mut Socket) -> Value {
    let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_mut() else {
        panic!("unexpected TLS on loopback");
    };
    loop {
        let mut header = [0; 2];
        stream.read_exact(&mut header).await.unwrap();
        let len = match header[1] & 0x7f {
            126 => {
                let mut b = [0; 2];
                stream.read_exact(&mut b).await.unwrap();
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0; 8];
                stream.read_exact(&mut b).await.unwrap();
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut body = vec![0; len];
        stream.read_exact(&mut body).await.unwrap();
        if header[0] & 0x0f == 1 {
            return serde_json::from_slice(&body).unwrap();
        }
    }
}

async fn submit(base: &str, cookie: &str, client: &reqwest::Client, command: &str) -> String {
    let response = client
        .post(format!("{base}/api/commands"))
        .header("origin", base)
        .header("cookie", cookie)
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

async fn await_request(base: &str, cookie: &str, id: &str) -> Value {
    timeout(Duration::from_secs(10), async {
        loop {
            let shot = snapshot(base, cookie).await;
            if shot["snapshot"]["requests"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["commandId"] == id && r["state"] != "running")
            {
                return shot;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("command did not finish")
}

async fn stop_sigint(child: &mut Child) {
    let pid = child.id().unwrap();
    let status = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .await
        .unwrap();
    assert!(status.success(), "could not signal exact test process");
    let exit = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("graceful shutdown timeout")
        .unwrap();
    assert!(exit.success(), "graceful shutdown failed: {exit}");
}

#[tokio::test]
async fn standalone_missing_assets_and_clean_and_process_loss_reopen() {
    let root = temp_path();
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("session.sqlite");
    let missing = root.join("not-built");
    let missing_port = port();
    let mut absent = spawn(&db, &missing, missing_port);
    let output = timeout(Duration::from_secs(5), absent.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains(missing.to_str().unwrap()),
        "missing path absent from error: {error}"
    );
    assert!(
        TcpListener::bind(("127.0.0.1", missing_port)).is_ok(),
        "missing-assets server still listening"
    );
    assert!(!db.exists(), "missing assets unexpectedly touched database");

    let assets = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/dist")
        .canonicalize()
        .expect("web/dist must be prepared before this test");
    let selected_port = port();
    let base = format!("http://127.0.0.1:{selected_port}");
    let client = reqwest::Client::new();
    let mut first = spawn(&db, &assets, selected_port);
    ready(&mut first, &base, &client).await;
    let cookie = login(&base, &client).await;
    let first_id = submit(&base, &cookie, &client, "echo standalone").await;
    let _ = await_request(&base, &cookie, &first_id).await;
    let child_id = submit(&base, &cookie, &client, "child standalone-child").await;
    let first_shot = await_request(&base, &cookie, &child_id).await;
    assert!(
        first_shot["snapshot"]["envelopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "MESSAGE"),
        "child message absent before reopen"
    );
    stop_sigint(&mut first).await;

    let mut second = spawn(&db, &assets, selected_port);
    ready(&mut second, &base, &client).await;
    let cookie = login(&base, &client).await;
    let clean = snapshot(&base, &cookie).await;
    assert!(
        clean["snapshot"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["commandId"] == first_id)
    );
    assert!(
        clean["snapshot"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["commandId"] == child_id)
    );
    second.kill().await.unwrap();
    assert!(
        !second.wait().await.unwrap().success(),
        "process-loss phase was not a kill"
    );

    let mut third = spawn(&db, &assets, selected_port);
    ready(&mut third, &base, &client).await;
    let cookie = login(&base, &client).await;
    let lost = snapshot(&base, &cookie).await;
    assert!(
        lost["snapshot"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["commandId"] == first_id)
    );
    assert!(
        lost["snapshot"]["envelopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "MESSAGE")
    );
    stop_sigint(&mut third).await;
    let _ = std::fs::remove_dir_all(root);
}
