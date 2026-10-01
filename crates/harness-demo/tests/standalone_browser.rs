use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt, StreamExt};
use harness::{
    model::RequestId,
    store::{RecordedReplayTurn, Store},
    transport::client::request_body,
};
use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

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

fn binary() -> PathBuf {
    std::env::var_os("HARNESS_DEMO_TEST_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                option_env!("CARGO_BIN_EXE_harness-demo").expect("demo binary is required"),
            )
        })
        .canonicalize()
        .expect("demo binary must exist")
}

fn launcher() -> PathBuf {
    std::env::var_os("HARNESS_LAUNCHER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(option_env!("CARGO_MANIFEST_DIR").expect("manifest path is required"))
                .join("../../scripts/launch-browser-harness")
        })
        .canonicalize()
        .expect("browser launcher must exist")
}

fn web_assets() -> PathBuf {
    std::env::var_os("HARNESS_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(option_env!("CARGO_MANIFEST_DIR").expect("manifest path is required"))
                .join("../../web/dist")
        })
        .canonicalize()
        .expect("web assets must be prepared before this test")
}

fn spawn(
    db: &Path,
    assets: &Path,
    port: u16,
    capture: &Path,
    phase: &str,
    binary_path: &Path,
) -> Child {
    let launcher = launcher();
    Command::new(&launcher)
        .args([
            db.to_str().unwrap(),
            &format!("127.0.0.1:{port}"),
            assets.to_str().unwrap(),
        ])
        .env("HARNESS_DEMO_BIN", binary_path)
        .current_dir(std::env::temp_dir())
        .env("HARNESS_DEMO_SESSION_SECRET", SECRET)
        .env("HARNESS_DEMO_CAPTURE_REQUESTS", capture)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "failed to spawn browser server: phase={phase} launcher={} binary={} database={} capture={} error={error}",
                launcher.display(),
                binary_path.display(),
                db.display(),
                capture.display(),
            )
        })
}

const STDERR_DIAGNOSTIC_LIMIT: usize = 16 * 1024;

async fn readiness_failure(
    child: &mut Child,
    phase: &str,
    binary_path: &Path,
    db: &Path,
    capture: &Path,
    reason: &str,
    exit: Option<std::process::ExitStatus>,
) -> ! {
    let (status, stderr) = if let Some(status) = exit {
        let Some(pipe) = child.stderr.take() else {
            panic!(
                "{reason}: phase={phase} binary={} database={} capture={} exit_status={status} stderr=<stderr pipe unavailable>",
                binary_path.display(),
                db.display(),
                capture.display(),
            );
        };
        let mut bounded = pipe.take((STDERR_DIAGNOSTIC_LIMIT + 1) as u64);
        let mut bytes = Vec::new();
        let read = timeout(Duration::from_secs(1), bounded.read_to_end(&mut bytes)).await;
        let stderr = match read {
            Ok(Ok(_)) => {
                let truncated = bytes.len() > STDERR_DIAGNOSTIC_LIMIT;
                bytes.truncate(STDERR_DIAGNOSTIC_LIMIT);
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&bytes),
                    if truncated {
                        "\n<stderr truncated>"
                    } else {
                        ""
                    }
                )
            }
            Ok(Err(error)) => format!("<stderr read failed: {error}>"),
            Err(_) => "<stderr read timed out after process exit>".to_owned(),
        };
        (status.to_string(), stderr)
    } else {
        (
            "<process still running; no exit status observed>".to_owned(),
            "<not read because try_wait did not observe process exit>".to_owned(),
        )
    };
    let retained = retain_private_failure_evidence(phase, db, capture);
    panic!(
        "{reason}: phase={phase} binary={} database={} capture={} exit_status={status} stderr={stderr} retained={retained}",
        binary_path.display(),
        db.display(),
        capture.display(),
    );
}

