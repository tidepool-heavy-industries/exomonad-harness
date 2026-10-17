//! The actual HTTP/SSE reader drives Engine admission and a persistent Store.
use super::*;
use crate::{
    embedding::{
        ClientOperationId, HostIdentity, HostInferenceState, InferenceBlockReason,
        InputContinuation,
    },
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::Item,
    model::{AgentPath, ConversationIdentity, Effort, RequestId},
    provider::{Provider, ProviderError},
    server::{CommandReceiptOutcome, HostCommand},
    store::{PreparedProviderRequest, Store},
    turn::JobScheduler,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Notify, watch},
};

#[derive(Clone)]
struct NoAuth;
impl Auth for NoAuth {
    fn admission(
        &self,
    ) -> Result<crate::transport::NativeTransportAdmission, crate::transport::TransportError> {
        crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
            "/offline/synthetic-credential",
        ))
    }
    fn access(&self) -> Result<String, TransportError> {
        panic!("loopback never authenticates")
    }
}
#[derive(Default)]
struct Effects(AtomicUsize);
#[async_trait::async_trait]
impl Provider for Effects {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"ok":true}))
    }
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"type":"function","name":"effect","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
}
struct Reply {
    body: String,
    short_read: bool,
    buffered_tail: bool,
    barrier: Option<Arc<Notify>>,
}
impl Reply {
    fn eof(body: String) -> Self {
        Self {
            body,
            short_read: false,
            buffered_tail: false,
            barrier: None,
        }
    }
    fn complete() -> Self {
        Self::eof(format!(
            "{}{}",
            event(
                json!({"type":"response.output_item.done","output_index":0,"item":{"id":"final","type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}})
            ),
            event(json!({"type":"response.completed","response":{"id":"completed","output":[]}}))
        ))
    }
}
fn event(value: Value) -> String {
    format!("data: {value}\r\n\r\n")
}
fn partial() -> String {
    // Unknown events and CRLF/multiline data go through the production framer.
    format!(
        "data: {{\"type\":\"unknown.test\"}}\r\n\r\ndata: {{\"type\":\"response.output_text.delta\",\r\ndata: \"item_id\":\"partial\",\"delta\":\"unfinished\",\"content_index\":0}}\r\n\r\n{}",
        "data: {\"type\":\"response.output_item.done\",\"item\":"
    )
}
#[derive(Clone)]
struct HttpScript {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<(RequestId, Value)>>>,
    started: Arc<Notify>,
}
#[async_trait::async_trait]
impl ResponsesTransport for HttpScript {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push((
            request.request_id().clone(),
            serde_json::from_slice(request.wire_body()).unwrap(),
        ));
        self.started.notify_one();
        let Reply {
            body,
            short_read,
            buffered_tail,
            barrier,
        } = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected provider retry");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            if let Some(barrier) = barrier {
                barrier.notified().await;
            }
            let length = body.len() + usize::from(short_read) * 256;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\nX-Request-Id: loopback-request\r\n\r\n").as_bytes()).await.unwrap();
            for chunk in body.as_bytes().chunks(7) {
                if stream.write_all(chunk).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        let response = reqwest::get(format!("http://{address}")).await.unwrap();
        if buffered_tail {
            let (tx, mut rx) = tokio::sync::mpsc::channel(32);
            let result = read_response(response, Some(tx), "", "").await;
            // The real parser finishes before Engine sees any event. Forwarding
            // these bounded facts and returning failure complete in one poll.
            while let Ok(event) = rx.try_recv() {
                sink.send(event).await.unwrap();
            }
            result
        } else {
            read_response(response, Some(sink), "", "").await
        }
    }
}
type Runner = Engine<NoAuth, Effects, HttpScript>;
fn setup(
    store: Arc<Store>,
    replies: Vec<Reply>,
) -> (Runner, HostIdentity, HttpScript, Arc<Effects>) {
    let identity = HostIdentity {
        run: "continuation-http".into(),
        actor: AgentPath("/root".into()),
        incarnation: "owner".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let script = HttpScript {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(vec![])),
        started: Arc::new(Notify::new()),
    };
    let effects = Arc::new(Effects::default());
    let runner = Engine::with_transport(
        script.clone(),
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        effects.clone(),
        EngineConfig {
            instructions: "continuation tests".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "continuation-http".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    (runner, identity, script, effects)
}
fn mailbox() -> tokio::sync::mpsc::UnboundedReceiver<crate::mailbox::DurableMailboxWake> {
    tokio::sync::mpsc::unbounded_channel().1
}
fn input(
    store: &Store,
    identity: &HostIdentity,
    operation: ClientOperationId,
    text: &str,
) -> crate::server::CommandReceipt {
    let command = HostCommand::Input {
        target: identity.clone(),
        text: text.into(),
    };
    store.enqueue_embedded_command(operation, &command).unwrap();
    if let Some(existing) = store
        .embedded_command(&identity.run, operation)
        .unwrap()
        .and_then(|record| record.receipt)
    {
        return existing;
    }
    store
        .claim_embedded_command(&identity.run, operation)
        .unwrap()
        .unwrap();
    match store
        .admit_embedded_command_input(identity, operation, text)
        .unwrap()
    {
        crate::store::CommandInputAdmission::New(receipt)
        | crate::store::CommandInputAdmission::Retained(receipt) => receipt,
    }
}
fn block(store: &Store, identity: &HostIdentity) -> crate::embedding::InferenceBlock {
    let HostInferenceState::Blocked { block } = store.embedded_inference_state(identity).unwrap()
    else {
        panic!("expected blocked")
    };
    block
}

#[tokio::test]
async fn http_eof_and_read_failure_retry_twice_then_success_reopen_preserves_only_completed_input()
{
    for short_read in [false, true] {
        let path =
            std::env::temp_dir().join(format!("http-continuation-{}.sqlite", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&path).unwrap());
        let mut failed = Reply::eof(partial());
        failed.short_read = short_read;
        let (runner, identity, script, effects) = setup(
            store.clone(),
            vec![failed, Reply::eof(partial()), Reply::complete()],
        );
        let (_cancel, cancellation) = watch::channel(false);
        let started = std::time::Instant::now();
        let completed = runner
            .run_embedded(None, vec![Item::user_text("once")], cancellation, mailbox())
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_secs(4));
        assert_eq!(script.requests.lock().unwrap().len(), 3);
        assert_eq!(effects.0.load(Ordering::SeqCst), 0);
        let requests = script.requests.lock().unwrap();
        for (_, body) in requests.iter().skip(1) {
            assert!(
                !body.to_string().contains("unfinished"),
                "observational delta became provider input"
            );
            assert_eq!(
                body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|i| i["role"] == "user")
                    .count(),
                1
            );
        }
        let first = requests[0].0.clone();
        drop(requests);
        let summaries: Vec<_> = store
            .events(Some(&first))
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "transport_failure_summary")
            .collect();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&summaries[0].payload).unwrap()["end"],
            if short_read {
                "read_failed"
            } else {
                "clean_eof"
            }
        );
        drop(runner);
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Idle
        );
        assert!(reopened.request(&completed.head_request).unwrap().is_some());
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn http_retry_budget_survives_reopen_and_duplicate_send_does_not_reset_lineage() {
    let path = std::env::temp_dir().join(format!("http-budget-{}.sqlite", uuid::Uuid::new_v4()));
    let store = Arc::new(Store::open(&path).unwrap());
    let (runner, identity, script, _) = setup(
        store.clone(),
        (0..4).map(|_| Reply::eof(partial())).collect(),
    );
    let (_cancel, cancellation) = watch::channel(false);
    runner
        .run_embedded(None, vec![], cancellation.clone(), mailbox())
        .await
        .unwrap_err();
    assert_eq!(script.requests.lock().unwrap().len(), 3);
    drop(runner);
    drop(store);
    let store = Arc::new(Store::open(&path).unwrap());
    let (runner, _, script, _) = setup(store.clone(), vec![Reply::eof(partial())]);
    let operation = ClientOperationId(uuid::Uuid::new_v4());
    let receipt = input(&store, &identity, operation, "resume exactly once");
    assert!(matches!(
        receipt.outcome,
        CommandReceiptOutcome::Admitted {
            continuation: Some(InputContinuation::Granted { .. }),
            ..
        }
    ));
    assert_eq!(
        input(&store, &identity, operation, "resume exactly once"),
        receipt
    );
    runner
        .run_embedded(None, vec![], cancellation, mailbox())
        .await
        .unwrap_err();
    assert_eq!(
        script.requests.lock().unwrap().len(),
        1,
        "manual Send reset automatic budget"
    );
    assert_eq!(
        input(&store, &identity, operation, "resume exactly once"),
        receipt
    );
    assert!(matches!(
        store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Blocked { .. }
    ));
    assert!(store.unread("/root").unwrap().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn http_explicit_incomplete_schema_and_completed_tail_veto_automatic_retry() {
    let done = event(
        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"tail-call","type":"function_call","call_id":"effect-call","name":"effect","arguments":"{}"}}),
    );
    for body in [
        event(
            json!({"type":"response.incomplete","response":{"id":"incomplete-id","incomplete_details":{"reason":"max_output_tokens"}}}),
        ),
        "data: {invalid}\n\n".into(),
        done,
    ] {
        let store = Arc::new(Store::memory().unwrap());
        let explicit = body.contains("response.incomplete");
        let mut reply = Reply::eof(body);
        reply.buffered_tail = !explicit && reply.body.contains("tail-call");
        let (runner, identity, script, effects) = setup(store.clone(), vec![reply]);
        let (_cancel, cancellation) = watch::channel(false);
        runner
            .run_embedded(None, vec![], cancellation, mailbox())
            .await
            .unwrap_err();
        assert_eq!(script.requests.lock().unwrap().len(), 1);
        assert_eq!(
            effects.0.load(Ordering::SeqCst),
            0,
            "buffered completed tail must be retained without dispatch"
        );
        if explicit {
            assert!(
                matches!(block(&store,&identity).reason,InferenceBlockReason::ResponseIncomplete{response_id,..} if response_id=="incomplete-id")
            );
        }
    }
}

