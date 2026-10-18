//! Protocol fixtures follow the official Interactions OpenAPI and thinking REST
//! example, retained under the task's interactions-contract evidence directory.
//! https://ai.google.dev/static/api/interactions.openapi.json
//! https://ai.google.dev/gemini-api/docs/thinking
//! https://ai.google.dev/gemini-api/docs/function-calling#stateless-mode
use super::super::{ResponseEnd, TransportObservation};
use super::*;

fn observation() -> TransportObservation {
    TransportObservation::new(ResponseEnd::ProviderTerminal)
}
fn send(assembly: &mut Assembly, event: Value) -> Result<Vec<StreamEvent>, TransportError> {
    assembly
        .advance(&event.to_string(), "", "", &mut observation())
        .map_err(ProtocolFault::into_error)?
        .into_events(None)
}
fn created() -> Assembly {
    let mut assembly = Assembly::default();
    send(&mut assembly, json!({"event_type":"interaction.created","interaction":{"id":"turn-λ","status":"in_progress"}})).unwrap();
    assembly
}
fn start(assembly: &mut Assembly, index: u64, step: Value) {
    send(
        assembly,
        json!({"event_type":"step.start","index":index,"step":step}),
    )
    .unwrap();
}
fn stop(assembly: &mut Assembly, index: u64) -> Vec<StreamEvent> {
    send(assembly, json!({"event_type":"step.stop","index":index})).unwrap()
}
fn terminal(assembly: &mut Assembly, status: &str) {
    // Schema-derived requires_action fixture; this is not a live server capture.
    send(
        assembly,
        json!({"event_type":"interaction.status_update","interaction_id":"turn-λ","status":status}),
    )
    .unwrap();
}
fn request_fixture() -> ResponsesRequest {
    ResponsesRequest {
        input: vec![Item::user_text("hello λ")],
        instructions: "instructions".into(),
        tools: Vec::<Value>::new().into(),
        tools_allowed: None,
        model: "gemini-4-argon".into(),
        pinned_effort: Effort::Medium,
        session_id: "session".into(),
        prompt_cache_comparison: None,
    }
}
fn normalize(request_value: &ResponsesRequest, authored: &[bool]) -> Result<Value, TransportError> {
    request(request_value, InferenceLimits::gemini_default(), authored)
}

#[test]
fn late_signature_retains_initial_summary_and_exact_native_step() {
    let mut assembly = created();
    start(
        &mut assembly,
        0,
        json!({"type":"thought","signature":"","summary":[{"type":"text","text":"initial λ"}]}),
    );
    send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"second"}}})).unwrap();
    assert!(send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"opaque+/="}})).unwrap().is_empty());
    let done = stop(&mut assembly, 0);
    let StreamEvent::CompletedItem(done) = &done[0] else {
        panic!("expected complete thought")
    };
    assert_eq!(done.output_index(), 0);
    assert_eq!(
        done.item().0["summary"],
        json!([{"type":"summary_text","text":"initial λ"},{"type":"summary_text","text":"second"}])
    );
    assert_eq!(
        done.item().0["gemini_step"],
        json!({"type":"thought","signature":"opaque+/=","summary":[{"type":"text","text":"initial λ"},{"type":"text","text":"second"}]})
    );
    terminal(&mut assembly, "completed");
    assert_eq!(assembly.finish_complete().unwrap().items[0], *done.item());
}

#[test]
fn signature_without_summary_survives_replay_without_inventing_summary() {
    let mut assembly = created();
    start(&mut assembly, 0, json!({"type":"thought"}));
    send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"opaque"}})).unwrap();
    stop(&mut assembly, 0);
    terminal(&mut assembly, "completed");
    let item = assembly.finish_complete().unwrap().items.remove(0);
    assert_eq!(item.0["summary"], json!([]));
    let mut request = request_fixture();
    request.input.push(item);
    let body = normalize(&request, &[true, false]).unwrap();
    assert_eq!(
        body["input"][1],
        json!({"type":"thought","signature":"opaque"})
    );
}

