use super::*;
use crate::{
    embedding::{BindingSuccessorAuthority, BindingSuccessorCommit},
    transport::StreamInterruption,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Admit,
    AdmitWrongParent,
    AdmitStaleIssuer,
    Settle(EmbeddedRoundOutcome),
    SettleWrongExpected,
    SettleOlderPending,
    Interrupt,
    InterruptWrongExpected,
    Transfer,
    RefuseTransfer,
    RetryTransfer,
}

impl Op {
    fn partition(self) -> usize {
        OPERATIONS.iter().position(|op| *op == self).unwrap()
    }
}

const OPERATIONS: [Op; 13] = [
    Op::Admit,
    Op::AdmitWrongParent,
    Op::AdmitStaleIssuer,
    Op::Settle(EmbeddedRoundOutcome::Completed),
    Op::Settle(EmbeddedRoundOutcome::Cancelled),
    Op::Settle(EmbeddedRoundOutcome::Rejected),
    Op::SettleWrongExpected,
    Op::SettleOlderPending,
    Op::Interrupt,
    Op::InterruptWrongExpected,
    Op::Transfer,
    Op::RefuseTransfer,
    Op::RetryTransfer,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Pending,
    Completed,
    Cancelled,
    Rejected,
}

impl Phase {
    fn persisted(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Coverage {
    accepted: usize,
    refused: usize,
    refused_while_pending: usize,
}

#[derive(Default)]
struct CampaignCoverage {
    partitions: [Coverage; 13],
    completed_histories: usize,
    refused_live_ancestor: usize,
    admitted_after_interruption: usize,
}

impl Drop for CampaignCoverage {
    fn drop(&mut self) {
        // Preserve reached partitions when a production/model comparison panics.
        for op in OPERATIONS {
            eprintln!("{op:?}: {:?}", self.partitions[op.partition()]);
        }
        eprintln!("completed_frontier_histories={}", self.completed_histories);
        eprintln!(
            "refused_live_ancestor={} admitted_after_interruption={}",
            self.refused_live_ancestor, self.admitted_after_interruption
        );
    }
}

struct Authority(bool);
impl BindingSuccessorAuthority for Authority {
    fn validate_successor(
        &self,
        _: &HostIdentity,
        _: &HostIdentity,
    ) -> std::result::Result<bool, String> {
        Ok(self.0)
    }
}

struct Issued {
    request: RequestId,
    parent: Option<RequestId>,
    issuer: HostIdentity,
    interruption: Option<StreamInterruption>,
    phase: Phase,
    input: Item,
}

// The oracle retains accepted facts, rather than traversing Store's branch
// rows or borrowing its binding/frontier helpers to decide an operation.
// This exercises Store transitions, not host recovery policy. The native
// embedding driver owns the fresh-input gate after a provider interruption;
// every admission here supplies a new explicit user input.
struct Model {
    current: HostIdentity,
    issued: Vec<Issued>,
    settled: Option<RequestId>,
    transfer: Option<(HostIdentity, HostIdentity)>,
    interruptions: usize,
}

impl Model {
    fn tip(&self) -> Option<&RequestId> {
        self.issued.last().map(|issued| &issued.request)
    }

    fn pending(&self) -> Option<&RequestId> {
        self.tip().filter(|tip| Some(*tip) != self.settled.as_ref())
    }

    fn stale_issuer(&self) -> HostIdentity {
        self.transfer
            .as_ref()
            .map(|(old, _)| old.clone())
            .unwrap_or_else(|| HostIdentity {
                incarnation: "unissued".into(),
                ..self.current.clone()
            })
    }

    fn observe(&self, store: &Store, trace: &[Op]) {
        let pending = self.pending().cloned();
        assert_eq!(
            store.embedded_round_frontier(&self.current).unwrap(),
            EmbeddedRoundFrontier {
                settled_head: self.settled.clone(),
                pending_head: pending.clone(),
                pending_interruption: pending.and_then(|_| self
                    .issued
                    .last()
                    .unwrap()
                    .interruption),
            },
            "{trace:?}"
        );
        for issued in &self.issued {
            let persisted_phase: String = store
                .lock()
                .query_row(
                    "SELECT round_phase FROM requests WHERE id=?1",
                    [&issued.request.0],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(persisted_phase, issued.phase.persisted(), "{trace:?}");
            assert_eq!(
                store.items(&issued.request).unwrap(),
                vec![issued.input.clone()],
                "{trace:?}"
            );
            assert_eq!(
                store.request(&issued.request).unwrap().unwrap().parent,
                issued.parent,
                "{trace:?}"
            );
            assert_eq!(
                store.request_output_origin(&issued.request).unwrap(),
                ConversationIdentity::Embedded {
                    run: issued.issuer.run.clone(),
                    actor: issued.issuer.actor.clone(),
                    incarnation: issued.issuer.incarnation.clone(),
                },
                "{trace:?}"
            );
        }
        let events = store.events(None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "model_interrupted")
                .count(),
            self.interruptions,
            "{trace:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "chat_message")
                .count(),
            self.issued.len(),
            "{trace:?}"
        );
        assert!(
            matches!(
                store.embedded_round_frontier(&self.stale_issuer()),
                Err(StoreError::InvalidEmbeddedBinding)
            ),
            "{trace:?}"
        );
    }

    fn step(&mut self, store: &Store, op: Op, step: usize, trace: &[Op]) -> bool {
        let absent = RequestId("never-admitted".into());
        match op {
            Op::Admit | Op::AdmitWrongParent | Op::AdmitStaleIssuer => {
                let request = RequestId(format!("request-{step}"));
                let parent = if matches!(op, Op::AdmitWrongParent) {
                    Some(absent)
                } else {
                    self.tip().cloned()
                };
                let issuer = if matches!(op, Op::AdmitStaleIssuer) {
                    self.stale_issuer()
                } else {
                    self.current.clone()
                };
                let expected = matches!(op, Op::Admit);
                let input = Item(serde_json::json!({
                    "type": "message", "role": "user",
                    "content": format!("explicit input {step}"),
                }));
                let result = store.write_embedded_request(
                    &issuer,
                    &request,
                    parent.as_ref(),
                    std::slice::from_ref(&input),
                    Usage::default(),
                );
                assert_eq!(result.is_ok(), expected, "{trace:?}: {result:?}");
                if expected {
                    self.issued.push(Issued {
                        request,
                        parent,
                        issuer,
                        interruption: None,
                        phase: Phase::Pending,
                        input,
                    });
                } else {
                    assert!(store.request(&request).unwrap().is_none(), "{trace:?}");
                }
                expected
            }
            Op::Settle(_) | Op::SettleWrongExpected | Op::SettleOlderPending => {
                let request = if matches!(op, Op::SettleOlderPending) {
                    self.issued
                        .iter()
                        .rev()
                        .nth(1)
                        .map(|issued| issued.request.clone())
                        .unwrap_or(absent.clone())
                } else {
                    self.pending().cloned().unwrap_or(absent.clone())
                };
                let expected_head = if matches!(op, Op::SettleWrongExpected) {
                    Some(absent)
                } else {
                    self.settled.clone()
                };
                let expected = matches!(op, Op::Settle(_)) && self.pending().is_some();
                let outcome = match op {
                    Op::Settle(outcome) => outcome,
                    _ => EmbeddedRoundOutcome::Completed,
                };
                assert_eq!(
                    store
                        .settle_embedded_round(
                            &self.current,
                            expected_head.as_ref(),
                            &request,
                            outcome
                        )
                        .unwrap(),
                    expected,
                    "{trace:?}"
                );
                if expected {
                    self.settled = Some(request);
                    self.issued.last_mut().unwrap().phase = match outcome {
                        EmbeddedRoundOutcome::Completed => Phase::Completed,
                        EmbeddedRoundOutcome::Cancelled => Phase::Cancelled,
                        EmbeddedRoundOutcome::Rejected => Phase::Rejected,
                    };
                }
                expected
            }
            Op::Interrupt | Op::InterruptWrongExpected => {
                let request = self.pending().cloned().unwrap_or(absent.clone());
                let expected_head = if matches!(op, Op::InterruptWrongExpected) {
                    Some(absent)
                } else {
                    self.settled.clone()
                };
                let expected = matches!(op, Op::Interrupt) && self.pending().is_some();
                let cause = if step % 2 == 0 {
                    StreamInterruption::MissingCompletion
                } else {
                    StreamInterruption::ReadFailed
                };
                assert_eq!(
                    store
                        .record_interrupted_model_request(
                            &request,
                            cause,
                            Some((&self.current, expected_head.as_ref()))
                        )
                        .unwrap(),
                    expected,
                    "{trace:?}"
                );
                if expected {
                    self.issued.last_mut().unwrap().interruption = Some(cause);
                    self.interruptions += 1;
                }
                expected
            }
            Op::Transfer | Op::RefuseTransfer => {
                let successor = HostIdentity {
                    incarnation: format!("issuer-{step}"),
                    ..self.current.clone()
                };
                let expected = matches!(op, Op::Transfer);
                let result = store.transfer_embedded_binding(
                    &self.current,
                    &successor,
                    &Authority(expected),
                );
                assert_eq!(result.is_ok(), expected, "{trace:?}: {result:?}");
                if expected {
                    assert!(matches!(result.unwrap(), BindingSuccessorCommit::Installed));
                    self.transfer = Some((self.current.clone(), successor.clone()));
                    self.current = successor;
                }
                expected
            }
            Op::RetryTransfer => {
                let (predecessor, successor) = self.transfer.clone().unwrap_or_else(|| {
                    (
                        self.stale_issuer(),
                        HostIdentity {
                            incarnation: "not-installed".into(),
                            ..self.current.clone()
                        },
                    )
                });
                let expected = self.transfer.is_some();
                let result =
                    store.transfer_embedded_binding(&predecessor, &successor, &Authority(true));
                assert_eq!(result.is_ok(), expected, "{trace:?}: {result:?}");
                if expected {
                    assert!(matches!(
                        result.unwrap(),
                        BindingSuccessorCommit::AlreadyInstalled
                    ));
                }
                expected
            }
        }
    }
}

#[test]
fn bounded_frontier_histories_preserve_exact_cas_and_request_issuers() {
    use EmbeddedRoundOutcome::*;
    use Op::*;
    let operations = OPERATIONS;
    let prefixes = [
        vec![],
        vec![Admit],
        vec![Admit, Admit],
        vec![Admit, Settle(Completed)],
        vec![Admit, Interrupt],
        vec![Admit, Transfer],
        vec![Admit, Admit, Transfer, Interrupt],
        vec![Admit, Settle(Rejected), Transfer, Admit],
    ];
    let mut coverage = CampaignCoverage::default();
    for prefix in prefixes {
        for left in operations {
            for right in operations {
                let store = Store::memory().unwrap();
                let current = HostIdentity {
                    run: "history-run".into(),
                    actor: AgentPath("/root".into()),
                    incarnation: "original".into(),
                };
                store.bind_embedded_actor(&current, None).unwrap();
                let mut model = Model {
                    current,
                    issued: vec![],
                    settled: None,
                    transfer: None,
                    interruptions: 0,
                };
                let history = prefix
                    .iter()
                    .copied()
                    .chain([left, right])
                    .collect::<Vec<_>>();
                model.observe(&store, &[]);
                for (step, op) in history.iter().copied().enumerate() {
                    let trace = &history[..=step];
                    let was_pending = model.pending().is_some();
                    let was_interrupted =
                        was_pending && model.issued.last().unwrap().interruption.is_some();
                    let older_pending = was_pending
                        && model
                            .issued
                            .iter()
                            .rev()
                            .nth(1)
                            .is_some_and(|issued| issued.phase == Phase::Pending);
                    if model.step(&store, op, step, trace) {
                        coverage.partitions[op.partition()].accepted += 1;
                        if matches!(op, Admit) && was_interrupted {
                            coverage.admitted_after_interruption += 1;
                        }
                    } else {
                        coverage.partitions[op.partition()].refused += 1;
                        if was_pending {
                            coverage.partitions[op.partition()].refused_while_pending += 1;
                        }
                        if matches!(op, SettleOlderPending) && older_pending {
                            coverage.refused_live_ancestor += 1;
                        }
                    }
                    model.observe(&store, trace);
                }
                coverage.completed_histories += 1;
            }
        }
    }
    assert_eq!(
        coverage.completed_histories,
        8 * operations.len() * operations.len()
    );
    assert!(coverage.refused_live_ancestor > 0);
    assert!(coverage.admitted_after_interruption > 0);
    assert!(coverage.partitions[SettleWrongExpected.partition()].refused_while_pending > 0);
    assert!(coverage.partitions[InterruptWrongExpected.partition()].refused_while_pending > 0);
    for op in operations {
        let observed = coverage.partitions[op.partition()];
        match op {
            Admit | Transfer => {
                assert!(
                    observed.accepted > 0,
                    "missing acceptance support for {op:?}"
                );
                assert_eq!(observed.refused, 0, "{op:?}");
            }
            AdmitWrongParent
            | AdmitStaleIssuer
            | SettleWrongExpected
            | SettleOlderPending
            | InterruptWrongExpected
            | RefuseTransfer => {
                assert_eq!(observed.accepted, 0, "{op:?}");
                assert!(observed.refused > 0, "missing refusal support for {op:?}");
            }
            Settle(_) | Interrupt | RetryTransfer => {
                assert!(
                    observed.accepted > 0,
                    "missing acceptance support for {op:?}"
                );
                assert!(observed.refused > 0, "missing refusal support for {op:?}");
            }
        }
    }
}