#[tokio::test]
async fn http_send_racing_failure_consumes_only_one_attempt_intent_without_effect_replay() {
    let barrier = Arc::new(Notify::new());
    let failed = Reply {
        body: event(
            json!({"type":"response.incomplete","response":{"id":"no-auto","incomplete_details":{"reason":"content_filter"}}}),
        ),
        short_read: false,
        buffered_tail: false,
        barrier: Some(barrier.clone()),
    };
    let store = Arc::new(Store::memory().unwrap());
    let (runner, identity, script, effects) = setup(
        store.clone(),
        vec![
            failed,
            Reply::eof(event(
                json!({"type":"response.incomplete","response":{"id":"again"}}),
            )),
        ],
    );
    let started = script.started.clone();
    let (_cancel, cancellation) = watch::channel(false);
    let running = tokio::spawn(async move {
        runner
            .run_embedded(None, vec![], cancellation, mailbox())
            .await
    });
    started.notified().await;
    let operation = ClientOperationId(uuid::Uuid::new_v4());
    let receipt = input(&store, &identity, operation, "racing input");
    assert!(matches!(
        receipt.outcome,
        CommandReceiptOutcome::Admitted {
            continuation: Some(InputContinuation::Queued),
            ..
        }
    ));
    assert_eq!(input(&store, &identity, operation, "racing input"), receipt);
    barrier.notify_one();
    running.await.unwrap().unwrap_err();
    assert_eq!(
        script.requests.lock().unwrap().len(),
        2,
        "deferred input intent was not consumed exactly once"
    );
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    assert_eq!(input(&store, &identity, operation, "racing input"), receipt);
    assert!(matches!(
        store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Blocked { .. }
    ));
}

#[tokio::test]
async fn http_stop_during_retry_delay_fences_reservation_before_reopen() {
    let path = std::env::temp_dir().join(format!("http-stop-{}.sqlite", uuid::Uuid::new_v4()));
    let store = Arc::new(Store::open(&path).unwrap());
    let (runner, identity, script, _) = setup(store.clone(), vec![Reply::eof(partial())]);
    let (cancel, cancellation) = watch::channel(false);
    let running = tokio::spawn(async move {
        runner
            .run_embedded(None, vec![], cancellation, mailbox())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                store.embedded_inference_state(&identity).unwrap(),
                HostInferenceState::AutoRetry { .. }
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.send(true).unwrap();
    running.await.unwrap().unwrap_err();
    assert_eq!(script.requests.lock().unwrap().len(), 1);
    let retained = block(&store, &identity);
    assert_eq!(retained.reason, InferenceBlockReason::HostInterrupted);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(block(&reopened, &identity), retained);
    std::fs::remove_file(path).unwrap();
}
