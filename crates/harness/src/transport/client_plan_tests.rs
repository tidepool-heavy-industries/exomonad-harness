use super::*;
use crate::transport::ResponsesClient;
use crate::{item::Item, model::Effort};
use serde_json::json;

fn plan_request() -> ResponsesRequest {
    ResponsesRequest {
        input: vec![
            Item::configuration_update(Effort::Low),
            Item(json!({"type":"message","role":"user","content":"Start work"})),
            Item(json!({"type":"custom_tool_call","call_id":"pending","name":"cell","namespace":"functions","async":true,"input":"work"})),
            Item::configuration_update(Effort::High),
            Item(json!({"type":"function_call","call_id":"settled","name":"yield","arguments":"{}"})),
            Item(json!({"type":"function_call_output","call_id":"settled","output":"ready"})),
        ],
        instructions: "Persistent shared instructions".into(),
        tools: vec![
            json!({"type":"custom","name":"cell","description":"Execute a Haskell cell","async":true}),
            json!({"type":"function","name":"yield","description":"Wait for work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ].into(),
        tools_allowed: None,
        model: "gpt-6.1-sol".into(),
        pinned_effort: Effort::Low,
        session_id: "plan-production-request".into(),
        prompt_cache_comparison: None,
    }
}

#[test]
fn public_production_request_preserves_async_and_full_history() {
    let request = plan_request();
    let original = serde_json::to_value(&request).unwrap();
    let normalized = normalized_request(&request).unwrap();
    let wire = http_request(
        &reqwest::Client::new(),
        &request,
        serde_json::to_vec(&normalized).unwrap(),
        "test-bearer",
    )
    .build()
    .unwrap();
    assert_eq!(wire.url().as_str(), CHATGPT_PLAN_ENDPOINT);
    assert_eq!(
        wire.headers()[reqwest::header::AUTHORIZATION],
        "Bearer test-bearer"
    );
    for header in [
        "chatgpt-account-id",
        "version",
        "originator",
        "x-openai-internal-codex-responses-lite",
    ] {
        assert!(!wire.headers().contains_key(header));
    }
    for header in ["session-id", "thread-id", "x-client-request-id"] {
        assert_eq!(wire.headers()[header], request.session_id.thread());
    }
    assert_eq!(
        wire.headers()[reqwest::header::USER_AGENT],
        "exomonad-harness/0.1.0"
    );
    assert_eq!(wire.headers()[reqwest::header::CONTENT_ENCODING], "zstd");
    let decoded = zstd::stream::decode_all(wire.body().unwrap().as_bytes().unwrap()).unwrap();
    let body: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body["reasoning"].get("context").is_none());
    assert_eq!(body["instructions"], request.instructions);
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["name"], "functions");
    assert_eq!(body["tools"][0]["tools"][0]["async"], true);
    assert!(body["tools"][0]["tools"][1].get("async").is_none());
    let expected: Vec<_> = request
        .input
        .iter()
        .filter(|item| !item.is_configuration_update())
        .cloned()
        .collect();
    assert_eq!(body["input"], serde_json::to_value(expected).unwrap());
    assert_eq!(body["input"][1]["call_id"], "pending");
    assert_eq!(body["input"][1]["async"], true);
    for field in [
        "previous_response_id",
        "background",
        "max_output_tokens",
        "conversation",
        "metadata",
    ] {
        assert!(body.get(field).is_none());
    }
    assert_eq!(serde_json::to_value(&request).unwrap(), original);
}

#[test]
fn provider_http_body_omits_durable_output_format_without_changing_retained_results() {
    use crate::{
        item::ToolKind,
        model::{CallId, RequestId},
        provider::{OutputFormat, ToolFailure},
        store::Store,
        turn::JobOutput,
    };
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Directory(std::env::temp_dir().join(format!(
        "harness-provider-output-format-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir(&directory.0).unwrap();
    let path = directory.0.join("store.sqlite");
    let store = Store::open(&path).unwrap();
    let receipt = json!({"nativeResult":{"version":1,"output_format":"user payload λ"}});
    let terminals = [
        JobOutput::Completed(Ok(receipt.clone())),
        JobOutput::Completed(Err(
            ToolFailure::from("failed").with_result_receipt(receipt.clone())
        )),
        JobOutput::CancelledWithReceipt(Ok(receipt.clone())),
        JobOutput::CancellationUnconfirmed(
            ToolFailure::from("unconfirmed").with_result_receipt(receipt.clone()),
        ),
    ];
    let mut records = Vec::new();
    let mut request = plan_request();
    request.input = vec![
        Item::user_text("Keep output_format inside ordinary text"),
        Item(json!({"type":"reasoning","summary":[],"encrypted_content":"opaque continuity"})),
        Item::configuration_update(Effort::High),
    ];
    for kind in [ToolKind::Function, ToolKind::Custom] {
        for format in [OutputFormat::NativeReceiptV1, OutputFormat::LegacyOpaque] {
            for (index, terminal) in terminals.iter().enumerate() {
                let id = format!("{kind:?}-{format:?}-{index}");
                let parent = RequestId(id.clone());
                let call = CallId(id);
                store.create_request(&parent, None, "/root").unwrap();
                let invocation = Item(match kind {
                    ToolKind::Function => {
                        json!({"type":"function_call","call_id":call.0,"name":"probe","arguments":"{}"})
                    }
                    ToolKind::Custom => {
                        json!({"type":"custom_tool_call","call_id":call.0,"name":"probe","input":"raw Haskell λ","async":true})
                    }
                });
                store.append_items(&parent, &[invocation.clone()]).unwrap();
                let operation = store.claim(&call, &parent).unwrap();
                store
                    .write_job_output_with_format(&operation, kind, terminal, format)
                    .unwrap();
                let recorded = store
                    .replay_tool_output_operation(&operation)
                    .unwrap()
                    .unwrap();
                assert_eq!(recorded.output_format, format);
                assert_eq!(recorded.item.output_format().unwrap(), format);
                request.input.push(invocation);
                request.input.push(recorded.item.clone());
                records.push((operation, recorded));
            }
        }
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    let canonical = serde_json::to_value(&request).unwrap();
    let normalized = normalize_for_dispatch(
        &request,
        (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard),
    )
    .unwrap();
    let wire = http_request(
        &reqwest::Client::new(),
        &request,
        normalized.into_bytes(),
        "fixture-token",
    )
    .build()
    .unwrap();
    let compressed = wire.body().unwrap().as_bytes().unwrap();
    let body: Value =
        serde_json::from_slice(&zstd::stream::decode_all(compressed).unwrap()).unwrap();
    let projected = body["input"].as_array().unwrap();
    let expected = request
        .input
        .iter()
        .filter(|item| !item.is_configuration_update())
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), expected.len());
    for (wire, retained) in projected.iter().zip(expected) {
        if matches!(
            retained.0["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        ) {
            assert!(
                wire.get("output_format").is_none(),
                "internal format reached provider input"
            );
            assert_eq!(wire["type"], retained.0["type"]);
            assert_eq!(wire["call_id"], retained.0["call_id"]);
            assert_eq!(wire["output"], retained.0["output"]);
            assert!(wire["output"].as_str().unwrap().contains("user payload λ"));
        } else {
            assert_eq!(wire, &retained.0);
        }
    }
    assert_eq!(serde_json::to_value(&request).unwrap(), canonical);
    for (operation, recorded) in records {
        assert_eq!(
            store.replay_tool_output_operation(&operation).unwrap(),
            Some(recorded)
        );
    }
}

#[test]
fn public_route_enforces_its_wire_contract_and_allowed_host_tools() {
    let mut request = plan_request();
    request.tools_allowed = Some(vec!["yield".into()]);
    let body = request_body(&request).unwrap();
    assert_eq!(body["tools"][0]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["tools"][0]["name"], "yield");
    request.tools_allowed = Some(vec![]);
    let body = request_body(&request).unwrap();
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("tools").is_none());
    request.input.push(Item(
        json!({"type":"message","role":"system","content":"bad"}),
    ));
    assert!(request_body(&request).is_err());
    request.input.pop();
    request.tools = vec![json!({"type":"tool_search"})].into();
    assert!(request_body(&request).is_err());
}

#[test]
fn issued_identity_binds_root_child_fork_and_retry_headers_to_cache_affinity() {
    use crate::transport::SessionIdentity;
    let cases = [
        (SessionIdentity::standalone("root"), "root", "root"),
        (
            SessionIdentity::from_relationship("child".into(), Some("parent".into()), None),
            "child",
            "parent",
        ),
        (
            SessionIdentity::from_relationship("fork".into(), None, Some("source".into())),
            "fork",
            "source",
        ),
        (
            SessionIdentity::from_relationship(
                "both".into(),
                Some("parent".into()),
                Some("source".into()),
            ),
            "both",
            "parent",
        ),
    ];
    for (identity, thread, affinity) in cases {
        let store = crate::store::Store::memory().unwrap();
        let head = crate::model::RequestId(format!("issued-{thread}"));
        store.create_request(&head, None, "/root").unwrap();
        let mut caller = ResponsesRequest {
            input: vec![],
            session_id: identity.clone(),
            ..plan_request()
        };
        let contract = (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard);
        let prepared = store
            .prepare_provider_request(
                &head,
                &head,
                caller.clone(),
                &[],
                None,
                crate::transport::PreparedTransportSelection::component(contract),
                crate::store::ProviderRequestPurpose::Inference,
            )
            .unwrap();
        caller.session_id = SessionIdentity::standalone("changed-after-issuance");
        let frozen = prepared.request().clone();
        let wire = http_request(
            &http_client().unwrap(),
            &frozen,
            prepared.wire_body().to_vec(),
            "fixture-token",
        )
        .build()
        .unwrap();
        assert_eq!(wire.headers()["session-id"], affinity);
        assert_eq!(wire.headers()["thread-id"], thread);
        assert_eq!(wire.headers()["x-client-request-id"], thread);
        let decoded = zstd::stream::decode_all(wire.body().unwrap().as_bytes().unwrap()).unwrap();
        let body: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(body["prompt_cache_key"], affinity);
        assert_eq!(decoded, prepared.wire_body());
        let issuer = prepared.issuer().clone();
        store
            .record_provider_turn(
                &issuer,
                &crate::transport::ResponsesTurn {
                    response_id: "retained".into(),
                    items: vec![],
                    usage: Default::default(),
                },
            )
            .unwrap();
        let restored = store.replay_turns(&head).unwrap().remove(0).model_request;
        assert_eq!(restored.session_id, identity);
        let retry = store
            .prepare_provider_request(
                &head,
                &head,
                restored,
                &[],
                None,
                crate::transport::PreparedTransportSelection::component(contract),
                crate::store::ProviderRequestPurpose::Inference,
            )
            .unwrap();
        let retry_wire = http_request(
            &http_client().unwrap(),
            retry.request(),
            retry.wire_body().to_vec(),
            "rotated-fixture-token",
        )
        .build()
        .unwrap();
        for header in ["session-id", "thread-id", "x-client-request-id"] {
            assert_eq!(wire.headers()[header], retry_wire.headers()[header]);
        }
        assert_eq!(prepared.wire_body(), retry.wire_body());
    }
}

#[test]
fn affinity_cache_key_strips_only_exact_prefixed_digest_and_clamps() {
    use crate::transport::SessionIdentity;
    for (affinity, expected) in [
        (format!("ses_{}", "a".repeat(64)), "a".repeat(64)),
        (
            format!("ses_{}", "g".repeat(64)),
            format!("ses_{}", "g".repeat(60)),
        ),
        ("ses_short".into(), "ses_short".into()),
        ("x".repeat(70), "x".repeat(64)),
    ] {
        let request = ResponsesRequest {
            session_id: SessionIdentity::from_relationship(
                "current-thread".into(),
                Some(affinity.clone()),
                None,
            ),
            ..plan_request()
        };
        let body = request_body(&request).unwrap();
        assert_eq!(body["prompt_cache_key"], expected);
        let wire = http_request(
            &http_client().unwrap(),
            &request,
            serde_json::to_vec(&body).unwrap(),
            "fixture-token",
        )
        .build()
        .unwrap();
        assert_eq!(wire.headers()["session-id"], affinity);
        assert_eq!(wire.headers()["thread-id"], "current-thread");
    }
}

#[test]
fn retired_private_contracts_cannot_be_deserialized_as_public() {
    assert!(serde_json::from_value::<ResponsesRoute>(json!("codex")).is_err());
    assert!(serde_json::from_value::<ResponsesProtocol>(json!("lite")).is_err());
}

#[tokio::test]
async fn missing_plan_credentials_are_refused_before_traffic() {
    #[derive(Clone)]
    struct WrongAuthority;
    impl Auth for WrongAuthority {
        fn admission(
            &self,
        ) -> Result<crate::transport::NativeTransportAdmission, crate::transport::TransportError>
        {
            crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
                "/offline/synthetic-credential",
            ))
        }
        fn access(&self) -> Result<String, TransportError> {
            Err(TransportError::Authentication)
        }
    }
    let client = ResponsesClient::new(WrongAuthority);
    let selection = crate::engine::ResponsesTransport::prepare_submission(&client).unwrap();
    let store = crate::store::Store::memory().unwrap();
    let head = crate::model::RequestId("wrong-authority".into());
    store.create_request(&head, None, "/root").unwrap();
    let request = ResponsesRequest {
        input: vec![],
        ..plan_request()
    };
    let prepared = store
        .prepare_provider_request(
            &head,
            &head,
            request,
            &[],
            None,
            selection,
            crate::store::ProviderRequestPurpose::Inference,
        )
        .unwrap();
    assert_eq!(prepared.request_id(), &head);
    assert_eq!(
        prepared.response_contract(),
        (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard)
    );
    assert!(matches!(
        client.create(prepared).await,
        Err(TransportError::Authentication)
    ));
    assert_eq!(
        store.provider_response_state(&head).unwrap(),
        Some(crate::store::ProviderResponseState::Streaming)
    );
}

#[test]
fn streamed_async_calls_retain_the_original_item_and_call_id() {
    let original = json!({"type":"custom_tool_call","id":"item-1","namespace":"functions","name":"cell","call_id":"original-call","input":"work","async":true,"provider_extension":"kept"});
    let mut assembly = ResponseAssembly::default();
    let event = assembly
        .accept(
            &json!({"type":"response.output_item.done","output_index":0,"item":original})
                .to_string(),
        )
        .unwrap();
    assert!(
        matches!(event, Some(StreamEvent::CompletedItem(ref item)) if item.item().0 == original)
    );
    assembly
        .accept(
            &json!({"type":"response.completed","response":{"id":"response-1","output":[]}})
                .to_string(),
        )
        .unwrap();
    let turn = assembly.finish().unwrap();
    assert_eq!(turn.items[0].0, original);
    let call = turn.items[0].tool_call().unwrap().unwrap();
    assert_eq!(call.call_id.0, "original-call");
}

#[tokio::test]
async fn production_http_client_never_forwards_history_on_redirect() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = first.read(&mut request).await.unwrap();
        first.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        drop(first);
        match tokio::time::timeout(Duration::from_millis(100), listener.accept()).await {
            Err(_) => false,
            Ok(Ok((mut redirected, _))) => {
                let _ = redirected.read(&mut request).await;
                let _ = redirected
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                true
            }
            Ok(Err(error)) => panic!("redirect fixture failed: {error}"),
        }
    });
    let response = http_client()
        .unwrap()
        .post(format!("http://{address}/original"))
        .body("private request history")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    assert!(
        !server.await.unwrap(),
        "history was sent to the redirect target"
    );
}

