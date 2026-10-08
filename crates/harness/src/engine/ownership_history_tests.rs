use super::*;
use crate::{embedding::HostIdentity, provider::ProviderError, turn::JobOutput};
use std::sync::Mutex;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline history fixture")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Issuer {
    EngineWait,
    ProviderFunction,
    ProviderCustom,
    ProviderWait,
}

impl Issuer {
    fn invocation(self, call: &CallId) -> Item {
        let name = match self {
            Self::EngineWait => "yield",
            Self::ProviderFunction => "work",
            Self::ProviderCustom => "cell",
            Self::ProviderWait => "wait_agent",
        };
        if self == Self::ProviderCustom {
            Item(json!({"type":"custom_tool_call", "call_id":call.0,
                "name":name, "async":true, "input":"work"}))
        } else {
            Item(json!({"type":"function_call", "call_id":call.0,
                "name":name, "async":true, "arguments":"{}"}))
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Terminal {
    Success,
    WaitCancelled,
    Failure,
    Cancelled,
    CancellationReceipt,
    Unconfirmed,
}

impl Terminal {
    fn output(self) -> JobOutput {
        match self {
            // Payloads deliberately resemble a different owner. They are values,
            // never evidence of who issued the operation.
            Self::Success => JobOutput::Completed(Ok(json!({"name":"yield", "done":true}))),
            Self::WaitCancelled => {
                JobOutput::Completed(Ok(json!({"reason":"cancelled", "ready_results":[]})))
            }
            Self::Failure => JobOutput::Completed(Err("yield failed".into())),
            Self::Cancelled => JobOutput::Cancelled,
            Self::CancellationReceipt => JobOutput::CancelledWithReceipt(Ok(json!({"done":true}))),
            Self::Unconfirmed => JobOutput::CancellationUnconfirmed("still running".into()),
        }
    }

    fn completed(self) -> bool {
        matches!(self, Self::Success | Self::WaitCancelled | Self::Failure)
    }

    fn expected_terminal(self) -> crate::store::TerminalOutcome {
        use crate::store::TerminalOutcome;
        match self {
            Self::Success | Self::WaitCancelled => TerminalOutcome::Success,
            Self::Failure => TerminalOutcome::Failure("yield failed".into()),
            Self::Cancelled => TerminalOutcome::Cancelled,
            Self::CancellationReceipt => {
                TerminalOutcome::CancelledWithReceipt(Ok(json!({"done":true})))
            }
            Self::Unconfirmed => TerminalOutcome::CancellationUnconfirmed("still running".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notification {
    Commit,
    Abort,
}

#[derive(Clone, Copy, Debug)]
struct Boundary {
    issuer: Issuer,
    terminal: Terminal,
    unpublished_context: bool,
}

impl Boundary {
    fn notification(self) -> Option<Notification> {
        if self.issuer == Issuer::EngineWait || !self.terminal.completed() {
            None
        } else if self.unpublished_context {
            Some(Notification::Abort)
        } else {
            Some(Notification::Commit)
        }
    }
}

#[derive(Default)]
struct Host {
    issued: Mutex<HashSet<OperationId>>,
    notifications: Mutex<Vec<(OperationId, Notification)>>,
}

impl Host {
    fn observe(&self, operation: &OperationId, kind: Notification) -> Result<(), ProviderError> {
        if !self.issued.lock().unwrap().contains(operation) {
            return Err(ProviderError::Tool("no issued operation".into()));
        }
        self.notifications
            .lock()
            .unwrap()
            .push((operation.clone(), kind));
        Ok(())
    }
}

#[async_trait::async_trait]
impl Provider for Host {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        panic!("retained terminal must not execute provider work")
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        self.observe(operation, Notification::Commit)
    }
    async fn output_aborted(&self, operation: &OperationId) -> Result<(), ProviderError> {
        self.observe(operation, Notification::Abort)
    }
}

struct Finish;
#[async_trait::async_trait]
impl ResponsesTransport for Finish {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        Ok(ResponsesTurn {
            response_id: "finished".into(),
            items: vec![Item(json!({"type":"message", "role":"assistant",
                "phase":"final_answer", "content":"done"}))],
            usage: Usage::default(),
        })
    }
}

struct HistoryEvidence<'a> {
    boundaries: &'a [Boundary],
    published: &'a [bool],
}

impl Drop for HistoryEvidence<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "ownership history: {:?}; published: {:?}",
                self.boundaries, self.published
            );
        }
    }
}

