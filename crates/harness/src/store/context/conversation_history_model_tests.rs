//! Frozen conversation cuts against an independent accepted-fact history.
use super::*;
use crate::{
    checkpoint::{CheckpointChild, CheckpointCuts},
    model::{CallId, Effort},
    store::provider_response::ProviderResponseState,
    transport::{
        ResponsesProtocol, ResponsesRequest, ResponsesRoute, ResponsesTurn,
        sse::{ResponseAssembly, StreamEvent},
    },
};
use proptest::prelude::*;
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
enum Op {
    Done(u8, u8),
    Complete(u8),
    Eof,
    Capture,
    Project(usize, u8),
    Reopen,
}

struct Frozen {
    cuts: CheckpointCuts<()>,
    entries: Vec<(u64, i64, u8)>,
    valid: bool,
    epoch: usize,
    observed_at_capture: Vec<(String, i64, Item)>,
}

#[derive(Default, Debug)]
struct Coverage {
    histories: usize,
    done_inserted: usize,
    done_duplicate: usize,
    refused: usize,
    captures_valid: usize,
    captures_invalid: usize,
    projects_accepted: usize,
    projects_refused: usize,
    reopens: usize,
}

fn payload(class: u8) -> Item {
    let summary = if class == 2 {
        vec![]
    } else {
        vec![json!({"type":"summary_text","text":format!("visible-{class}")})]
    };
    Item(
        json!({"type":"reasoning", "encrypted_content":format!("opaque-{class}"), "summary":summary}),
    )
}

fn boundary() -> Item {
    Item(
        json!({"type":"custom_tool_call","call_id":"capture","name":"haskell_sync","input":"checkpoint"}),
    )
}

fn target(kind: u8) -> ContextProjectionTarget {
    ContextProjectionTarget {
        model: if kind == 1 { "luna" } else { "astra" }.into(),
        route: if kind == 2 {
            ResponsesRoute::ChatGptPlan
        } else {
            ResponsesRoute::Codex
        },
        protocol: ResponsesProtocol::Standard,
    }
}