#[test]
fn arguments_fragments_and_out_of_order_stops_release_calls_in_provider_order() {
    let mut assembly = created();
    start(
        &mut assembly,
        0,
        json!({"type":"function_call","id":"a","name":"echo","arguments":{}}),
    );
    start(
        &mut assembly,
        1,
        json!({"type":"function_call","id":"b","name":"echo","arguments":{"n":2}}),
    );
    for arguments in ["{\"nested\":", "{\"λ\":[1,true]}}"] {
        send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":arguments}})).unwrap();
    }
    assert!(stop(&mut assembly, 1).is_empty());
    let done = stop(&mut assembly, 0);
    let indices = done
        .iter()
        .map(|event| match event {
            StreamEvent::CompletedItem(item) => item.output_index(),
            _ => panic!("unexpected delta"),
        })
        .collect::<Vec<_>>();
    assert_eq!(indices, vec![0, 1]);
    terminal(&mut assembly, "requires_action");
    let turn = assembly.finish_complete().unwrap();
    assert_eq!(
        turn.items[0].tool_call().unwrap().unwrap().input,
        crate::item::ToolInput::Function(json!({"nested":{"λ":[1,true]}}))
    );
    let mut request = request_fixture();
    request.input.extend(turn.items.clone());
    request.input.extend([
        Item(json!({"type":"function_call_output","call_id":"b","output":"second"})),
        Item(json!({"type":"function_call_output","call_id":"a","output":{"ok":true}})),
    ]);
    let body = normalize(&request, &[true, false, false, false, false]).unwrap();
    assert_eq!(body["input"][1], turn.items[0].0["gemini_step"]);
    assert_eq!(body["input"][2], turn.items[1].0["gemini_step"]);
    assert_eq!(
        body["input"][3],
        json!({"type":"function_result","call_id":"a","name":"echo","result":{"ok":true}})
    );
    assert_eq!(
        body["input"][4],
        json!({"type":"function_result","call_id":"b","name":"echo","result":[{"type":"text","text":"second"}]})
    );
}

#[test]
fn event_id_replay_is_idempotent_but_conflicting_bytes_are_refused() {
    let mut assembly = created();
    let event = json!({"event_id":"one","event_type":"step.start","index":0,"step":{"type":"model_output","content":[{"type":"text","text":"once"}]}});
    send(&mut assembly, event.clone()).unwrap();
    assert!(send(&mut assembly, event.clone()).unwrap().is_empty());
    let mut conflict = event;
    conflict["step"]["content"][0]["text"] = "changed".into();
    assert!(send(&mut assembly, conflict).is_err());
}

#[test]
fn incomplete_and_gapped_steps_never_produce_a_successful_turn() {
    let mut unfinished = created();
    start(&mut unfinished, 0, json!({"type":"model_output"}));
    assert!(send(&mut unfinished, json!({"event_type":"interaction.completed","interaction":{"id":"turn-λ","status":"completed"}})).is_err());
    assert!(unfinished.finish_complete().is_err());
    let mut gap = created();
    start(&mut gap, 1, json!({"type":"model_output","content":[]}));
    assert!(stop(&mut gap, 1).is_empty());
    terminal(&mut gap, "completed");
    assert!(gap.finish_complete().is_err());
    let mut no_terminal = created();
    start(
        &mut no_terminal,
        0,
        json!({"type":"model_output","content":[]}),
    );
    stop(&mut no_terminal, 0);
    assert!(no_terminal.finish_complete().is_err());
}

#[test]
fn duplicate_indices_late_deltas_and_invalid_arguments_are_refused() {
    let mut duplicate = created();
    start(&mut duplicate, 0, json!({"type":"model_output"}));
    assert!(
        send(
            &mut duplicate,
            json!({"event_type":"step.start","index":0,"step":{"type":"model_output"}})
        )
        .is_err()
    );
    let mut late = created();
    start(&mut late, 0, json!({"type":"model_output","content":[]}));
    stop(&mut late, 0);
    assert!(
        send(
            &mut late,
            json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"late"}})
        )
        .is_err()
    );
    let mut invalid = created();
    start(
        &mut invalid,
        0,
        json!({"type":"function_call","id":"a","name":"echo","arguments":{}}),
    );
    send(&mut invalid, json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"{broken"}})).unwrap();
    assert!(send(&mut invalid, json!({"event_type":"step.stop","index":0})).is_err());
}

