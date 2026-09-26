use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use harness::{model::RequestId, store::Store};
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

fn spawn(db: &Path, assets: &Path, port: u16, capture: &Path) -> Child {
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
        .env("HARNESS_DEMO_CAPTURE_REQUESTS", capture)
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

fn before_request_decisions(db: &Path) -> Vec<Value> {
    let store = Store::open(db).expect("open isolated Engine Store");
    store
        .decisions(None)
        .expect("read Store decisions")
        .into_iter()
        .filter(|row| row.decision.hook == "before-request")
        .map(|row| {
            let request = row
                .request
                .as_ref()
                .expect("before-request decision must reference a request");
            let stored_request = store
                .request(request)
                .expect("read decision request")
                .expect("decision request must exist");
            let items = store
                .items(&RequestId(request.0.clone()))
                .expect("read decision request items");
            json!({
                "id": row.id,
                "request": request.0,
                "agent": stored_request.branch,
                "items": items,
                "decision": row.decision.decision,
                "evidence": row.decision.evidence,
            })
        })
        .collect()
}

#[tokio::test]
async fn standalone_missing_assets_and_clean_and_process_loss_reopen() {
    let root = temp_path();
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("session.sqlite");
    let capture = root.join("outgoing-requests.jsonl");
    let missing = root.join("not-built");
    let missing_port = port();
    let absent = spawn(&db, &missing, missing_port, &capture);
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
    let mut first = spawn(&db, &assets, selected_port, &capture);
    ready(&mut first, &base, &client).await;
    let cookie = login(&base, &client).await;
    let first_id = submit(&base, &cookie, &client, "echo standalone").await;
    let echo = await_request(&base, &cookie, &first_id).await;
    let echo_record = echo["snapshot"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["commandId"] == first_id)
        .expect("echo request must be visible");
    assert_eq!(echo_record["state"], "completed");
    assert_eq!(echo_record["outcome"], "completed");
    assert_eq!(echo_record["detail"], "standalone");
    let inject_id = submit(&base, &cookie, &client, "echo inject-context").await;
    let injected = await_request(&base, &cookie, &inject_id).await;
    let inject_record = injected["snapshot"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["commandId"] == inject_id)
        .expect("inject-context request must be visible");
    assert_eq!(inject_record["state"], "completed");
    assert_eq!(inject_record["outcome"], "completed");
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
    let decisions_before_reopen = before_request_decisions(&db);
    let tagged: Vec<_> = decisions_before_reopen
        .iter()
        .filter(|row| row["evidence"]["consumer"] == "standalone-browser")
        .collect();
    assert!(
        !tagged.is_empty(),
        "expected before-request evidence for browser Engine requests; decisions: {decisions_before_reopen:#?}"
    );
    assert!(
        tagged.iter().all(|row| {
            row["request"]
                .as_str()
                .is_some_and(|request| !request.is_empty())
                && row["agent"].as_str().is_some_and(|agent| !agent.is_empty())
        }),
        "hook decisions must retain request and agent provenance: {tagged:#?}"
    );
    assert!(
        tagged.iter().any(|row| {
            row["agent"] == "/root"
                && serde_json::to_string(&row["items"])
                    .is_ok_and(|items| items.contains("echo standalone"))
                && row["decision"] == json!({"SendRestricted":{"tools_allowed":["sleep"]}})
                && row["evidence"]["selection"] == "echo-sleep"
                && row["evidence"]["sleep_advertised"] == true
        }),
        "expected echo-sleep decision, advertised sleep, Engine request and /root agent: {tagged:#?}"
    );
    let injected_decisions: Vec<_> = tagged
        .iter()
        .filter(|row| {
            serde_json::to_string(&row["items"])
                .is_ok_and(|items| items.contains("echo inject-context"))
        })
        .collect();
    assert_eq!(
        injected_decisions.len(),
        1,
        "inject-context should produce exactly one hook decision: {tagged:#?}"
    );
    assert_eq!(injected_decisions[0]["agent"], "/root");
    assert_eq!(
        injected_decisions[0]["decision"],
        json!({"Inject":{
            "item":{"type":"message","role":"user","content":"standalone injected context"},
            "tools_allowed":["sleep"]
        }})
    );
    assert_eq!(
        injected_decisions[0]["evidence"]["selection"],
        "echo-inject-sleep"
    );
    assert_eq!(
        injected_decisions[0]["evidence"]["consumer"],
        "standalone-browser"
    );
    assert!(
        tagged.iter().any(|row| {
            row["agent"] != "/root"
                && row["decision"] == json!({"SendRestricted":{"tools_allowed":[]}})
                && row["evidence"]["selection"] == "child-empty"
        }),
        "expected child-empty decision (its path is intentionally not fixed): {tagged:#?}"
    );
    let captured: Vec<Value> = std::fs::read_to_string(&capture)
        .expect("deterministic transport must capture outgoing requests")
        .lines()
        .map(|line| serde_json::from_str(line).expect("captured request must be JSON"))
        .collect();
    let echo_request = captured
        .iter()
        .find(|request| {
            serde_json::to_string(&request["input"])
                .is_ok_and(|input| input.contains("echo standalone"))
        })
        .expect("captured echo Engine request");
    let inject_request = captured
        .iter()
        .find(|request| {
            serde_json::to_string(&request["input"])
                .is_ok_and(|input| input.contains("echo inject-context"))
        })
        .expect("captured echo inject-context Engine request");
    let child_request = captured
        .iter()
        .find(|request| {
            serde_json::to_string(&request["input"])
                .is_ok_and(|input| input.contains("child standalone"))
        })
        .expect("captured child Engine request");
    assert_eq!(echo_request["tools"], child_request["tools"]);
    assert_eq!(inject_request["tools"], echo_request["tools"]);
    let injected_input = inject_request["input"].as_array().unwrap();
    assert_eq!(
        injected_input.last().unwrap(),
        &json!({"type":"message","role":"user","content":"standalone injected context"})
    );
    assert_eq!(
        injected_input
            .iter()
            .filter(|item| item["content"] == "standalone injected context")
            .count(),
        1
    );
    assert!(
        serde_json::to_string(&inject_request["input"])
            .unwrap()
            .contains("echo inject-context")
    );
    assert_eq!(
        inject_request["tool_choice"],
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"sleep"}]})
    );
    let tool_names: Vec<&str> = echo_request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(
        tool_names.contains(&"sleep"),
        "full definitions must include sleep"
    );
    assert_eq!(
        echo_request["tool_choice"],
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"sleep"}]})
    );
    assert_eq!(child_request["tool_choice"], "none");
    stop_sigint(&mut first).await;

    let captured_before_reopen = std::fs::read_to_string(&capture).unwrap();
    let mut second = spawn(&db, &assets, selected_port, &capture);
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
            .any(|r| r["commandId"] == inject_id)
    );
    assert!(
        clean["snapshot"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["commandId"] == child_id)
    );
    assert_eq!(
        before_request_decisions(&db),
        decisions_before_reopen,
        "opening and reading the stored session must not create hook decisions"
    );
    assert_eq!(
        std::fs::read_to_string(&capture).unwrap(),
        captured_before_reopen,
        "reopening the stored session must not issue/capture another Engine request"
    );
    second.kill().await.unwrap();
    assert!(
        !second.wait().await.unwrap().success(),
        "process-loss phase was not a kill"
    );

    let mut third = spawn(&db, &assets, selected_port, &capture);
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
        lost["snapshot"]["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["commandId"] == inject_id)
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