fn run_history(
    operations: &[Op],
    coverage: &RefCell<Coverage>,
) -> std::result::Result<(), TestCaseError> {
    let path = std::env::temp_dir().join(format!(
        "harness-cut-history-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let mut store = Store::open(&path).unwrap();
    let outcome = (|| {
        let head = RequestId("issued-response".into());
        let root = AgentPath("/root".into());
        store.create_request(&head, None, &root.0).unwrap();
        store.set_effort(&head, Effort::Low).unwrap();
        store
            .admit_agent(&root, None, Some(&head), &json!({}), &json!({}))
            .unwrap();
        let request = ResponsesRequest {
            input: vec![],
            instructions: "independent history".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "astra".into(),
            pinned_effort: Effort::Low,
            session_id: "cut-history".into(),
        };
        let issued = store
            .issue_provider_request(
                &head,
                store.seal_replay_request(&request).unwrap(),
                (ResponsesRoute::Codex, ResponsesProtocol::Standard),
            )
            .unwrap();
        let mut assembly = ResponseAssembly::default();
        let raw = json!({"type":"response.output_item.done","output_index":2,"item":boundary().0})
            .to_string();
        let Some(StreamEvent::CompletedItem(done)) = assembly.accept(&raw).unwrap() else {
            panic!("real done parser")
        };
        assert!(store.append_provider_done(&issued, &done).unwrap());
        let operation = store.claim(&CallId("capture".into()), &head).unwrap();
        let mut accepted = BTreeMap::<u64, u8>::new();
        // The authored effort update and boundary arrival occupy two positions
        // before generated callbacks. Provider indices are separate.
        let mut arrival_positions = BTreeMap::<u64, i64>::new();
        let mut next_arrival_position = 2_i64;
        let mut terminal = None;
        let mut frozen = Vec::<Frozen>::new();
        let mut epoch = 0;
        let mut next_child = 0;
        // The first capture has an index gap; the second is independently valid.
        // This causal prefix stays intact when the arbitrary suffix shrinks.
        let prefix = [
            Op::Capture,
            Op::Done(0, 0),
            Op::Done(1, 0),
            Op::Capture,
            Op::Project(0, 1),
            Op::Project(1, 1),
        ];
        for op in prefix.iter().chain(operations) {
            match *op {
                Op::Done(slot, class) => {
                    let index = [0_u64, 1, 3][slot as usize % 3];
                    let previous = accepted.get(&index).copied();
                    let expected = previous.map_or(terminal.is_none(), |old| old == class);
                    let raw = json!({"type":"response.output_item.done","output_index":index,"item":payload(class).0}).to_string();
                    let observed = match ResponseAssembly::default().accept(&raw) {
                        Ok(Some(StreamEvent::CompletedItem(done))) => {
                            store.append_provider_done(&issued, &done).map(|_| ())
                        }
                        Err(_) => Err(StoreError::InvalidProviderEvidence(head.clone())),
                        _ => panic!("done event must either complete or refuse"),
                    };
                    prop_assert_eq!(observed.is_ok(), expected, "operation {:?}", op);
                    if expected {
                        if previous.is_some() {
                            coverage.borrow_mut().done_duplicate += 1;
                        } else {
                            coverage.borrow_mut().done_inserted += 1;
                            accepted.insert(index, class);
                            arrival_positions.insert(index, next_arrival_position);
                            next_arrival_position += 1;
                        }
                    } else {
                        coverage.borrow_mut().refused += 1;
                    }
                }
                Op::Complete(class) => {
                    let mut after = accepted.clone();
                    after.entry(0).or_insert(class);
                    after.entry(1).or_insert(class);
                    let maximum = if after.contains_key(&3) { 3 } else { 2 };
                    let items = (0..=maximum)
                        .map(|index| {
                            if index == 2 {
                                boundary()
                            } else {
                                payload(after[&index])
                            }
                        })
                        .collect();
                    let turn = ResponsesTurn {
                        response_id: "complete".into(),
                        items,
                        usage: Default::default(),
                    };
                    let observed = store.record_provider_turn(&issued, &turn);
                    let expected = terminal != Some(false);
                    prop_assert_eq!(observed.is_ok(), expected, "operation {:?}", op);
                    if expected {
                        for index in after.keys() {
                            if !accepted.contains_key(index) {
                                arrival_positions.insert(*index, next_arrival_position);
                                next_arrival_position += 1;
                            }
                        }
                        accepted = after;
                        terminal = Some(true);
                    } else {
                        coverage.borrow_mut().refused += 1;
                    }
                }
                Op::Eof => {
                    store
                        .stop_provider_response(&head, ProviderResponseState::Interrupted)
                        .unwrap();
                    if terminal.is_none() {
                        terminal = Some(false);
                    }
                }
                Op::Capture => {
                    let cuts = store
                        .capture_checkpoint_cuts(&operation, &json!({}), Arc::new(()))
                        .unwrap();
                    let entries = accepted
                        .iter()
                        .filter(|(index, _)| **index < 2)
                        .map(|(i, c)| (*i, arrival_positions[i], *c))
                        .collect();
                    let valid = accepted.contains_key(&0) && accepted.contains_key(&1);
                    if valid {
                        coverage.borrow_mut().captures_valid += 1;
                    } else {
                        coverage.borrow_mut().captures_invalid += 1;
                    }
                    let observed_at_capture = {
                        let c = store.lock().unwrap();
                        history(&c, cuts.before_call().snapshot_request(), true)
                            .unwrap()
                            .into_iter()
                            .map(|entry| {
                                (entry.origin.request.0, entry.origin.position, entry.item)
                            })
                            .collect()
                    };
                    frozen.push(Frozen {
                        cuts,
                        entries,
                        valid,
                        epoch,
                        observed_at_capture,
                    });
                }
                Op::Project(which, kind) => {
                    let capture = &frozen[which % frozen.len()];
                    let destination = target(kind);
                    let child_path = AgentPath(format!("/root/child-{next_child}"));
                    next_child += 1;
                    let expected = capture.epoch == epoch
                        && capture.valid
                        && kind != 2
                        && (kind == 0 || capture.entries.iter().all(|(_, _, class)| *class != 2));
                    let observed = store.attach_checkpoint_child(
                        capture.cuts.before_call(),
                        CheckpointChild {
                            path: &child_path,
                            parent: &root,
                            contract: &json!({}),
                            checkout: &json!({}),
                            task: None,
                            target: &destination,
                        },
                    );
                    prop_assert_eq!(
                        observed.is_ok(),
                        expected,
                        "operation {:?}, capture-valid={}, capture-epoch={}, current-epoch={}",
                        op,
                        capture.valid,
                        capture.epoch,
                        epoch
                    );
                    if let Ok((child, _)) = observed {
                        coverage.borrow_mut().projects_accepted += 1;
                        let identity = store.standalone_identity(child_path);
                        let history = store
                            .context_request_state(child.head_request.as_ref().unwrap(), &identity)
                            .unwrap()
                            .history;
                        if kind == 0 {
                            prop_assert_eq!(
                                history
                                    .iter()
                                    .filter(|entry| entry.2.0["type"] != "configuration_update")
                                    .map(|entry| entry.2.clone())
                                    .collect::<Vec<_>>(),
                                capture
                                    .entries
                                    .iter()
                                    .map(|(_, _, class)| payload(*class))
                                    .collect::<Vec<_>>()
                            );
                        } else {
                            let rendered = serde_json::to_string(&history).unwrap();
                            prop_assert!(!rendered.contains("opaque-"));
                            for class in 0..2 {
                                prop_assert_eq!(
                                    rendered.matches(&format!("visible-{class}")).count(),
                                    capture
                                        .entries
                                        .iter()
                                        .filter(|(_, _, value)| *value == class)
                                        .count()
                                );
                            }
                        }
                    } else {
                        coverage.borrow_mut().projects_refused += 1;
                        prop_assert!(store.agent(&child_path).unwrap().is_none());
                    }
                }
                Op::Reopen => {
                    store = Store::open(&path).unwrap();
                    epoch += 1;
                    coverage.borrow_mut().reopens += 1;
                }
            }
            // Observe the actual reader only; expected membership comes entirely
            // from the accepted-fact map saved at capture, not Store traversal.
            for capture in &frozen {
                let c = store.lock().unwrap();
                let actual =
                    history(&c, capture.cuts.before_call().snapshot_request(), true).unwrap();
                let actual = actual
                    .into_iter()
                    .map(|entry| (entry.origin.request.0, entry.origin.position, entry.item))
                    .collect::<Vec<_>>();
                let expected = capture
                    .entries
                    .iter()
                    .map(|(_, position, class)| (head.0.clone(), *position, payload(*class)))
                    .collect::<Vec<_>>();
                let provider_occurrences = actual
                    .iter()
                    .filter(|(_, _, item)| item.0["type"] != "configuration_update")
                    .cloned()
                    .collect::<Vec<_>>();
                prop_assert_eq!(
                    provider_occurrences,
                    expected,
                    "provider cut after {:?}",
                    op
                );
                // Configuration is outside provider occurrence composition. The
                // whole issued cut additionally obeys immutable-snapshot identity.
                prop_assert_eq!(
                    &actual,
                    &capture.observed_at_capture,
                    "whole cut changed after {:?}",
                    op
                );
            }
        }
        coverage.borrow_mut().histories += 1;
        Ok(())
    })();
    drop(store);
    if outcome.is_ok() {
        std::fs::remove_file(&path).unwrap();
    } else {
        eprintln!(
            "retained failing conversation database: {}; history={operations:?}",
            path.display()
        );
    }
    outcome
}

#[test]
fn captured_cuts_match_independent_occurrence_histories() {
    use proptest::test_runner::{Config, TestRunner};
    let strategy = proptest::collection::vec((0_u8..7, 0_u8..3, 0_u8..3), 1..40).prop_map(|raw| {
        raw.into_iter()
            .map(|(kind, slot, value)| match kind {
                0 => Op::Done(slot, value),
                1 => Op::Complete(value),
                2 => Op::Eof,
                3 => Op::Capture,
                4 | 5 => Op::Project(slot as usize, value),
                _ => Op::Reopen,
            })
            .collect::<Vec<_>>()
    });
    let mut config = Config {
        cases: 96,
        source_file: Some(file!()),
        ..Config::default()
    };
    if let Ok(path) = std::env::var("HARNESS_PROPTEST_REGRESSIONS") {
        config.failure_persistence = Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(Box::leak(path.into_boxed_str())),
        ));
    }
    let coverage = RefCell::new(Coverage::default());
    let result = TestRunner::new(config).run(&strategy, |ops| run_history(&ops, &coverage));
    eprintln!("conversation cut history coverage: {:?}", coverage.borrow());
    result.unwrap();
}