#[test]
fn native_projection_mutations_and_foreign_provider_history_are_refused() {
    let native = json!({"type":"function_call","id":"call","name":"echo","arguments":{"n":1}});
    let call = project(native, "turn:0").unwrap();
    for (field, value) in [
        ("name", json!("other")),
        ("call_id", json!("different")),
        ("arguments", json!({"n":2})),
    ] {
        let mut changed = call.clone();
        changed.0[field] = value;
        let mut request = request_fixture();
        request.input.push(changed);
        request.input.push(Item(
            json!({"type":"function_call_output","call_id":"call","output":"ok"}),
        ));
        assert!(
            normalize(&request, &[true, false, false]).is_err(),
            "accepted changed {field}"
        );
    }
    for foreign in [
        json!({"type":"reasoning","encrypted_content":"opaque","summary":[]}),
        json!({"type":"message","role":"assistant","content":"foreign response"}),
        json!({"type":"function_call","call_id":"foreign","name":"echo","arguments":"{}"}),
    ] {
        let mut request = request_fixture();
        request.input.push(Item(foreign));
        assert!(normalize(&request, &[true, false]).is_err());
    }
}

#[test]
fn only_argon_and_supported_reasoning_levels_are_admitted() {
    let mut request = request_fixture();
    for model in [
        "gemini-3.8-flash",
        "gemini-4-argon-preview",
        "deepseek-flash",
        "",
    ] {
        request.model = model.into();
        assert!(normalize(&request, &[true]).is_err());
    }
    request.model = "gemini-4-argon".into();
    for effort in [Effort::Low, Effort::Medium, Effort::High] {
        request.pinned_effort = effort;
        let body = normalize(&request, &[true]).unwrap();
        assert_eq!(
            body["generation_config"]["thinking_level"],
            serde_json::to_value(effort).unwrap()
        );
        assert_eq!(body["store"], false);
        assert!(body.get("previous_interaction_id").is_none());
    }
    assert!(normalize(&request, &[]).is_err());
}

#[test]
fn requires_action_needs_a_call_and_failed_status_is_not_success() {
    let mut empty = created();
    assert!(send(&mut empty, json!({"event_type":"interaction.status_update","interaction_id":"turn-λ","status":"requires_action"})).is_err());
    for status in ["failed", "cancelled", "incomplete"] {
        let mut assembly = created();
        assert!(send(&mut assembly,json!({"event_type":"interaction.completed","interaction":{"id":"turn-λ","status":status}})).is_err());
        assert!(assembly.finish_complete().is_err());
    }
}

#[test]
fn usage_missing_components_are_unknown_and_explicit_zero_remains_zero() {
    let absent = Usage::from_interaction_usage(&json!({}));
    assert_eq!(absent.input_tokens, None);
    assert_eq!(absent.output_tokens, None);
    let zero = Usage::from_interaction_usage(
        &json!({"usage":{"total_input_tokens":0,"total_output_tokens":0,"total_thought_tokens":0,"total_cached_tokens":0}}),
    );
    assert_eq!(zero.input_tokens, Some(0));
    assert_eq!(zero.output_tokens, Some(0));
    assert_eq!(zero.cached_tokens, Some(0));
    let partial = Usage::from_interaction_usage(&json!({"usage":{"total_output_tokens":5}}));
    assert_eq!(partial.output_tokens, None);
    assert_eq!(partial.reasoning_tokens, None);
    let full = Usage::from_interaction_usage(
        &json!({"usage":{"total_input_tokens":2,"total_output_tokens":5,"total_thought_tokens":7}}),
    );
    assert_eq!(full.output_tokens, Some(12));
    assert_eq!(full.reasoning_tokens, Some(7));
}