#[test]
fn cache_comparison_defaults_omit_and_invalid_selections_refuse_before_dispatch() {
    let mut request = plan_request();
    let body = request_body(&request).unwrap();
    assert!(body.get("prompt_cache_options").is_none());
    request.prompt_cache_comparison = Some(crate::transport::PromptCacheComparison {
        comparison_response_id: "baseline".into(),
    });
    let body = request_body(&request).unwrap();
    assert_eq!(
        body["prompt_cache_options"],
        json!({"comparison_response_id":"baseline"})
    );
    request
        .prompt_cache_comparison
        .as_mut()
        .unwrap()
        .comparison_response_id
        .clear();
    assert!(request_body(&request).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_account_switch_during_held_stream_preserves_sent_authentication() {
    use crate::{
        embedding::{ClientOperationId, HostIdentity},
        model::AgentPath,
        server::HostCommand,
        store::Store,
        transport::auth::chatgpt::{RunAccountAuth, SavedAccounts},
    };
    use std::{io::Write, os::unix::fs::OpenOptionsExt, sync::Arc};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory =
        std::env::temp_dir().join(format!("harness-held-account-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let launch = directory.join("first.json");
    let second = directory.join("second.json");
    let fixture = include_str!("fixtures/saved-account-test.json");
    for (path, bytes) in [
        (&launch, fixture.to_owned()),
        (
            &second,
            fixture
                .replace("test-account-a", "test-account-b")
                .replace("first@example.test", "second@example.test"),
        ),
    ] {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap()
            .write_all(bytes.as_bytes())
            .unwrap();
    }
    let store = Arc::new(Store::memory().unwrap());
    let auth = RunAccountAuth::new(
        store.clone(),
        "run".into(),
        SavedAccounts::beside(launch).unwrap(),
    )
    .unwrap();
    let selected = auth
        .selection()
        .unwrap()
        .accounts
        .into_iter()
        .find(|entry| entry.label == "second@example.test")
        .unwrap()
        .id;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (held, observed) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut held = Some(held);
        let mut released = Some(released);
        for (index, expected) in ["test-account-a", "test-account-b"].into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0u8; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&chunk[..count]);
            }
            assert!(
                String::from_utf8_lossy(&bytes)
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {expected}"))
            );
            let prefix = format!(
                "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"r{index}\"}}}}\n\n"
            );
            let completed = format!(
                "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"r{index}\",\"output\":[]}}}}\n\n"
            );
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{prefix}", prefix.len() + completed.len()).as_bytes()).await.unwrap();
            if index == 0 {
                held.take().unwrap().send(()).unwrap();
                released.take().unwrap().await.unwrap();
            }
            socket.write_all(completed.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    // The production request builder and SSE reader use the captured token.
    // Only the destination is changed to a local fixture; native dispatch's
    // public endpoint remains fixed.
    async fn submit(auth: RunAccountAuth, address: std::net::SocketAddr) -> ResponsesTurn {
        let token = tokio::task::spawn_blocking(move || auth.access())
            .await
            .unwrap()
            .unwrap();
        let http = http_client().unwrap();
        let request = plan_request();
        let mut wire = http_request(
            &http,
            &request,
            serde_json::to_vec(&request_body(&request).unwrap()).unwrap(),
            &token,
        )
        .build()
        .unwrap();
        *wire.url_mut() = reqwest::Url::parse(&format!("http://{address}/responses")).unwrap();
        let response = http.execute(wire).await.unwrap();
        read_response(response, None, &token, "").await.unwrap()
    }
    let first_auth = auth.clone();
    let first = tokio::spawn(async move { submit(first_auth, address).await });
    tokio::time::timeout(Duration::from_secs(10), observed)
        .await
        .unwrap()
        .unwrap();
    assert!(!first.is_finished());
    let operation = ClientOperationId(uuid::Uuid::new_v4());
    store
        .enqueue_embedded_command(
            operation,
            &HostCommand::SelectAccount {
                target: HostIdentity {
                    run: "run".into(),
                    actor: AgentPath("/root".into()),
                    incarnation: "first".into(),
                },
                account_id: selected,
            },
        )
        .unwrap();
    store
        .claim_embedded_command("run", operation)
        .unwrap()
        .unwrap();
    let selecting = auth.clone();
    tokio::task::spawn_blocking(move || selecting.select_account(operation, |_| {}))
        .await
        .unwrap()
        .unwrap();
    assert!(!first.is_finished());
    release.send(()).unwrap();
    assert_eq!(first.await.unwrap().response_id, "r0");
    assert_eq!(submit(auth, address).await.response_id, "r1");
    server.await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
