use super::*;
use crate::provider::{CallContext, ProviderError};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline fixture")
    }
}

#[derive(Default)]
struct Signals {
    first_entered: Notify,
    successor_held: Notify,
    release_first: Notify,
    first_retained: Notify,
    second_retained: Notify,
}

struct Host {
    store: Arc<Store>,
    signals: Arc<Signals>,
    issued: Mutex<Vec<OperationId>>,
    shared_output: bool,
}

#[async_trait::async_trait]
impl Provider for Host {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        panic!("the scheduler must supply the exact issuing operation")
    }
    async fn call_with_context(
        &self,
        _: &str,
        _: serde_json::Value,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        let operation = context.operation.expect("scheduler issuance");
        let index = {
            let mut issued = self.issued.lock().unwrap();
            let index = issued.len();
            issued.push(operation.clone());
            index
        };
        if index == 0 {
            self.signals.first_entered.notify_one();
            self.signals.release_first.notified().await;
        } else {
            assert_eq!(index, 1, "recovery must not redispatch either operation");
        }
        Ok(if self.shared_output {
            json!("same result")
        } else {
            serde_json::to_value(operation).unwrap()
        })
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        assert!(self.store.has_completed_output(operation).unwrap());
        let index = self
            .issued
            .lock()
            .unwrap()
            .iter()
            .position(|issued| issued == operation)
            .expect("callback must select the exact issued operation");
        if index == 0 {
            self.signals.first_retained.notify_one();
        } else {
            assert_eq!(index, 1);
            self.signals.second_retained.notify_one();
        }
        Ok(())
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
}

struct Script {
    turns: AtomicUsize,
    signals: Arc<Signals>,
    names: [&'static str; 2],
}

fn invocation(name: &str) -> Item {
    Item(
        json!({"type":"function_call", "call_id":"reused", "name":name, "async":true, "arguments":"{}"}),
    )
}

#[async_trait::async_trait]
impl ResponsesTransport for Script {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let index = self.turns.fetch_add(1, Ordering::SeqCst);
        let items = match index {
            0 => vec![invocation(self.names[0])],
            1 => {
                self.signals.successor_held.notify_one();
                self.signals.first_retained.notified().await;
                vec![invocation(self.names[1])]
            }
            index => {
                if index == 2 {
                    self.signals.second_retained.notified().await;
                }
                vec![Item(
                    json!({"type":"message", "role":"assistant", "phase":"final_answer", "content":"done"}),
                )]
            }
        };
        Ok(ResponsesTurn {
            response_id: format!("turn-{index}"),
            items,
            usage: Usage::default(),
        })
    }
}

fn engine(
    store: Arc<Store>,
    host: Arc<Host>,
    signals: Arc<Signals>,
    names: [&'static str; 2],
) -> Engine<Offline, Host, Script> {
    Engine::with_transport(
        Script {
            turns: AtomicUsize::new(0),
            signals,
            names,
        },
        store,
        Arc::new(JobScheduler::new(2).unwrap()),
        host,
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: "reused-call".into(),
            agent: AgentPath("/root".into()),
        },
    )
}

struct Finish;
#[async_trait::async_trait]
impl ResponsesTransport for Finish {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        Ok(ResponsesTurn {
            response_id: "recovered".into(),
            usage: Usage::default(),
            items: vec![Item(
                json!({"type":"message", "role":"assistant", "phase":"final_answer", "content":"done"}),
            )],
        })
    }
}