mod joined_http {
    use super::*;
    use crate::{
        context::ContextBlock,
        engine::EngineConfig,
        invocation::{
            CellBudget, Invocation, InvocationOptions, Limits, Outcome, Step as InvocationStep,
        },
        model::{AgentPath, CallId, RequestId},
        store::{ProviderRequestPurpose, Store},
        transport::{
            Auth, CredentialReference, NativeTransportAdmission, NativeTransportContract,
            PreparedTransportSelection, ResponsesClient, ResponsesProtocol, ResponsesRoute,
            auth::GeminiAuth,
        },
        turn::JobScheduler,
    };
    use axum::{
        Router,
        body::{Body, Bytes},
        extract::State,
        http::HeaderMap,
        response::Response,
        routing::post,
    };
    use std::{
        collections::VecDeque,
        io::Write,
        os::unix::fs::OpenOptionsExt,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::sync::mpsc;

    const KEY: &str = "offline-synthetic-gemini-key";
    struct Files(PathBuf);
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[derive(Clone)]
    struct ServerState {
        responses: Arc<Mutex<VecDeque<Vec<u8>>>>,
        captured: mpsc::UnboundedSender<(HeaderMap, Value)>,
    }
    async fn respond(
        State(state): State<ServerState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        state
            .captured
            .send((headers, serde_json::from_slice(&body).unwrap()))
            .unwrap();
        let bytes = state
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("exactly two HTTP requests");
        // Fragment through UTF-8 boundaries and SSE delimiters at the HTTP body layer.
        let chunks = bytes
            .chunks(3)
            .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(futures_util::stream::iter(chunks)))
            .unwrap()
    }
    fn sse(events: Vec<Value>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend_from_slice(
                format!(
                    "event: {}\ndata: {}\n\n",
                    event["event_type"].as_str().unwrap(),
                    event
                )
                .as_bytes(),
            );
        }
        bytes.extend_from_slice(b"event: done\ndata: [DONE]\n\n");
        bytes
    }
    fn assert_request(headers: &HeaderMap, body: &Value) {
        assert_eq!(headers["x-goog-api-key"], KEY);
        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("content-encoding"));
        assert_eq!(body["model"], "gemini-4-argon");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(body.get("previous_interaction_id").is_none());
    }
    async fn next(invocation: &mut Invocation) -> InvocationStep {
        tokio::time::timeout(Duration::from_secs(10), invocation.next())
            .await
            .expect("bounded local HTTP progress")
    }

    #[tokio::test]
    async fn native_http_tool_history_survives_store_reopen_and_exact_next_preparation() {
        let files = Files(
            std::env::temp_dir().join(format!("gemini-native-http-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&files.0).unwrap();
        let key_path = files.0.join("key");
        let mut key = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_path)
            .unwrap();
        writeln!(key, "{KEY}").unwrap();
        drop(key);
        let thought = json!({"type":"thought","signature":"opaque+/=","summary":[{"type":"text","text":"retained λ"}],"future_native_field":{"keep":[1,2]}});
        let calls = [
            json!({"type":"function_call","id":"call-a","name":"echo","arguments":{"n":1},"future_call_field":"preserve"}),
            json!({"type":"function_call","id":"call-b","name":"echo","arguments":{"n":2}}),
        ];
        let mut initial_thought = thought.clone();
        initial_thought["signature"] = "".into();
        let first = sse(vec![
            json!({"event_type":"interaction.created","interaction":{"id":"http-one","status":"in_progress"}}),
            json!({"event_type":"step.start","index":0,"step":initial_thought}),
            json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"opaque+/="}}),
            json!({"event_type":"step.stop","index":0}),
            json!({"event_type":"step.start","index":1,"step":calls[0]}),
            json!({"event_type":"step.start","index":2,"step":calls[1]}),
            json!({"event_type":"step.stop","index":2}),
            json!({"event_type":"step.stop","index":1}),
            // Schema-derived status event, not a claimed live server capture.
            json!({"event_type":"interaction.status_update","interaction_id":"http-one","status":"requires_action","usage":{"total_input_tokens":10,"total_output_tokens":4,"total_thought_tokens":3}}),
        ]);
        let final_step = json!({"type":"model_output","content":[{"type":"text","text":"done λ"}],"future_output_field":true});
        let second = sse(vec![
            json!({"event_type":"interaction.created","interaction":{"id":"http-two","status":"in_progress"}}),
            json!({"event_type":"step.start","index":0,"step":final_step}),
            json!({"event_type":"step.stop","index":0}),
            json!({"event_type":"interaction.completed","interaction":{"id":"http-two","status":"completed","usage":{"total_input_tokens":0,"total_output_tokens":0,"total_thought_tokens":0}}}),
        ]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1beta/interactions",
            listener.local_addr().unwrap()
        );
        let (sent, mut captured) = mpsc::unbounded_channel();
        let responses = Arc::new(Mutex::new(VecDeque::from([first, second])));
        let app = Router::new()
            .route("/v1beta/interactions", post(respond))
            .with_state(ServerState {
                responses: responses.clone(),
                captured: sent,
            });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let admission = NativeTransportAdmission::rehydrate(NativeTransportContract {
            route: ResponsesRoute::GeminiInteractions,
            protocol: ResponsesProtocol::Interactions,
            endpoint,
            credential: CredentialReference::File { path: key_path },
            limits: InferenceLimits::gemini_default(),
        })
        .unwrap();
        let auth = GeminiAuth::from_admission(admission.clone()).unwrap();
        assert_eq!(auth.admission().unwrap().contract(), admission.contract());
        let db = files.0.join("store.sqlite");
        let store = Arc::new(Store::open(&db).unwrap());
        let mut invocation = Invocation::start::<GeminiAuth,_>(ResponsesClient::new(auth), store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()), EngineConfig {
                instructions:"joined native workflow".into(),
                tools:vec![json!({"type":"function","name":"echo","strict":true,"parameters":{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false}})],
                model:"gemini-4-argon".into(), effort:Effort::High, session_id:"joined".into(), agent:AgentPath("/joined".into()),
            }, "joined-http".into(), CellBudget::new(Limits {requests:2,tools:2,reported_tokens:1000,seconds:20}),
            vec![Item::user_text("run both λ")], InvocationOptions::default()).unwrap();
        // Invocation exposes one callback until its continuation is resumed.
        // Reversed settlement arrival is covered through the request normalizer.
        for n in [1, 2] {
            let InvocationStep::Callback(callback) = next(&mut invocation).await else {
                panic!("native call {n} must reach production callback")
            };
            assert_eq!(callback.name, "echo");
            assert_eq!(callback.arguments, json!({"n":n}));
            callback
                .complete(Ok(json!({"answer":n,"nested":{"λ":[true,null]}})))
                .unwrap();
        }
        let InvocationStep::Finished(receipt) = next(&mut invocation).await else {
            panic!("two callbacks must finish")
        };
        assert!(
            matches!(&receipt.outcome,Outcome::Text(text) if text == "done λ"),
            "{:?}",
            receipt.outcome
        );
        assert_eq!(receipt.counts.requests, 2);
        assert_eq!(receipt.counts.tools, 2);
        let (first_headers, first_body) = captured.try_recv().unwrap();
        assert_request(&first_headers, &first_body);
        assert_eq!(
            first_body["input"],
            json!([{"type":"user_input","content":[{"type":"text","text":"run both λ"}]}])
        );
        let (headers, body) = captured.try_recv().unwrap();
        assert_request(&headers, &body);
        let input = body["input"].as_array().unwrap();
        assert_eq!(
            input
                .iter()
                .filter(|step| step["type"] == "thought")
                .collect::<Vec<_>>(),
            vec![&thought]
        );
        assert_eq!(
            input
                .iter()
                .filter(|step| step["type"] == "function_call")
                .collect::<Vec<_>>(),
            calls.iter().collect::<Vec<_>>()
        );
        let results = input
            .iter()
            .filter(|step| step["type"] == "function_result")
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        for (result, n) in results.iter().zip([1, 2]) {
            assert_eq!(result["call_id"], if n == 1 { "call-a" } else { "call-b" });
            assert_eq!(result["name"], "echo");
            let text = result["result"][0]["text"].as_str().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(text).unwrap(),
                json!({"answer":n,"nested":{"λ":[true,null]}})
            );
        }
        assert!(responses.lock().unwrap().is_empty());
        assert!(captured.try_recv().is_err());
        for id in ["call-a", "call-b"] {
            assert_eq!(store.claims(&CallId(id.into())).unwrap().len(), 1);
        }
        let head = receipt.requests.last().unwrap().clone();
        let branch = store.request(&head).unwrap().unwrap().branch;
        let original = receipt.transcript.clone();
        drop(invocation);
        drop(store);
        let reopened = Store::open(&db).unwrap();
        for request_id in &receipt.requests {
            for item in reopened
                .items(request_id)
                .unwrap()
                .iter()
                .filter(|item| item.0.get("gemini_step").is_some())
            {
                assert!(original.contains(item), "reopened native original changed");
            }
        }
        let document = reopened.read_context(&head).unwrap();
        assert!(
            document.blocks.iter().any(|block| matches!(
                block,
                ContextBlock::Native {
                    protected: true,
                    ..
                }
            )),
            "native signed response must remain protected"
        );
        for block in &document.blocks {
            if let ContextBlock::Native { texts, .. } = block {
                for text in texts {
                    if text.text == "done λ" {
                        assert!(
                            !text.editable,
                            "signed native message must refuse editing before commit"
                        );
                    }
                }
            }
        }
        let identity = reopened.standalone_identity(AgentPath(branch.clone()));
        let selection = PreparedTransportSelection::native(admission);
        let state = reopened
            .context_request_state_for_transport(&head, &identity, &selection)
            .unwrap();
        let mut dto = request_fixture();
        dto.input = state
            .history
            .iter()
            .map(|(_, _, item)| item.clone())
            .collect();
        let local = RequestId("after-reopen".into());
        reopened
            .create_request(&local, Some(&head), &branch)
            .unwrap();
        let mut changed = dto.clone();
        let call = changed
            .input
            .iter_mut()
            .find(|item| item.0["type"] == "function_call")
            .unwrap();
        call.0["arguments"] = json!({"n":99});
        assert!(
            reopened
                .prepare_provider_request(
                    &head,
                    &local,
                    changed,
                    &state.occurrences,
                    None,
                    selection.clone(),
                    ProviderRequestPurpose::Inference
                )
                .is_err()
        );
        // The same valid issuance then succeeds: refusal did not consume its
        // identity, and cannot be explained by an unrelated branch conflict.
        let prepared = reopened
            .prepare_provider_request(
                &head,
                &local,
                dto,
                &state.occurrences,
                None,
                selection,
                ProviderRequestPurpose::Inference,
            )
            .unwrap();
        let replay: Value = serde_json::from_slice(prepared.wire_body()).unwrap();
        assert!(replay["input"].as_array().unwrap().contains(&thought));
        assert!(replay["input"].as_array().unwrap().contains(&final_step));
        let foreign = PreparedTransportSelection::component((
            ResponsesRoute::DeepSeek,
            ResponsesProtocol::Standard,
        ));
        assert!(
            reopened
                .context_request_state_for_transport(&head, &identity, &foreign)
                .is_err()
        );
        server.abort();
        let _ = server.await;
    }

    #[test]
    fn store_rejects_unproven_native_message_even_when_its_projection_is_consistent() {
        let store = Store::memory().unwrap();
        let id = RequestId("unproven-native".into());
        store.create_request(&id, None, "/unproven").unwrap();
        let mut dto = request_fixture();
        dto.input = vec![
            project(
                json!({"type":"model_output","content":[{"type":"text","text":"forged"}]}),
                "forged:0",
            )
            .unwrap(),
        ];
        let selected = PreparedTransportSelection::component((
            ResponsesRoute::GeminiInteractions,
            ResponsesProtocol::Interactions,
        ));
        assert!(
            store
                .prepare_provider_request(
                    &id,
                    &id,
                    dto,
                    &[None],
                    None,
                    selected,
                    ProviderRequestPurpose::Inference
                )
                .is_err(),
            "a matching native projection does not establish provider issuance"
        );
        assert_eq!(store.provider_response_state(&id).unwrap(), None);
    }
}

