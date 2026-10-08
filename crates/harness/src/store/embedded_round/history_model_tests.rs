use super::*;
use crate::{
    embedding::{BindingSuccessorAuthority, BindingSuccessorCommit},
    transport::StreamInterruption,
};

#[derive(Clone, Copy, Debug)]
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
}

// The oracle retains accepted facts, rather than traversing Store's branch
// rows or borrowing its binding/frontier helpers to decide an operation.
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
        assert_eq!(
            store
                .events(None)
                .unwrap()
                .iter()
                .filter(|event| event.kind == "model_interrupted")
                .count(),
            self.interruptions,
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
                let result = store.write_embedded_request(
                    &issuer,
                    &request,
                    parent.as_ref(),
                    &[],
                    Usage::default(),
                );
                assert_eq!(result.is_ok(), expected, "{trace:?}: {result:?}");
                if expected {
                    self.issued.push(Issued {
                        request,
                        parent,
                        issuer,
                        interruption: None,
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
    let operations = [
        Admit,
        AdmitWrongParent,
        AdmitStaleIssuer,
        Settle(Completed),
        Settle(Cancelled),
        Settle(Rejected),
        SettleWrongExpected,
        SettleOlderPending,
        Interrupt,
        InterruptWrongExpected,
        Transfer,
        RefuseTransfer,
        RetryTransfer,
    ];
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
    let mut histories = 0;
    let mut accepted = 0;
    let mut refused = 0;
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
                    if model.step(&store, op, step, trace) {
                        accepted += 1;
                    } else {
                        refused += 1;
                    }
                    model.observe(&store, trace);
                }
                histories += 1;
            }
        }
    }
    assert_eq!(histories, 8 * operations.len() * operations.len());
    assert!(accepted > histories && refused > histories);
    eprintln!(
        "frontier histories={histories} accepted_operations={accepted} refused_operations={refused}"
    );
}