/// The deterministic browser fixture contains synthetic conversation content.
/// Opt-in capture preserves the SQLite WAL set and outgoing model requests at
/// a caller-controlled private path before the ephemeral test shell disappears.
fn retain_private_failure_evidence(phase: &str, db: &Path, capture: &Path) -> String {
    let Ok(root) = std::env::var("HARNESS_DIAGNOSTIC_DIR") else {
        return "not requested".into();
    };
    let target = Path::new(&root).join(phase);
    if let Err(error) = std::fs::create_dir_all(&target) {
        return format!("create failed: {error}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) =
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700))
        {
            return format!("private permissions failed: {error}");
        }
    }
    let files = [
        (db.to_path_buf(), "session.sqlite"),
        (db.with_extension("sqlite-wal"), "session.sqlite-wal"),
        (db.with_extension("sqlite-shm"), "session.sqlite-shm"),
        (capture.to_path_buf(), "outgoing-requests.jsonl"),
    ];
    let mut saved = Vec::new();
    for (source, name) in files {
        if source.exists() {
            match std::fs::copy(&source, target.join(name)) {
                Ok(_) => saved.push(name),
                Err(error) => return format!("copy {name} failed: {error}"),
            }
        }
    }
    format!("{} ({})", target.display(), saved.join(","))
}