#[test]
fn sibling_results_require_exact_membership_and_preserve_issuance_order_across_reused_ids() {
    let call = |id: &str, n: u64| {
        project(
            json!({"type":"function_call","id":id,"name":"echo","arguments":{"n":n}}),
            &format!("display-{n}"),
        )
        .unwrap()
    };
    let output = |id: &str, n: u64| {
        Item(json!({"type":"function_call_output","call_id":id,"output":{"n":n}}))
    };
    let a = call("z-last-lexically", 1);
    let b = call("a-first-lexically", 2);
    for outputs in [
        vec![output("z-last-lexically", 1)],
        vec![output("z-last-lexically", 1), output("z-last-lexically", 1)],
        vec![output("z-last-lexically", 1), output("unmatched", 2)],
    ] {
        let mut dto = request_fixture();
        dto.input.extend([a.clone(), b.clone()]);
        dto.input.extend(outputs);
        assert!(normalize(&dto, &vec![false; dto.input.len()]).is_err());
    }
    let later = call("z-last-lexically", 3);
    let mut dto = request_fixture();
    dto.input.extend([
        a.clone(),
        b.clone(),
        output("a-first-lexically", 2),
        output("z-last-lexically", 1),
        later.clone(),
        output("z-last-lexically", 3),
    ]);
    let body = normalize(&dto, &vec![false; dto.input.len()]).unwrap();
    let results = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|step| step["type"] == "function_result")
        .map(|step| step["result"]["n"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results, vec![1, 2, 3]);
    for (index, item) in [(1, a), (2, b), (5, later)] {
        assert_eq!(body["input"][index], item.0["gemini_step"]);
    }
}