async fn run_reused_history(shared_output: bool, names: [&'static str; 2], here_copy: bool) {
    let store = Arc::new(Store::memory().unwrap());
    let signals = Arc::new(Signals::default());
    let host = Arc::new(Host {
        store: store.clone(),
        signals: signals.clone(),
        issued: Mutex::new(vec![]),
        shared_output,
    });
    let runtime = Arc::new(engine(store.clone(), host.clone(), signals.clone(), names));
    let (_cancel, cancellation) = watch::channel(false);
    let running = runtime.clone();
    let first_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        running
            .run(
                None,
                vec![],
                first_cancellation,
                tokio::sync::mpsc::unbounded_channel::<Envelope>().1,
            )
            .await
    });
    let deadline = std::time::Duration::from_secs(5);
    tokio::time::timeout(deadline, signals.successor_held.notified())
        .await
        .unwrap();
    tokio::time::timeout(deadline, signals.first_entered.notified())
        .await
        .unwrap();
    let first = host.issued.lock().unwrap()[0].clone();
    assert_eq!(
        store.claims_for_operation(&first).unwrap()[0].state,
        crate::store::ClaimState::Pending
    );
    signals.release_first.notify_one();
    let completed = tokio::time::timeout(deadline, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let issued = host.issued.lock().unwrap().clone();
    assert_eq!(issued.len(), 2);
    assert_eq!(issued[0].call, issued[1].call);
    assert_eq!(issued[0].origin, issued[1].origin);
    assert_ne!(issued[0].request, issued[1].request);
    let outputs = issued
        .iter()
        .zip(names)
        .map(|(operation, name)| {
            assert_eq!(
                store
                    .invocation_item(&operation.request, &operation.call)
                    .unwrap(),
                invocation(name).tool_call().unwrap()
            );
            store
                .replay_tool_output_operation(operation)
                .unwrap()
                .unwrap()
                .item
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs[0] == outputs[1], shared_output);
    for output in &outputs {
        assert_eq!(
            completed
                .transcript
                .iter()
                .filter(|item| *item == output)
                .count(),
            if shared_output { 2 } else { 1 }
        );
    }
    let records = store.recovery_history(&completed.head_request).unwrap();
    let call_positions = records
        .iter()
        .enumerate()
        .filter_map(|(position, record)| {
            record
                .item
                .tool_call()
                .unwrap()
                .is_some_and(|call| call.call_id.0 == "reused")
                .then_some(position)
        })
        .collect::<Vec<_>>();
    assert_eq!(call_positions.len(), 2);
    let older_output_position = records
        .iter()
        .position(|record| {
            record
                .output
                .as_ref()
                .is_some_and(|output| output.operation == issued[0])
        })
        .unwrap();
    assert!(
        older_output_position > call_positions[1],
        "older result must arrive after the reused call"
    );
    let mut head = completed.head_request;
    let mut config = runtime.config.clone();
    if here_copy {
        let parent = config.agent.clone();
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(&parent, None, Some(&head), &json!({}), &json!({}))
            .unwrap();
        let copy = RequestId("copied".into());
        store
            .admit_here_agent_with_snapshot(
                &child,
                &parent,
                &copy,
                &json!({}),
                "/root",
                "/root/child",
                "AtBoundary",
                &Item(json!({"type":"message","role":"user","content":"continue"})),
            )
            .unwrap();
        config.agent = child;
        head = copy;
    }
    for _ in 0..2 {
        let recovered = Engine::<Offline, _, _>::with_transport(
            Finish,
            store.clone(),
            Arc::new(JobScheduler::new(2).unwrap()),
            host.clone(),
            config.clone(),
        )
        .run_recovering(
            Some(head),
            vec![],
            cancellation.clone(),
            tokio::sync::mpsc::unbounded_channel::<Envelope>().1,
        )
        .await
        .unwrap();
        let retained = store.recovery_history(&recovered.head_request).unwrap();
        for (operation, output) in issued.iter().zip(&outputs) {
            let owned = retained
                .iter()
                .filter_map(|record| record.output.as_ref())
                .filter(|publication| &publication.operation == operation)
                .collect::<Vec<_>>();
            assert_eq!(owned.len(), 1);
            assert_eq!(owned[0].item, *output);
            assert_eq!(
                store
                    .replay_tool_output_operation(operation)
                    .unwrap()
                    .unwrap()
                    .item,
                *output
            );
            assert_eq!(
                recovered
                    .transcript
                    .iter()
                    .filter(|item| *item == output)
                    .count(),
                if shared_output { 2 } else { 1 }
            );
        }
        assert_eq!(*host.issued.lock().unwrap(), issued);
        head = recovered.head_request;
    }
}

#[tokio::test]
async fn async_settlement_after_successor_reuses_call_id_recovers_each_operation_once() {
    for here_copy in [false, true] {
        run_reused_history(false, ["work", "work"], here_copy).await;
    }
    eprintln!("distinct-output async recovery: histories=2 Here_partitions=2 fresh_recoveries=4");
}

#[tokio::test]
async fn equal_async_outputs_recover_exact_operations_through_here_copy() {
    for names in [["work", "work"], ["work", "other_work"]] {
        run_reused_history(true, names, true).await;
    }
    eprintln!(
        "equal-output async recovery: histories=2 tool_name_partitions=2 Here_copies=2 fresh_recoveries=4"
    );
}