async fn ready(
    child: &mut Child,
    base: &str,
    client: &reqwest::Client,
    phase: &str,
    binary_path: &Path,
    db: &Path,
    capture: &Path,
) {
    let readiness = timeout(Duration::from_secs(10), async {
        loop {
            if client
                .get(format!("{base}/api/session"))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            let status = child.try_wait().unwrap_or_else(|error| {
                panic!(
                    "failed checking browser server exit: phase={phase} binary={} database={} capture={} error={error}",
                    binary_path.display(),
                    db.display(),
                    capture.display(),
                )
            });
            if let Some(status) = status {
                readiness_failure(
                    child,
                    phase,
                    binary_path,
                    db,
                    capture,
                    "server exited before ready",
                    Some(status),
                )
                .await;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await;
    if readiness.is_err() {
        let status = child.try_wait().unwrap_or_else(|error| {
            panic!(
                "failed checking browser server after readiness timeout: phase={phase} binary={} database={} capture={} error={error}",
                binary_path.display(),
                db.display(),
                capture.display(),
            )
        });
        readiness_failure(
            child,
            phase,
            binary_path,
            db,
            capture,
            "session route readiness timeout after 10 seconds",
            status,
        )
        .await;
    }
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
    while let Some(message) = socket.next().await {
        match message.expect("read websocket frame") {
            Message::Text(text) => {
                return serde_json::from_str(&text).expect("decode websocket JSON text frame");
            }
            Message::Ping(payload) => socket
                .send(Message::Pong(payload))
                .await
                .expect("reply to websocket ping"),
            Message::Pong(_) | Message::Frame(_) => {}
            Message::Binary(_) => panic!("expected websocket JSON text frame, received binary"),
            Message::Close(frame) => panic!("websocket closed before JSON text frame: {frame:?}"),
        }
    }
    panic!("websocket ended before JSON text frame");
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

async fn await_gate_pair(
    base: &str,
    cookie: &str,
    command_id: &str,
    db: &Path,
) -> (Value, Vec<Value>) {
    timeout(Duration::from_secs(10), async {
        loop {
            let shot = snapshot(base, cookie).await;
            let tool_jobs: Vec<Value> = shot["snapshot"]["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|job| {
                    job["callId"].as_str().is_some_and(|id| !id.is_empty())
                        && job["requestId"].as_str().is_some_and(|id| !id.is_empty())
                        && job["toolName"].as_str().is_some_and(|name| !name.is_empty())
                        && job["delivered"].is_boolean()
                })
                .cloned()
                .collect();
            let request_sequence = before_request_decisions(db);
            let pair_observed = tool_jobs
                .iter()
                .filter(|job| {
                    job["toolName"] == "gate"
                        && job["state"] == "running"
                        && job["delivered"] == false
                })
                .any(|a| {
                    let Some(a_sequence) = request_sequence
                        .iter()
                        .position(|row| row["request"] == a["requestId"])
                    else {
                        return false;
                    };
                    tool_jobs.iter().any(|b| {
                        if b["delivered"] != true
                            || b["callId"] == a["callId"]
                            || b["requestId"] == a["requestId"]
                        {
                            return false;
                        }
                        request_sequence
                            .iter()
                            .position(|row| row["request"] == b["requestId"])
                            .is_some_and(|b_sequence| b_sequence > a_sequence)
                    })
                });
            if pair_observed {
                return (shot, tool_jobs);
            }

            // A terminal command without the paired gate jobs is direct
            // expected-red evidence for a missing async producer.
            let command_finished = shot["snapshot"]["requests"]
                .as_array()
                .unwrap()
                .iter()
                .any(|request| request["commandId"] == command_id && request["state"] != "running");
            if command_finished {
                return (shot, tool_jobs);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("async start exposed neither the pending/delivered ToolJobRecord pair nor a terminal command")
}

async fn await_job_state(base: &str, cookie: &str, call_id: &str, state: &str) -> Value {
    timeout(Duration::from_secs(10), async {
        loop {
            let shot = snapshot(base, cookie).await;
            if let Some(job) = shot["snapshot"]["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|job| job["callId"] == call_id && job["state"] == state)
            {
                return job.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("ToolJobRecord did not reach requested state")
}

fn output_count(items: &[Value], call_id: &str) -> usize {
    items
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .count()
}

fn stored_items(db: &Path, request_id: &str) -> Vec<Value> {
    Store::open(db)
        .expect("open Engine Store")
        .items(&RequestId(request_id.to_owned()))
        .expect("read Engine request history")
        .into_iter()
        .map(|item| item.0)
        .collect()
}

fn recorded_model_turns(db: &Path) -> Vec<RecordedReplayTurn> {
    let store = Store::open(db).expect("open Engine Store");
    let mut seen = std::collections::HashSet::new();
    let mut recorded = Vec::new();
    for event in store.events(None).expect("read immutable Store events") {
        if event.kind != "model_turn" {
            continue;
        }
        let request = event.request.expect("model turn has an issuing request");
        if seen.contains(&request) {
            continue;
        }
        let turns: Vec<_> = store
            .replay_turns(&request)
            .expect("read exact issued replay windows")
            .into_iter()
            .filter(|turn| !seen.contains(&turn.request))
            .collect();
        seen.extend(turns.iter().map(|turn| turn.request.clone()));
        recorded.extend(turns);
    }
    recorded
}

fn recorded_turn_for(db: &Path, request_id: &str) -> RecordedReplayTurn {
    let matches: Vec<_> = recorded_model_turns(db)
        .into_iter()
        .filter(|turn| turn.request.0 == request_id)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "request must have exactly one immutable model_turn event: {request_id}"
    );
    matches.into_iter().next().unwrap()
}

fn captured_request_for(capture: &Path, db: &Path, request_id: &str) -> Value {
    let turn = recorded_turn_for(db, request_id);
    let expected_body = request_body(&turn.model_request).expect("serialize recorded request body");
    let captures: Vec<Value> = std::fs::read_to_string(capture)
        .expect("read transport captures")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("capture JSON"))
        .filter(|body| *body == expected_body)
        .collect();
    assert_eq!(
        captures.len(),
        1,
        "immutable model_request must identify exactly one production request_body capture for request {request_id}"
    );
    captures.into_iter().next().unwrap()
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
    let db = root.join("private/session.sqlite");
    let capture = root.join("outgoing-requests.jsonl");
    let missing = root.join("not-built");
    let missing_port = port();
    let missing_binary = binary();
    let absent = spawn(
        &db,
        &missing,
        missing_port,
        &capture,
        "missing-assets",
        &missing_binary,
    );
    let output = timeout(Duration::from_secs(5), absent.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !output.status.success(),
        "phase=missing-assets binary={} database={} capture={} exit_status={} stderr={}",
        missing_binary.display(),
        db.display(),
        capture.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
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

    let assets = web_assets();
    // A failed launch must leave the occupied listener with its original owner.
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let occupied_db = root.join("occupied/session.sqlite");
    let occupied_binary = binary();
    let refused = spawn(
        &occupied_db,
        &assets,
        occupied_port,
        &capture,
        "occupied-port",
        &occupied_binary,
    );
    let refused = timeout(Duration::from_secs(5), refused.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !refused.status.success(),
        "phase=occupied-port binary={} database={} capture={} exit_status={} stderr={}",
        occupied_binary.display(),
        occupied_db.display(),
        capture.display(),
        refused.status,
        String::from_utf8_lossy(&refused.stderr),
    );
    assert!(TcpListener::bind(("127.0.0.1", occupied_port)).is_err());
    drop(occupied);
    assert!(TcpListener::bind(("127.0.0.1", occupied_port)).is_ok());

    let selected_port = port();
    let base = format!("http://127.0.0.1:{selected_port}");
    let client = reqwest::Client::new();
    let first_binary = binary();
    let mut first = spawn(
        &db,
        &assets,
        selected_port,
        &capture,
        "first",
        &first_binary,
    );
    ready(
        &mut first,
        &base,
        &client,
        "first",
        &first_binary,
        &db,
        &capture,
    )
    .await;
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
    let early_decisions = before_request_decisions(&db);
    let tagged: Vec<_> = early_decisions
        .iter()
        .filter(|row| row["evidence"]["consumer"] == "standalone-browser")
        .collect();
    assert!(
        !tagged.is_empty(),
        "expected before-request evidence for browser Engine requests; decisions: {early_decisions:#?}"
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

    // `async start` alone drives the controlled two-request scenario. Do not
    // submit another command to make B; inspect the records emitted by start.
    // B is automatically emitted by async start's same Engine::run scenario on
    // its next transport/model turn while A remains pending.
    let async_start_id = submit(&base, &cookie, &client, "async start").await;
    let (async_start_snapshot, tool_jobs) =
        await_gate_pair(&base, &cookie, &async_start_id, &db).await;

    let pending_a: Vec<&Value> = tool_jobs
        .iter()
        .filter(|job| {
            job["toolName"] == "gate" && job["state"] == "running" && job["delivered"] == false
        })
        .collect();
    let a_request_sequence = before_request_decisions(&db);
    let a_sequence = pending_a
        .first()
        .and_then(|job| job["requestId"].as_str())
        .and_then(|request_id| {
            a_request_sequence
                .iter()
                .position(|row| row["request"] == request_id)
        });
    let delivered_b_candidates: Vec<(&Value, usize)> = tool_jobs
        .iter()
        .filter(|job| {
            if job["delivered"] != true {
                return false;
            }
            let Some(a) = pending_a.first() else {
                return false;
            };
            if job["callId"] == a["callId"] || job["requestId"] == a["requestId"] {
                return false;
            }
            let Some(a_sequence) = a_sequence else {
                return false;
            };
            a_request_sequence
                .iter()
                .position(|row| row["request"] == job["requestId"])
                .is_some_and(|b_sequence| b_sequence > a_sequence)
        })
        .filter_map(|job| {
            a_request_sequence
                .iter()
                .position(|row| row["request"] == job["requestId"])
                .map(|sequence| (job, sequence))
        })
        .collect();
    assert_eq!(
        pending_a.len(),
        1,
        "expected exactly one A running and undelivered after accepted async start; command={} requests={:#?} jobs={:#?}",
        async_start_id,
        async_start_snapshot["snapshot"]["requests"],
        async_start_snapshot["snapshot"]["jobs"]
    );
    let delivered_b = delivered_b_candidates
        .iter()
        .min_by_key(|(_, sequence)| sequence)
        .map(|(job, _)| *job)
        .expect("expected a distinct delivered B from a later actual Engine request while A remains pending");
    let gate_a = pending_a[0];
    let gate_b = delivered_b;
    let gate_call_id = gate_a["callId"]
        .as_str()
        .expect("A ToolJobRecord must expose its original callId");
    let gate_request_id = gate_a["requestId"]
        .as_str()
        .expect("A ToolJobRecord must expose its emitting Engine requestId");
    let b_call_id = gate_b["callId"]
        .as_str()
        .expect("B ToolJobRecord must expose its original callId");
    let b_request_id = gate_b["requestId"]
        .as_str()
        .expect("B ToolJobRecord must expose its emitting Engine requestId");
    assert_ne!(gate_call_id, b_call_id, "A and B must be distinct calls");
    assert_ne!(
        gate_request_id, b_request_id,
        "A and B must come from distinct Engine requests"
    );
    let b_sequence = a_request_sequence
        .iter()
        .position(|row| row["request"] == b_request_id)
        .expect("B requestId must occur in actual Store before-request history");
    assert!(
        Some(b_sequence) > a_sequence,
        "B must be emitted by the subsequent Engine request; sequence A={a_sequence:?}, B={b_sequence}"
    );

    let echo_id = submit(&base, &cookie, &client, "echo hello").await;
    let echo_while_pending = await_request(&base, &cookie, &echo_id).await;
    let echo_record = echo_while_pending["snapshot"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["commandId"] == echo_id)
        .expect("echo hello request must remain visible while async work is pending");
    assert_eq!(echo_record["state"], "completed");
    assert_eq!(echo_record["outcome"], "completed");
    assert_eq!(echo_record["detail"], "hello");

    let engine_store = Store::open(&db).expect("open Store for async Engine evidence");
    let gate_request = RequestId(gate_request_id.to_owned());
    assert!(
        engine_store
            .request(&gate_request)
            .expect("read emitting Engine request")
            .is_some(),
        "ToolJobRecord requestId must identify a persisted Engine request"
    );
    let gate_items = engine_store
        .items(&gate_request)
        .expect("read emitting Engine request history");
    assert!(
        gate_items.iter().any(|item| {
            item.0["type"] == "function_call"
                && item.0["name"] == "gate"
                && item.0["call_id"] == gate_call_id
                && item.0["async"] == true
        }),
        "Store Engine history must contain the original native-async gate call: {gate_items:#?}"
    );
    let b_emitting_items = stored_items(&db, b_request_id);
    assert!(
        b_emitting_items.iter().any(|item| {
            item["type"] == "function_call"
                && item["call_id"] == b_call_id
                && item["name"] == gate_b["toolName"]
                && item["async"] == true
        }),
        "B's observed call must retain native async metadata in its actual Engine request: {b_emitting_items:#?}"
    );
    let initial_capture = std::fs::read_to_string(&capture)
        .expect("read actual async request captures")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("captured request JSON"))
        .find(|request| {
            request["input"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["content"] == "async start"))
        })
        .expect("actual async start transport request");
    for name in ["gate", "echo"] {
        assert!(
            initial_capture["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name && tool["async"] == true),
            "actual request must advertise {name} with async:true: {initial_capture:#?}"
        );
    }
    assert!(
        engine_store
            .pending_at(&gate_request)
            .expect("read pending Store claims")
            .iter()
            .any(|pending| pending.call_id.0 == gate_call_id),
        "Store must retain the original gate call as pending"
    );
    let b_turns = recorded_model_turns(&db);
    let b_consumers: Vec<_> = b_turns
        .iter()
        .filter(|turn| {
            let input: Vec<_> = turn
                .model_request
                .input
                .iter()
                .map(|item| item.0.clone())
                .collect();
            output_count(&input, b_call_id) == 1
        })
        .collect();
    assert_eq!(
        b_consumers.len(),
        1,
        "B output must first be consumed exactly once in an immutable as-sent model request: {b_turns:#?}"
    );
    let b_consumer = b_consumers[0];
    let b_input: Vec<_> = b_consumer
        .model_request
        .input
        .iter()
        .map(|item| item.0.clone())
        .collect();
    assert!(
        b_input
            .iter()
            .any(|item| { item["type"] == "function_call" && item["call_id"] == gate_call_id }),
        "the as-sent B consumer must retain original A call provenance: {b_input:#?}"
    );
    assert_eq!(
        output_count(&b_input, gate_call_id),
        0,
        "A must remain unanswered in the earliest B-consuming as-sent request"
    );
    let b_capture = captured_request_for(&capture, &db, &b_consumer.request.0);
    assert_eq!(
        output_count(b_capture["input"].as_array().unwrap(), b_call_id),
        1
    );
    let pending_snapshot = snapshot(&base, &cookie).await;
    assert!(
        pending_snapshot["snapshot"]["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|job| job["callId"] == gate_call_id
                && job["state"] == "running"
                && job["delivered"] == false),
        "A must remain unanswered as B is consumed"
    );

    let release_id = submit(&base, &cookie, &client, "async release").await;
    let _ = await_request(&base, &cookie, &release_id).await;
    let _ = await_request(&base, &cookie, &async_start_id).await;
    let released_a = await_job_state(&base, &cookie, gate_call_id, "settled").await;
    assert_eq!(released_a["requestId"], gate_request_id);
    assert_eq!(released_a["output"], "A released");
    assert_eq!(
        released_a["delivered"], true,
        "released A output must be consumed"
    );
    let a_history = before_request_decisions(&db);
    let a_consumers: Vec<_> = a_history
        .iter()
        .filter(|row| output_count(row["items"].as_array().unwrap(), gate_call_id) == 1)
        .collect();
    assert_eq!(
        a_consumers.len(),
        1,
        "released A output must occur once: {a_history:#?}"
    );
    let cancel_start_id = submit(&base, &cookie, &client, "async start").await;
    let (_, cancel_jobs) = await_gate_pair(&base, &cookie, &cancel_start_id, &db).await;
    let cancel_a = cancel_jobs
        .iter()
        .find(|job| {
            job["toolName"] == "gate" && job["state"] == "running" && job["delivered"] == false
        })
        .expect("fresh scenario A");
    let cancel_call = cancel_a["callId"].as_str().unwrap().to_owned();
    let cancel_request = cancel_a["requestId"].as_str().unwrap().to_owned();
    let cancel_id = submit(&base, &cookie, &client, "async cancel").await;
    let _ = await_request(&base, &cookie, &cancel_id).await;
    let cancelled_a = await_job_state(&base, &cookie, &cancel_call, "cancelled").await;
    assert_eq!(cancelled_a["requestId"], cancel_request);
    let _ = await_request(&base, &cookie, &cancel_start_id).await;
    let late_release_id = submit(&base, &cookie, &client, "async release").await;
    let _ = await_request(&base, &cookie, &late_release_id).await;
    let post_release = snapshot(&base, &cookie).await;
    let final_cancelled = post_release["snapshot"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["callId"] == cancel_call)
        .expect("cancelled A remains recorded");
    assert_eq!(final_cancelled["state"], "cancelled");
    assert_eq!(final_cancelled["requestId"], cancel_request);
    assert_eq!(
        final_cancelled["output"],
        Value::Null,
        "late release must not resurrect A"
    );
    let cancel_history = before_request_decisions(&db);
    let cancel_consumers: Vec<_> = cancel_history
        .iter()
        .filter(|row| output_count(row["items"].as_array().unwrap(), &cancel_call) == 1)
        .collect();
    assert_eq!(
        cancel_consumers.len(),
        1,
        "cancelled A must not gain replacement outputs: {cancel_history:#?}"
    );
    let cancel_items = stored_items(&db, cancel_consumers[0]["request"].as_str().unwrap());
    assert_eq!(output_count(&cancel_items, &cancel_call), 1);
    stop_sigint(&mut first).await;

    let decisions_before_reopen = before_request_decisions(&db);
    let captured_before_reopen = std::fs::read_to_string(&capture).unwrap();
    let second_binary = binary();
    let mut second = spawn(
        &db,
        &assets,
        selected_port,
        &capture,
        "clean-reopen",
        &second_binary,
    );
    ready(
        &mut second,
        &base,
        &client,
        "clean-reopen",
        &second_binary,
        &db,
        &capture,
    )
    .await;
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
    let recovery_start_id = submit(&base, &cookie, &client, "async start").await;
    let (_, recovery_jobs) = await_gate_pair(&base, &cookie, &recovery_start_id, &db).await;
    let recovery_a = recovery_jobs
        .iter()
        .find(|job| {
            job["toolName"] == "gate" && job["state"] == "running" && job["delivered"] == false
        })
        .expect("in-flight A before process loss");
    let recovery_call = recovery_a["callId"].as_str().unwrap().to_owned();
    let recovery_request = recovery_a["requestId"].as_str().unwrap().to_owned();
    second.kill().await.unwrap();
    assert!(
        !second.wait().await.unwrap().success(),
        "process-loss phase was not a kill"
    );

    let third_binary = binary();
    let mut third = spawn(
        &db,
        &assets,
        selected_port,
        &capture,
        "process-loss-reopen",
        &third_binary,
    );
    ready(
        &mut third,
        &base,
        &client,
        "process-loss-reopen",
        &third_binary,
        &db,
        &capture,
    )
    .await;
    let cookie = login(&base, &client).await;
    let lost = snapshot(&base, &cookie).await;
    let interrupted_a = lost["snapshot"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["callId"] == recovery_call)
        .expect("reopened A ToolJobRecord");
    assert_eq!(interrupted_a["state"], "interrupted");
    assert_eq!(interrupted_a["requestId"], recovery_request);
    let recovery_store = Store::open(&db).expect("open recovered Store");
    let recovered_claim = recovery_store
        .claims(&harness::model::CallId(recovery_call.clone()))
        .expect("read recovered claim")
        .into_iter()
        .find(|claim| claim.request.0 == recovery_request)
        .expect("claim retains A request identity");
    assert_eq!(
        recovered_claim.state,
        harness::store::ClaimState::Interrupted
    );
    let recovery_rows = before_request_decisions(&db);
    let recovery_outputs: Vec<_> = recovery_rows
        .iter()
        .filter(|row| output_count(row["items"].as_array().unwrap(), &recovery_call) == 1)
        .collect();
    assert_eq!(
        recovery_outputs.len(),
        1,
        "recovery output must be unique in Engine history: {recovery_rows:#?}"
    );
    let recovery_items = stored_items(&db, recovery_outputs[0]["request"].as_str().unwrap());
    assert_eq!(output_count(&recovery_items, &recovery_call), 1);
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