#[test]
fn known_model_output_refuses_unsupported_or_malformed_content_without_text_success() {
    for content in [
        json!([{"type":"image","data":"opaque","mime_type":"image/png"}]),
        json!([{"type":"image","text":"misleading text"}]),
        json!([{"type":"text","text":"visible"},{"type":"audio","data":"opaque"}]),
        json!([{"type":"text","text":42}]),
        json!({"type":"text","text":"not an array"}),
    ] {
        let mut assembly = created();
        start(
            &mut assembly,
            0,
            json!({"type":"model_output","content":content}),
        );
        assert!(
            send(&mut assembly, json!({"event_type":"step.stop","index":0})).is_err(),
            "unsupported output became a completed text item: {content}"
        );
    }
    let mut assembly = created();
    let native = json!({"type":"future_server_observation","payload":{"opaque":"retain"}});
    start(&mut assembly, 0, native.clone());
    let events = stop(&mut assembly, 0);
    let StreamEvent::CompletedItem(done) = &events[0] else {
        panic!("expected inert native evidence")
    };
    assert_eq!(done.item().0["type"], "gemini_native");
    assert_eq!(done.item().0["gemini_step"], native);
    assert!(done.item().tool_call().unwrap().is_none());
}

#[test]
fn moved_input_and_config_match_previous_wire_construction() {
    for count in [0, 1, 16] {
        for instructions in ["", "policy λ"] {
            let mut request = request_fixture();
            request.input = (0..count).map(|_| Item::user_text("text λ\u{0}")).collect();
            request.instructions = instructions.into();
            let limits = InferenceLimits::gemini_default();
            let actual = super::request(&request, limits, &vec![true; count]).unwrap();
            let input: Vec<_> = (0..count).map(|_| json!({"type":"user_input","content":[{"type":"text","text":"text λ\u{0}"}]})).collect();
            let mut config = json!({"thinking_level":Effort::Medium,"thinking_summaries":"auto"});
            if let Some(limit) = limits.max_output_tokens {
                config["max_output_tokens"] = limit.into();
            }
            // Previous json! construction, independent of the returned body.
            let mut previous = json!({"model":request.model,"input":input,"store":false,"stream":true,"generation_config":config});
            if !instructions.is_empty() {
                previous["system_instruction"] = instructions.into();
            }
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&previous).unwrap()
            );
            assert!(matches!(
                super::request(&request, limits, &vec![true; count + 1]),
                Err(TransportError::InvalidAdmission)
            ));
        }
    }
}

#[test]
fn moved_gemini_body_retains_typed_native_overlay_refusal() {
    let mut request = request_fixture();
    request.input = vec![Item(
        json!({"type":"message","role":"user","content":"changed","gemini_step":{"type":"user_input","content":[{"type":"text","text":"original"}]}}),
    )];
    assert!(matches!(
        normalize(&request, &[false]),
        Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::InputItem
        ))
    ));
}
