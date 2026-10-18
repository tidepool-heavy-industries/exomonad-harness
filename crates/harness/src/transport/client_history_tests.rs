//! A literal history oracle for the production reader. The expected outcome
//! never calls the assembly's stop converter, failure classifier or projection.
use super::*;
use crate::transport::{
    IncompleteReason, ProviderStreamFailureEvent, ResponseEnd, ResponseTerminal, StreamInterruption,
};
use crate::{item::Item, store::ProviderResponseState};
use proptest::prelude::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
enum End {
    Complete,
    Incomplete,
    Budget,
    Failed,
    Cancelled,
    Error,
    Eof,
    ReadFailure,
    Malformed,
}
fn end_strategy() -> impl Strategy<Value = End> {
    prop_oneof![
        Just(End::Complete),
        Just(End::Incomplete),
        Just(End::Budget),
        Just(End::Failed),
        Just(End::Cancelled),
        Just(End::Error),
        Just(End::Eof),
        Just(End::ReadFailure),
        Just(End::Malformed),
    ]
}
fn frame(event: Value, ending: &str, body: &mut Vec<u8>) {
    body.extend_from_slice(format!("data: {event}{ending}{ending}").as_bytes());
}
fn response_item(index: u64, text: &str) -> Item {
    Item(json!({"type":"reasoning","id":format!("item-{index}"),
        "summary":[{"type":"summary_text","text":text}],"encrypted_content":format!("opaque+/{text}=")}))
}
fn native_step(text: &str) -> Value {
    json!({"type":"thought","signature":format!("opaque+/{text}="),
        "summary":[{"type":"text","text":text}]})
}
fn interaction_item(index: u64, text: &str) -> Item {
    Item(json!({"type":"reasoning","id":format!("history:{index}"),
        "summary":[{"type":"summary_text","text":text}],"gemini_step":native_step(text)}))
}
struct History {
    gemini: bool,
    rows: Vec<(String, u8)>,
    gap: bool,
    unfinished: bool,
    end: End,
    line_ending: &'static str,
}
impl History {
    fn rows_in_wire_order(&self) -> Vec<(u64, &str)> {
        let mut rows = self.rows.iter().enumerate().collect::<Vec<_>>();
        rows.sort_by_key(|(_, (_, priority))| priority);
        rows.into_iter()
            .map(|(index, (text, _))| (index as u64 + u64::from(self.gap), text.as_str()))
            .collect()
    }
    fn wire(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let mut push = |event| frame(event, self.line_ending, &mut body);
        push(if self.gemini {
            json!({"event_type":"interaction.created","interaction":{"id":"history"}})
        } else {
            json!({"type":"response.created","response":{"id":"history"}})
        });
        for (index, text) in self.rows_in_wire_order() {
            if self.gemini {
                push(json!({"event_type":"step.start","index":index,"step":native_step(text)}));
                push(json!({"event_type":"step.stop","index":index}));
            } else {
                push(
                    json!({"type":"response.output_item.done","output_index":index,"item":response_item(index,text).0}),
                );
            }
        }
        if self.unfinished && matches!(self.end, End::Eof | End::ReadFailure) {
            let index = self.rows.len() as u64 + u64::from(self.gap);
            push(if self.gemini {
                json!({"event_type":"step.start","index":index,"step":{"type":"function_call","id":"never","name":"effect"}})
            } else {
                json!({"type":"response.output_item.added","output_index":index,"item":{"type":"function_call","id":"never","name":"effect"}})
            });
        }
        let terminal = if self.gemini {
            match self.end {
                End::Complete => Some(
                    json!({"event_type":"interaction.completed","interaction":{"id":"history","status":"completed"}}),
                ),
                End::Incomplete => Some(
                    json!({"event_type":"interaction.status_update","interaction_id":"history","status":"incomplete"}),
                ),
                End::Budget => Some(
                    json!({"event_type":"interaction.status_update","interaction_id":"history","status":"budget_exceeded"}),
                ),
                End::Failed => Some(
                    json!({"event_type":"interaction.status_update","interaction_id":"history","status":"failed"}),
                ),
                End::Cancelled => Some(
                    json!({"event_type":"interaction.status_update","interaction_id":"history","status":"cancelled"}),
                ),
                End::Error => Some(json!({"event_type":"error"})),
                End::Malformed => Some(
                    json!({"event_type":"interaction.status_update","interaction_id":"history","status":"new-unrecognized-terminal"}),
                ),
                End::Eof | End::ReadFailure => None,
            }
        } else {
            match self.end {
                End::Complete => Some(
                    json!({"type":"response.completed","response":{"id":"history","output":[]}}),
                ),
                End::Incomplete | End::Budget => Some(
                    json!({"type":"response.incomplete","response":{"id":"history","incomplete_details":{"reason":if matches!(self.end,End::Budget) {"budget_exceeded"} else {"content_filter"}}}}),
                ),
                End::Failed | End::Cancelled => {
                    Some(json!({"type":"response.failed","response":{"id":"history"}}))
                }
                End::Error => Some(json!({"type":"error"})),
                End::Malformed => Some(json!({"type":"response.output_item.done","item":{}})),
                End::Eof | End::ReadFailure => None,
            }
        };
        if let Some(terminal) = terminal {
            push(terminal);
        }
        // A recognized terminal wins over malformed buffered tail bytes. A
        // malformed terminal does not acquire completion or replay permission.
        if !matches!(self.end, End::Eof | End::ReadFailure) {
            body.extend_from_slice(b"\xff\n");
        }
        body
    }
    fn expected_items(&self) -> Vec<Item> {
        let rows = self.rows_in_wire_order();
        if !self.gemini {
            return rows
                .into_iter()
                .map(|(index, text)| response_item(index, text))
                .collect();
        }
        // Native siblings become visible only as a contiguous stopped prefix.
        // This oracle deliberately uses sets, not the production step machine.
        let mut seen = BTreeSet::new();
        let mut emitted = Vec::new();
        let mut next = 0;
        for (index, _) in rows {
            seen.insert(index);
            while seen.contains(&next) {
                let text = &self.rows[next as usize].0;
                emitted.push(interaction_item(next, text));
                next += 1;
            }
        }
        emitted
    }
    fn expected_terminal(&self) -> Option<ResponseTerminal> {
        match self.end {
            End::Complete => Some(ResponseTerminal::Completed),
            End::Incomplete => Some(ResponseTerminal::Incomplete {
                response_id: "history".into(),
                reason: if self.gemini {
                    None
                } else {
                    Some(IncompleteReason::ContentFilter)
                },
            }),
            End::Budget => Some(ResponseTerminal::Incomplete {
                response_id: "history".into(),
                reason: Some(if self.gemini {
                    IncompleteReason::BudgetExceeded
                } else {
                    IncompleteReason::Unknown
                }),
            }),
            End::Failed | End::Cancelled if !self.gemini => Some(ResponseTerminal::ResponseFailed),
            End::Failed => Some(ResponseTerminal::InteractionFailed),
            End::Cancelled => Some(ResponseTerminal::InteractionCancelled),
            End::Error => Some(ResponseTerminal::Error),
            End::Eof | End::ReadFailure | End::Malformed => None,
        }
    }
}
async fn check_history(history: &History, cuts: &[usize]) {
    let bytes = history.wire();
    let mut chunks = Vec::new();
    let mut position = 0;
    for width in cuts.iter().cycle() {
        if position == bytes.len() {
            break;
        }
        let end = (position + width).min(bytes.len());
        chunks.push(Ok::<_, std::io::Error>(bytes[position..end].to_vec()));
        position = end;
    }
    if matches!(history.end, End::ReadFailure) {
        chunks.push(Err(std::io::Error::other("synthetic body read failure")));
    }
    let response = reqwest::Response::from(
        axum::http::Response::builder()
            .status(200)
            .body(reqwest::Body::wrap_stream(futures_util::stream::iter(
                chunks,
            )))
            .unwrap(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let (publisher, observation) = tokio::sync::oneshot::channel();
    let mut guard = ObservationGuard::new(Some(publisher), None);
    let result = read_response_observed(
        response,
        Some(tx),
        "",
        "",
        &mut guard,
        if history.gemini {
            ResponsesRoute::GeminiInteractions
        } else {
            ResponsesRoute::DeepSeek
        },
    )
    .await;
    drop(guard);
    let observed = observation.await.unwrap();
    let mut items = Vec::new();
    while let Some(event) = rx.recv().await {
        let StreamEvent::CompletedItem(done) = event else {
            panic!("invented delta");
        };
        items.push(done.item().clone());
    }
    assert_eq!(items, history.expected_items());
    assert_eq!(observed.terminal, history.expected_terminal());
    let expected_end = match history.end {
        End::Eof => ResponseEnd::CleanEof,
        End::ReadFailure => ResponseEnd::ReadFailed,
        End::Malformed => ResponseEnd::ProtocolError,
        _ => ResponseEnd::ProviderTerminal,
    };
    assert_eq!(observed.end, expected_end);
    let invalid_complete =
        matches!(history.end, End::Complete) && history.gap && !history.rows.is_empty();
    match (&history.end, result) {
        (End::Complete, Ok(turn)) if !invalid_complete => {
            assert_eq!(turn.response_id, "history");
            let expected = history
                .rows
                .iter()
                .enumerate()
                .map(|(index, (text, _))| {
                    if history.gemini {
                        interaction_item(index as u64, text)
                    } else {
                        response_item(index as u64, text)
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(turn.items, expected);
        }
        (end, Err(error)) => {
            // Expected categories are literal native facts; the production
            // classifier and shared disposition method are tested as outputs.
            let state = match end {
                End::Eof => {
                    assert!(matches!(
                        error,
                        TransportError::IncompleteResponse(StreamInterruption::MissingCompletion)
                    ));
                    ProviderResponseState::Interrupted
                }
                End::ReadFailure => {
                    assert!(matches!(
                        error,
                        TransportError::IncompleteResponse(StreamInterruption::ReadFailed)
                    ));
                    ProviderResponseState::Interrupted
                }
                End::Incomplete | End::Budget => {
                    let Some(ResponseTerminal::Incomplete {
                        response_id,
                        reason,
                    }) = history.expected_terminal()
                    else {
                        unreachable!()
                    };
                    assert!(
                        matches!(&error, TransportError::ResponseIncomplete { response_id:actual,reason:actual_reason } if actual == &response_id && actual_reason == &reason)
                    );
                    ProviderResponseState::Failed
                }
                End::Failed | End::Cancelled | End::Error => {
                    let expected = match end {
                        End::Error => ProviderStreamFailureEvent::Error,
                        _ if !history.gemini => ProviderStreamFailureEvent::ResponseFailed,
                        End::Failed => ProviderStreamFailureEvent::InteractionFailed,
                        End::Cancelled => ProviderStreamFailureEvent::InteractionCancelled,
                        _ => unreachable!(),
                    };
                    assert!(
                        matches!(&error, TransportError::ProviderStreamFailure { event, diagnostic:None } if *event == expected)
                    );
                    ProviderResponseState::Failed
                }
                End::Complete => {
                    assert!(invalid_complete);
                    assert!(matches!(
                        error,
                        TransportError::IncompleteOutputIndices { index: 0 }
                    ));
                    ProviderResponseState::Failed
                }
                End::Malformed => {
                    assert!(matches!(
                        error,
                        TransportError::MissingOutputIndex | TransportError::Stream(_)
                    ));
                    ProviderResponseState::Failed
                }
            };
            assert_eq!(error.failure_kind().provider_response_state(), state);
            assert!(error.request_failure().is_none());
        }
        (end, result) => panic!("wrong native outcome for {end:?}: {result:?}"),
    }
}

proptest! {
    #[test]
    fn reader_native_history_preserves_diagnosis_exact_prefix_and_chunk_invariance(
        gemini in any::<bool>(),
        rows in proptest::collection::vec(("[a-zλ😀\\r\\n]{0,16}", any::<u8>()), 0..6),
        gap in any::<bool>(), unfinished in any::<bool>(), end in end_strategy(),
        line_ending in prop_oneof![Just("\n"),Just("\r"),Just("\r\n")],
        cuts in proptest::collection::vec(1..100usize,1..12),
    ) {
        let history = History {gemini,rows,gap,unfinished,end,line_ending};
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            check_history(&history, &cuts).await;
            check_history(&history, &[usize::MAX]).await;
        });
    }
}

#[tokio::test]
async fn reader_terminal_matrix_includes_empty_gapped_and_retained_prefixes() {
    for gemini in [false, true] {
        for end in [
            End::Complete,
            End::Incomplete,
            End::Budget,
            End::Failed,
            End::Cancelled,
            End::Error,
            End::Eof,
            End::ReadFailure,
            End::Malformed,
        ] {
            for gap in [false, true] {
                for rows in [
                    vec![],
                    vec![("λ\r\n😀".into(), 2), ("opaque sibling".into(), 1)],
                ] {
                    check_history(
                        &History {
                            gemini,
                            rows,
                            gap,
                            unfinished: true,
                            end,
                            line_ending: "\r\n",
                        },
                        &[1],
                    )
                    .await;
                }
            }
        }
    }
}

#[tokio::test]
async fn dropped_native_reader_preserves_partial_facts_without_issuing_interruption() {
    for gemini in [false, true] {
        let history = History {
            gemini,
            rows: vec![("partial λ😀".into(), 0)],
            gap: false,
            unfinished: false,
            end: End::Eof,
            line_ending: "\r\n",
        };
        let mut body = history.wire();
        let partial = b"data: {\"unfinished-private-body\":";
        body.extend_from_slice(partial);
        let bytes = body.len() as u64;
        let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) })
            .chain(futures_util::stream::pending());
        let response = reqwest::Response::from(
            axum::http::Response::builder()
                .status(200)
                .body(reqwest::Body::wrap_stream(stream))
                .unwrap(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let (publisher, observation) = tokio::sync::oneshot::channel();
        let mut reader = Box::pin(async move {
            let mut guard = ObservationGuard::new(Some(publisher), None);
            read_response_observed(
                response,
                Some(tx),
                "",
                "",
                &mut guard,
                if gemini {
                    ResponsesRoute::GeminiInteractions
                } else {
                    ResponsesRoute::DeepSeek
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                outcome = &mut reader => panic!("running native reader settled: {outcome:?}"),
                event = rx.recv() => {
                    let Some(StreamEvent::CompletedItem(done)) = event else { panic!("missing completed prefix"); };
                    assert_eq!(done.item(), &history.expected_items()[0]);
                }
            }
            assert!(futures_util::poll!(&mut reader).is_pending());
        }).await.unwrap();
        drop(reader);
        let observed = observation.await.unwrap();
        assert_eq!(observed.end, ResponseEnd::Abandoned);
        assert_eq!(observed.terminal, None);
        assert_eq!(observed.received_bytes, Some(bytes));
        assert_eq!(observed.unframed_line_bytes, partial.len() as u64);
        assert!(rx.recv().await.is_none());
        assert!(
            !serde_json::to_string(&observed)
                .unwrap()
                .contains("unfinished-private-body")
        );
    }
}
