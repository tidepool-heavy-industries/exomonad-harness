//! The ordinary and compaction consumers retain the same stopped-provider facts
//! through real native admission, HTTP, Engine persistence and Store reopen.
use super::*;
use crate::store::{ProviderRequestPurpose, ProviderResponseState};
use crate::transport::{
    CredentialReference, IncompleteReason, InferenceLimits, NativeTransportAdmission,
    NativeTransportContract, ProviderStreamFailureEvent, ResponseEnd, ResponseTerminal,
    ResponsesProtocol, ResponsesRoute, StreamInterruption, TransportObservation,
    auth::{DeepSeekAuth, GeminiAuth},
};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, Debug)]
enum End {
    Eof,
    ReadFailure,
    Incomplete,
    Failed,
    Cancelled,
    Malformed,
    InvalidComplete,
}
struct NoEffects;
#[async_trait::async_trait]
impl Provider for NoEffects {
    async fn call(&self, _: &str, _: Value) -> Result<Value, crate::provider::ProviderError> {
        panic!("native terminal control cannot dispatch an effect")
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}
struct NativeHttp {
    directory: PathBuf,
    admission: NativeTransportAdmission,
    prefix: Item,
    server: tokio::task::JoinHandle<()>,
}
impl NativeHttp {
    async fn new(gemini: bool, end: End) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "native-terminal-disposition-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("key");
        let mut key = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        writeln!(key, "offline-native-terminal-key").unwrap();
        let step = json!({"type":"thought","signature":"opaque+/λ=","summary":[{"type":"text","text":"retained λ"}]});
        let prefix = if gemini {
            Item(
                json!({"type":"reasoning","id":"native:0","summary":[{"type":"summary_text","text":"retained λ"}],"gemini_step":step}),
            )
        } else {
            Item(
                json!({"type":"reasoning","id":"prefix","summary":[],"encrypted_content":"opaque+/λ="}),
            )
        };
        let mut events = if gemini {
            vec![
                json!({"event_type":"interaction.created","interaction":{"id":"native"}}),
                json!({"event_type":"step.start","index":0,"step":step}),
                json!({"event_type":"step.stop","index":0}),
            ]
        } else {
            vec![
                json!({"type":"response.created","response":{"id":"native"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":prefix.0}),
            ]
        };
        if matches!(end, End::InvalidComplete) {
            if gemini {
                events.extend([json!({"event_type":"step.start","index":2,"step":{"type":"thought","signature":"gapped"}}),json!({"event_type":"step.stop","index":2})]);
            } else {
                events.push(json!({"type":"response.output_item.done","output_index":2,"item":{"type":"reasoning","id":"gap","summary":[]}}));
            }
        }
        let terminal = match (gemini, end) {
            (_, End::Eof | End::ReadFailure) => None,
            (true, End::Incomplete) => Some(
                json!({"event_type":"interaction.status_update","interaction_id":"native","status":"incomplete"}),
            ),
            (true, End::Failed) => Some(
                json!({"event_type":"interaction.status_update","interaction_id":"native","status":"failed"}),
            ),
            (true, End::Cancelled) => Some(
                json!({"event_type":"interaction.status_update","interaction_id":"native","status":"cancelled"}),
            ),
            (true, End::Malformed) => Some(
                json!({"event_type":"interaction.status_update","interaction_id":"native","status":"unsupported"}),
            ),
            (true, End::InvalidComplete) => Some(
                json!({"event_type":"interaction.completed","interaction":{"id":"native","status":"completed"}}),
            ),
            (false, End::Incomplete) => Some(
                json!({"type":"response.incomplete","response":{"id":"native","incomplete_details":{"reason":"content_filter"}}}),
            ),
            (false, End::Failed | End::Cancelled) => {
                Some(json!({"type":"response.failed","response":{"id":"native"}}))
            }
            (false, End::Malformed) => Some(json!({"type":"response.output_item.done","item":{}})),
            (false, End::InvalidComplete) => {
                Some(json!({"type":"response.completed","response":{"id":"native","output":[]}}))
            }
        };
        events.extend(terminal);
        let body = events
            .into_iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect::<String>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            if gemini {
                "/v1beta/interactions"
            } else {
                "/responses"
            }
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut part = [0; 4096];
                let received = socket.read(&mut part).await.unwrap();
                assert_ne!(received, 0);
                request.extend_from_slice(&part[..received]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..end])
                        .unwrap()
                        .to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() < end + 4 + length {
                        continue;
                    }
                    assert!(headers.contains(if gemini {
                        "x-goog-api-key: offline-native-terminal-key"
                    } else {
                        "authorization: bearer offline-native-terminal-key"
                    }));
                    let json: Value =
                        serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    assert_eq!(json["store"], false);
                    break;
                }
            }
            let length = body.len() + usize::from(matches!(end, End::ReadFailure)) * 100;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let admission = NativeTransportAdmission::rehydrate(NativeTransportContract {
            route: if gemini {
                ResponsesRoute::GeminiInteractions
            } else {
                ResponsesRoute::DeepSeek
            },
            protocol: if gemini {
                ResponsesProtocol::Interactions
            } else {
                ResponsesProtocol::Standard
            },
            endpoint,
            credential: CredentialReference::File { path },
            limits: if gemini {
                InferenceLimits::gemini_default()
            } else {
                InferenceLimits::deepseek_default()
            },
        })
        .unwrap();
        Self {
            directory,
            admission,
            prefix,
            server,
        }
    }
    fn store(&self) -> Arc<Store> {
        Arc::new(Store::open(self.directory.join("store.sqlite")).unwrap())
    }
}
impl Drop for NativeHttp {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
async fn check<A: Auth + Clone + 'static>(
    http: &NativeHttp,
    auth: A,
    gemini: bool,
    compaction: bool,
    end: End,
) {
    let store = http.store();
    let client = ResponsesClient::new(auth);
    let selected = client.prepare_submission().unwrap();
    let engine = Engine::with_transport(
        client,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoEffects),
        EngineConfig {
            instructions: "native terminal disposition".into(),
            tools: vec![],
            model: if gemini {
                "gemini-4-argon"
            } else {
                "deepseek-v4-pro"
            }
            .into(),
            effort: Effort::Low,
            session_id: "native-terminal".into(),
            agent: AgentPath("/test".into()),
        },
    );
    let (_cancel, cancellation) = watch::channel(false);
    let error = if compaction {
        let source = RequestId("source".into());
        let child = RequestId("compactor".into());
        store.create_request(&source, None, "/test").unwrap();
        store
            .create_request(&child, Some(&source), "/test")
            .unwrap();
        let prepared = store
            .prepare_provider_request(
                &source,
                &child,
                ResponsesRequest {
                    input: vec![],
                    instructions: "native compaction terminal".into(),
                    tools: vec![].into(),
                    tools_allowed: None,
                    model: engine.config.model.clone(),
                    pinned_effort: Effort::Low,
                    session_id: "native-terminal".into(),
                    prompt_cache_comparison: None,
                },
                &[],
                None,
                selected,
                ProviderRequestPurpose::ServerCompaction,
            )
            .unwrap();
        engine
            .execute_compaction_submission(prepared, cancellation, &source)
            .await
            .unwrap_err()
    } else {
        engine
            .run(
                None,
                vec![Item::user_text("retain the native prefix")],
                cancellation,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap_err()
    };
    let state = match end {
        End::Eof | End::ReadFailure => {
            let cause = if matches!(end, End::Eof) {
                StreamInterruption::MissingCompletion
            } else {
                StreamInterruption::ReadFailed
            };
            if compaction {
                assert!(
                    matches!(error,EngineError::Transport(TransportError::IncompleteResponse(actual)) if actual == cause)
                );
            } else {
                assert!(
                    matches!(error,EngineError::InterruptedModelRound {cause:actual,..} if actual == cause)
                );
            }
            ProviderResponseState::Interrupted
        }
        End::Incomplete => {
            assert!(
                matches!(error,EngineError::Transport(TransportError::ResponseIncomplete {ref response_id,reason}) if response_id == "native" && reason == if gemini {None} else {Some(IncompleteReason::ContentFilter)})
            );
            ProviderResponseState::Failed
        }
        End::Failed | End::Cancelled => {
            let expected = if !gemini {
                ProviderStreamFailureEvent::ResponseFailed
            } else if matches!(end, End::Cancelled) {
                ProviderStreamFailureEvent::InteractionCancelled
            } else {
                ProviderStreamFailureEvent::InteractionFailed
            };
            assert!(
                matches!(error,EngineError::Transport(TransportError::ProviderStreamFailure {event,diagnostic:None}) if event == expected)
            );
            ProviderResponseState::Failed
        }
        End::Malformed => {
            assert!(matches!(
                error,
                EngineError::Transport(
                    TransportError::Stream(_) | TransportError::MissingOutputIndex
                )
            ));
            ProviderResponseState::Failed
        }
        End::InvalidComplete => {
            assert!(matches!(
                error,
                EngineError::Transport(TransportError::IncompleteOutputIndices { index: 1 })
            ));
            ProviderResponseState::Failed
        }
    };
    let records = store.events(None).unwrap();
    let observed = records
        .iter()
        .filter(|event| event.kind == "responses_transport")
        .collect::<Vec<_>>();
    assert_eq!(observed.len(), 1);
    let request = observed[0].request.clone().unwrap();
    let observation: TransportObservation = serde_json::from_str(&observed[0].payload).unwrap();
    let expected_terminal = match end {
        End::Eof | End::ReadFailure | End::Malformed => None,
        End::Incomplete => Some(ResponseTerminal::Incomplete {
            response_id: "native".into(),
            reason: if gemini {
                None
            } else {
                Some(IncompleteReason::ContentFilter)
            },
        }),
        End::Failed | End::Cancelled if !gemini => Some(ResponseTerminal::ResponseFailed),
        End::Failed => Some(ResponseTerminal::InteractionFailed),
        End::Cancelled => Some(ResponseTerminal::InteractionCancelled),
        End::InvalidComplete => Some(ResponseTerminal::Completed),
    };
    assert_eq!(observation.terminal, expected_terminal);
    assert_eq!(
        observation.end,
        match end {
            End::Eof => ResponseEnd::CleanEof,
            End::ReadFailure => ResponseEnd::ReadFailed,
            End::Malformed => ResponseEnd::ProtocolError,
            _ => ResponseEnd::ProviderTerminal,
        }
    );
    assert!(
        !serde_json::to_string(&observation)
            .unwrap()
            .contains("offline-native-terminal-key")
    );
    let items = store.items(&request).unwrap();
    assert!(items.contains(&http.prefix));
    assert_eq!(items.iter().filter(|item| **item == http.prefix).count(), 1);
    assert!(store.claims_on(&request).unwrap().is_empty());
    assert_eq!(
        store.provider_response_state(&request).unwrap(),
        Some(state)
    );
    drop(engine);
    drop(store);
    let reopened = http.store();
    assert_eq!(reopened.items(&request).unwrap(), items);
    assert_eq!(
        reopened.provider_response_state(&request).unwrap(),
        Some(state)
    );
    assert!(reopened.claims_on(&request).unwrap().is_empty());
}

#[tokio::test]
async fn native_http_ordinary_and_compaction_stops_preserve_disposition_after_reopen() {
    for gemini in [false, true] {
        for compaction in [false, true] {
            for end in [
                End::Eof,
                End::ReadFailure,
                End::Incomplete,
                End::Failed,
                End::Cancelled,
                End::Malformed,
                End::InvalidComplete,
            ] {
                let mut http = NativeHttp::new(gemini, end).await;
                tokio::time::timeout(Duration::from_secs(10), async {
                    if gemini {
                        check(
                            &http,
                            GeminiAuth::from_admission(http.admission.clone()).unwrap(),
                            true,
                            compaction,
                            end,
                        )
                        .await;
                    } else {
                        check(
                            &http,
                            DeepSeekAuth::from_admission(http.admission.clone()).unwrap(),
                            false,
                            compaction,
                            end,
                        )
                        .await;
                    }
                })
                .await
                .unwrap();
                // Read failure can race the server's final close, but this task
                // must have actually accepted, authenticated and answered once.
                tokio::time::timeout(Duration::from_secs(2), &mut http.server)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
    }
}