fn identity() -> HostIdentity {
    HostIdentity {
        run: "ownership-history".into(),
        actor: AgentPath("/root".into()),
        incarnation: "first".into(),
    }
}

fn engine(
    store: Arc<Store>,
    host: Arc<Host>,
    identity: &HostIdentity,
) -> Engine<Offline, Host, Finish> {
    Engine::with_transport(
        Finish,
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        host,
        EngineConfig {
            instructions: "history".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: "history".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    })
}

// These histories start at a durably issued terminal, rather than simulating a
// provider execution. Store constructs every invocation, claim and output;
// only the independently retained issuance ledger authorizes host callbacks.
async fn history(boundaries: &[Boundary], published: &[bool]) -> Vec<(OperationId, Notification)> {
    let _evidence = HistoryEvidence {
        boundaries,
        published,
    };
    assert_eq!(boundaries.len(), published.len());
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    let host = Arc::new(Host::default());
    let runtime = engine(store.clone(), host.clone(), &identity);
    let foreign_host = Arc::new(Host::default());
    let foreign = Engine::<Offline, _, _>::with_transport(
        Finish,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        foreign_host.clone(),
        EngineConfig {
            agent: AgentPath("/foreign".into()),
            ..runtime.config.clone()
        },
    );
    let mut head = None;
    let mut model = Vec::new();
    let mut operations = Vec::new();
    let call = CallId("reused-call".into());
    for (index, (&boundary, &publish)) in boundaries.iter().zip(published).enumerate() {
        // A successor can reuse the call ID only after the preceding output
        // has entered history. Otherwise it would mask the old call's output
        // interval; the Engine publishes before admitting such a successor.
        assert!(index == 0 || published[index - 1]);
        let request = RequestId(format!("issued-{index}"));
        let mut invocation = boundary.issuer.invocation(&call);
        if boundary.unpublished_context {
            invocation.0["async"] = json!(false);
        }
        store
            .write_embedded_request(
                &identity,
                &request,
                head.as_ref(),
                std::slice::from_ref(&invocation),
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&request, Effort::Low).unwrap();
        let operation = store.claim(&call, &request).unwrap();
        assert_eq!(operation.origin, runtime.origin);
        assert_eq!(
            store.invocation_item(&request, &call).unwrap(),
            invocation.tool_call().unwrap()
        );
        let kind = invocation.tool_call().unwrap().unwrap().input.kind();
        assert_eq!(
            store.claims_for_operation(&operation).unwrap()[0].state,
            crate::store::ClaimState::Pending
        );
        if boundary.issuer != Issuer::EngineWait {
            assert!(host.issued.lock().unwrap().insert(operation.clone()));
        }
        if boundary.unpublished_context {
            assert_ne!(boundary.issuer, Issuer::EngineWait);
            store
                .record_event(
                    Some(&request),
                    "context_disposition",
                    &serde_json::to_value(ContextDispositionRecord {
                        operation: operation.clone(),
                        requires_context_commit: true,
                    })
                    .unwrap(),
                )
                .unwrap();
        }
        assert!(store.context_receipt(&operation).unwrap().is_none());
        let output = boundary.terminal.output();
        if publish {
            runtime
                .persist_output(&operation, kind, &output, &request, &request)
                .await
                .unwrap();
        } else {
            runtime
                .retain_settled_output(&operation, kind, &output, &request)
                .await
                .unwrap();
        }
        if let Some(notification) = boundary.notification() {
            model.push((operation.clone(), notification));
        }
        assert_eq!(*host.notifications.lock().unwrap(), model);
        assert_eq!(
            store.has_completed_output(&operation).unwrap(),
            boundary.terminal.completed()
        );
        let retained = store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .unwrap();
        assert_eq!(retained.terminal, boundary.terminal.expected_terminal());
        assert_eq!(retained.item, Item::tool_output(&call, kind, &output));
        let before = store.items(&request).unwrap();
        foreign
            .retain_settled_output(&operation, kind, &output, &request)
            .await
            .unwrap();
        assert!(foreign_host.notifications.lock().unwrap().is_empty());
        assert_eq!(store.items(&request).unwrap(), before);
        assert!(
            store
                .settle_embedded_round(
                    &identity,
                    head.as_ref(),
                    &request,
                    crate::store::EmbeddedRoundOutcome::Completed
                )
                .unwrap()
        );
        operations.push((operation, retained, before));
        head = Some(request);
    }
    // Fresh schedulers contain no jobs. Both the retained-only and already
    // published branches must recover without redispatching or duplicating output.
    for _ in 0..2 {
        let fresh = engine(store.clone(), host.clone(), &identity);
        let (_cancel, cancellation) = watch::channel(false);
        let (_mail, mailbox) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
        let result = fresh
            .run_recovering_embedded(head.clone(), vec![], cancellation, mailbox)
            .await
            .unwrap();
        for (&boundary, (operation, retained, before)) in boundaries.iter().zip(&operations) {
            if let Some(notification) = boundary.notification() {
                model.push((operation.clone(), notification));
            }
            assert_eq!(
                store
                    .replay_tool_output_operation(operation)
                    .unwrap()
                    .unwrap(),
                *retained
            );
            assert_eq!(store.items(&operation.request).unwrap(), *before);
        }
        assert_eq!(*host.notifications.lock().unwrap(), model);
        assert_eq!(
            result
                .transcript
                .iter()
                .filter(|item| matches!(
                    item.0["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                ))
                .count(),
            boundaries.len()
        );
        assert!(
            store
                .settle_embedded_round(
                    &identity,
                    head.as_ref(),
                    &result.head_request,
                    crate::store::EmbeddedRoundOutcome::Completed
                )
                .unwrap()
        );
        head = Some(result.head_request);
    }
    model
}

#[tokio::test]
async fn persisted_ownership_histories_preserve_terminal_and_context_notifications() {
    let mut histories = 0;
    let mut commit_histories = 0;
    let mut abort_histories = 0;
    let mut silent_histories = 0;
    for issuer in [
        Issuer::EngineWait,
        Issuer::ProviderFunction,
        Issuer::ProviderCustom,
        Issuer::ProviderWait,
    ] {
        for terminal in [
            Terminal::Success,
            Terminal::WaitCancelled,
            Terminal::Failure,
            Terminal::Cancelled,
            Terminal::CancellationReceipt,
            Terminal::Unconfirmed,
        ] {
            // A cancelled yield wake is still a Completed wait result. Missing
            // wait jobs use claim interruption, tested through cancel_pending.
            if match issuer {
                Issuer::EngineWait => {
                    !matches!(terminal, Terminal::Success | Terminal::WaitCancelled)
                }
                Issuer::ProviderWait => !matches!(terminal, Terminal::Success),
                Issuer::ProviderFunction | Issuer::ProviderCustom => {
                    matches!(terminal, Terminal::WaitCancelled)
                }
            } {
                continue;
            }
            for unpublished_context in [false, true] {
                if unpublished_context
                    && !matches!(issuer, Issuer::ProviderFunction | Issuer::ProviderCustom)
                {
                    continue;
                }
                let boundary = Boundary {
                    issuer,
                    terminal,
                    unpublished_context,
                };
                for published in [false, true] {
                    let observed = history(&[boundary], &[published]).await;
                    histories += 1;
                    match boundary.notification() {
                        Some(Notification::Commit) => {
                            commit_histories += 1;
                            assert_eq!(observed.len(), 3);
                        }
                        Some(Notification::Abort) => {
                            abort_histories += 1;
                            assert_eq!(observed.len(), 3);
                        }
                        None => {
                            silent_histories += 1;
                            assert!(observed.is_empty());
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        (
            histories,
            commit_histories,
            abort_histories,
            silent_histories
        ),
        (46, 10, 8, 28)
    );
    eprintln!(
        "ownership histories={histories}, commit={commit_histories}, abort={abort_histories}, silent={silent_histories}"
    );
}

#[tokio::test]
async fn mixed_ownership_recovery_uses_original_operation_with_reused_call_id() {
    let mut histories = 0;
    for provider in [
        Issuer::ProviderFunction,
        Issuer::ProviderCustom,
        Issuer::ProviderWait,
    ] {
        for reverse in [false, true] {
            let mut boundaries = [
                Boundary {
                    issuer: Issuer::EngineWait,
                    terminal: Terminal::Success,
                    unpublished_context: false,
                },
                Boundary {
                    issuer: provider,
                    terminal: Terminal::Success,
                    unpublished_context: false,
                },
            ];
            if reverse {
                boundaries.reverse();
            }
            for second_published in [false, true] {
                let observed = history(&boundaries, &[true, second_published]).await;
                assert_eq!(observed.len(), 3);
                assert!(
                    observed
                        .iter()
                        .all(|(_, kind)| *kind == Notification::Commit)
                );
                assert!(
                    observed
                        .iter()
                        .all(|(operation, _)| operation == &observed[0].0)
                );
                histories += 1;
            }
        }
    }
    assert_eq!(histories, 12);
    eprintln!("mixed ownership histories={histories}");
}

#[tokio::test]
async fn pending_wait_cancellation_recovers_interruption_without_completion_notification() {
    for issuer in [Issuer::EngineWait, Issuer::ProviderWait] {
        let store = Arc::new(Store::memory().unwrap());
        let identity = identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        let host = Arc::new(Host::default());
        let runtime = engine(store.clone(), host.clone(), &identity);
        let request = RequestId("interrupted-wait".into());
        let call = CallId("wait".into());
        let invocation = issuer.invocation(&call);
        store
            .write_embedded_request(
                &identity,
                &request,
                None,
                std::slice::from_ref(&invocation),
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&request, Effort::Low).unwrap();
        let DispatchResult::Pending(pending) = runtime
            .dispatch_completed_item(invocation, &request)
            .await
            .unwrap()
        else {
            panic!("unsettled wait must retain its pending claim");
        };
        assert_eq!(pending.operation.origin, runtime.origin);
        assert!(pending.wait.is_some());
        assert_eq!(
            store.claims_for_operation(&pending.operation).unwrap()[0].state,
            crate::store::ClaimState::Pending
        );
        runtime
            .cancel_pending(std::slice::from_ref(&pending))
            .await
            .unwrap();
        assert_eq!(
            store.claims_for_operation(&pending.operation).unwrap()[0].state,
            crate::store::ClaimState::Interrupted
        );
        assert!(
            store
                .settle_embedded_round(
                    &identity,
                    None,
                    &request,
                    crate::store::EmbeddedRoundOutcome::Cancelled
                )
                .unwrap()
        );
        let expected = Item::tool_output(&call, ToolKind::Function, &JobOutput::Interrupted);
        let mut head = Some(request.clone());
        for _ in 0..2 {
            let fresh = engine(store.clone(), host.clone(), &identity);
            let (_cancel, cancellation) = watch::channel(false);
            let (_mail, mailbox) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
            let result = fresh
                .run_recovering_embedded(head.clone(), vec![], cancellation, mailbox)
                .await
                .unwrap();
            assert_eq!(
                result
                    .transcript
                    .iter()
                    .filter(|item| **item == expected)
                    .count(),
                1
            );
            assert!(host.notifications.lock().unwrap().is_empty());
            assert!(!store.has_completed_output(&pending.operation).unwrap());
            assert_eq!(store.items(&request).unwrap().len(), 1);
            assert!(
                store
                    .settle_embedded_round(
                        &identity,
                        head.as_ref(),
                        &result.head_request,
                        crate::store::EmbeddedRoundOutcome::Completed
                    )
                    .unwrap()
            );
            head = Some(result.head_request);
        }
    }
    eprintln!("pending wait cancellation histories=2");
}
