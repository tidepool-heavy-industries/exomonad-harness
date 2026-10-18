use super::*;
use crate::transport::{
    EventKind, IncompleteReason, ResponseEnd, ResponseTerminal, StreamInterruption,
    TransportObservation,
};
use proptest::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "active-request-secret";
const ACCOUNT: &str = "active-account-secret";

async fn response(
    body: Vec<u8>,
    truncated: bool,
    hold_open: bool,
) -> (
    reqwest::Response,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        socket.read(&mut request).await.unwrap();
        let length = body.len() + usize::from(truncated || hold_open) * 100;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-Id: {TOKEN}\r\nSet-Cookie: never-retain-this\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        if hold_open {
            let _ = released.await;
        }
        let _ = socket.shutdown().await;
    });
    let response = http_client()
        .unwrap()
        .get(format!("http://{address}/responses"))
        .send()
        .await
        .unwrap();
    (response, release, server)
}

fn incomplete() -> Vec<u8> {
    format!("data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp-test\"}}}}\n\ndata: {{\"type\":\"response.incomplete\",\"response\":{{\"id\":\"resp-test\",\"incomplete_details\":{{\"reason\":\"max_output_tokens\"}},\"encrypted_content\":\"{TOKEN}\"}}}}\n\n").into_bytes()
}

#[tokio::test]
async fn explicit_incomplete_stops_before_eof_and_delivers_metadata_before_stream_close() {
    let body = incomplete();
    let bytes = body.len() as u64;
    let (response, release, server) = response(body, false, true).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        read_response(response, Some(tx), TOKEN, ACCOUNT, Some(observation_tx)),
    )
    .await
    .expect("explicit terminal must not await EOF or the HTTP body timeout")
    .unwrap_err();
    assert!(
        matches!(&error, TransportError::ResponseIncomplete { response_id, reason: Some(IncompleteReason::MaxOutputTokens) } if response_id == "resp-test")
    );
    assert!(matches!(
        error.failure_kind(),
        super::super::TransportFailureKind::ProviderIncomplete { .. }
    ));
    assert!(error.request_failure().is_none());
    let metadata = observation_rx.await.unwrap();
    assert!(
        rx.recv().await.is_none(),
        "exactly one summary, emitted before sender drop"
    );
    assert_eq!(metadata.http_status, Some(200));
    assert_eq!(metadata.server_request_id.as_deref(), Some("[redacted]"));
    assert_eq!(metadata.content_type.as_deref(), Some("text/event-stream"));
    assert_eq!(metadata.response_created_id.as_deref(), Some("resp-test"));
    assert_eq!(metadata.received_bytes, Some(bytes));
    assert_eq!(metadata.framed_events, Some(2));
    assert_eq!(metadata.last_recognized_event, Some(EventKind::Incomplete));
    assert_eq!(
        metadata.terminal,
        Some(ResponseTerminal::Incomplete {
            response_id: "resp-test".into(),
            reason: Some(IncompleteReason::MaxOutputTokens)
        })
    );
    assert_eq!(metadata.end, ResponseEnd::ProviderTerminal);
    assert_eq!(
        (metadata.unframed_line_bytes, metadata.unframed_data_bytes),
        (0, 0)
    );
    let encoded = serde_json::to_string(&metadata).unwrap();
    for private in [
        TOKEN,
        ACCOUNT,
        "encrypted_content",
        "Set-Cookie",
        "never-retain-this",
    ] {
        assert!(!encoded.contains(private));
    }
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn eof_and_read_failure_keep_distinct_causes_and_never_dispatch_unfinished_items() {
    for truncated in [false, true] {
        let unknown = "private unknown event name λ".repeat(100);
        let mut body = format!(
            "data: {{\"type\":{}}}\n\n",
            serde_json::to_string(&unknown).unwrap()
        );
        // Neither this complete data line nor the unfinished next line has the
        // final blank line that would dispatch an SSE event.
        body.push_str("event: error\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"name\":\"never-run\"}}\npartial-json");
        let (response, _, server) = response(body.into_bytes(), truncated, false).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
        let error = read_response(response, Some(tx), TOKEN, ACCOUNT, Some(observation_tx))
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::IncompleteResponse(cause) if cause == if truncated { StreamInterruption::ReadFailed } else { StreamInterruption::MissingCompletion })
        );
        let metadata = observation_rx.await.unwrap();
        assert!(rx.recv().await.is_none());
        assert_eq!(
            metadata.end,
            if truncated {
                ResponseEnd::ReadFailed
            } else {
                ResponseEnd::CleanEof
            }
        );
        assert_eq!(metadata.terminal, None);
        assert_eq!(metadata.framed_events, Some(1));
        assert_eq!(metadata.unknown_event_count, 1);
        assert!(
            matches!(&metadata.event_kind_tail[0], EventKind::Unknown { sha256 } if sha256.len() == 64)
        );
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("private unknown")
        );
        assert_eq!(metadata.unframed_line_bytes, "partial-json".len() as u64);
        assert!(metadata.unframed_data_bytes > 0);
        assert!(metadata.unframed_event_name);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn observer_closure_does_not_replace_terminal_error_with_full_data_channel() {
    let (response, release, server) = response(incomplete(), false, true).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
    tx.send(StreamEvent::Delta {
        tool_name: None,
        item_id: "prior".into(),
        channel: super::super::sse::OutputChannel::Assistant,
        index: 0,
        text: "prior".into(),
    })
    .await
    .unwrap();
    let reader = tokio::spawn(read_response(
        response,
        Some(tx),
        TOKEN,
        ACCOUNT,
        Some(observation_tx),
    ));
    drop(observation_rx);
    // Neither diagnostic closure nor full output capacity blocks a terminal.
    rx.close();
    drop(rx);
    let error = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, TransportError::ResponseIncomplete { .. }));
    let _ = release.send(());
    server.await.unwrap();
}

fn visible_then_terminal(completed: bool) -> Vec<u8> {
    let mut wire = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"answer\",\"content_index\":0,\"delta\":\"λ\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"answer\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"λ\"}]}}\n\n"
    ).as_bytes().to_vec();
    if completed {
        wire.extend_from_slice(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-capacity-one\",\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n");
    }
    wire
}

#[tokio::test]
async fn capacity_one_data_stream_preserves_items_and_independent_observation() {
    let (response, release, server) = response(visible_then_terminal(true), false, true).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(read_response(
        response,
        Some(tx),
        TOKEN,
        ACCOUNT,
        Some(observation_tx),
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        assert!(matches!(rx.recv().await.unwrap(), StreamEvent::Delta { text, .. } if text == "λ"));
        assert!(
            matches!(rx.recv().await.unwrap(), StreamEvent::CompletedItem(done)
            if done.item().0["id"] == "answer")
        );
        let turn = reader.await.unwrap().unwrap();
        assert_eq!(turn.response_id, "resp-capacity-one");
        assert_eq!(turn.items.len(), 1);
        assert!(rx.recv().await.is_none());
        let observed = observation_rx.await.unwrap();
        assert_eq!(observed.terminal, Some(ResponseTerminal::Completed));
        assert_eq!(observed.end, ResponseEnd::ProviderTerminal);
        assert_eq!(observed.usage.unwrap().input_tokens, Some(3));
        // Bare response readers cannot issue request/queue timing facts.
        assert_eq!(observed.backend, None);
        assert_eq!(observed.first_text_ms, None);
    })
    .await
    .expect("capacity-one data channel must make progress with observation enabled");
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn dropped_held_stream_retains_observed_partial_facts_without_completion() {
    // One deterministic body chunk establishes exactly which partial bytes were
    // consumed before the pending next chunk. HTTP framing is covered by the
    // capacity-one test; this control isolates cancellation of the real reader.
    let unfinished = b"data: {\"private-partial-arguments\":";
    let mut body = visible_then_terminal(false);
    body.extend_from_slice(unfinished);
    let expected_bytes = body.len() as u64;
    let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) })
        .chain(futures_util::stream::pending());
    let response = reqwest::Response::from(
        axum::http::Response::builder()
            .status(200)
            .body(reqwest::Body::wrap_stream(stream))
            .unwrap(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
    let mut reader = Box::pin(read_response(
        response,
        Some(tx),
        TOKEN,
        ACCOUNT,
        Some(observation_tx),
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut items = 0;
        while items < 2 {
            tokio::select! {
                result = &mut reader => panic!("held response unexpectedly settled: {result:?}"),
                event = rx.recv() => { assert!(event.is_some()); items += 1; }
            }
        }
        assert!(futures_util::poll!(&mut reader).is_pending());
    })
    .await
    .unwrap();
    drop(reader);
    let observed = observation_rx.await.unwrap();
    assert_eq!(observed.end, ResponseEnd::Abandoned);
    assert_eq!(observed.terminal, None);
    assert_eq!(
        observed.last_recognized_event,
        Some(EventKind::OutputItemDone)
    );
    assert_eq!(observed.received_bytes, Some(expected_bytes));
    assert_eq!(observed.framed_events, Some(2));
    assert_eq!(observed.unframed_line_bytes, unfinished.len() as u64);
    assert_eq!(observed.unframed_data_bytes, 0);
    assert!(
        !serde_json::to_string(&observed)
            .unwrap()
            .contains("private-partial-arguments")
    );
    assert_eq!(observed.usage, None);
    assert!(rx.recv().await.is_none());
}

#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    Completed,
    Incomplete,
    ProtocolError,
    Eof,
}
fn disposition(wire: &[u8], cuts: &[usize]) -> Disposition {
    let mut framer = SseFramer::new();
    let mut assembly = ResponseAssembly::default();
    let mut position = 0;
    let mut ordinal = 0;
    while position < wire.len() {
        let width = cuts.get(ordinal % cuts.len()).copied().unwrap_or(1).max(1);
        let end = (position + width).min(wire.len());
        let (frames, error) = framer.push(&wire[position..end]);
        for frame in frames {
            match assembly.advance(&frame.data, "", "", None) {
                Ok(super::super::assembly::AssemblyAdvance::Complete(_)) => {
                    assert!(assembly.finish_complete().is_ok());
                    return Disposition::Completed;
                }
                Ok(super::super::assembly::AssemblyAdvance::Progress(_)) => {}
                Ok(super::super::assembly::AssemblyAdvance::Stopped(
                    super::super::assembly::DeclaredStop::Incomplete { .. },
                )) => return Disposition::Incomplete,
                Ok(super::super::assembly::AssemblyAdvance::Stopped(_)) | Err(_) => {
                    return Disposition::ProtocolError;
                }
            }
        }
        if error.is_some() {
            return Disposition::ProtocolError;
        }
        position = end;
        ordinal += 1;
    }
    assert!(matches!(
        assembly.finish(),
        Err(TransportError::IncompleteResponse(
            StreamInterruption::MissingCompletion
        ))
    ));
    Disposition::Eof
}
fn terminal_wire(completed: bool, line_end: &str) -> Vec<u8> {
    let terminal = if completed { "completed" } else { "incomplete" };
    let mut wire = format!("data: {{\"type\":{line_end}data: \"response.{terminal}\",\"response\":{{\"id\":\"resp-λ\",\"output\":[],\"incomplete_details\":{{\"reason\":\"content_filter\"}}}}}}{line_end}{line_end}").into_bytes();
    wire.extend_from_slice(b"\xff\n");
    wire
}

#[tokio::test]
async fn http_terminal_precedes_malformed_tail_without_waiting_for_body_close() {
    for completed in [false, true] {
        let (response, release, server) =
            response(terminal_wire(completed, "\r\n"), false, true).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            read_response(response, Some(tx), TOKEN, ACCOUNT, Some(observation_tx)),
        )
        .await
        .unwrap();
        if completed {
            assert_eq!(result.unwrap().response_id, "resp-λ");
            let observation = observation_rx.await.unwrap();
            assert_eq!(observation.terminal, Some(ResponseTerminal::Completed));
            assert_eq!(observation.end, ResponseEnd::ProviderTerminal);
            assert_eq!(observation.backend, None);
            assert_eq!(observation.queue_ms, None);
            assert_eq!(observation.total_ms, None);
            assert!(rx.recv().await.is_none());
        } else {
            assert!(matches!(
                result,
                Err(TransportError::ResponseIncomplete {
                    reason: Some(IncompleteReason::ContentFilter),
                    ..
                })
            ));
            assert_eq!(
                observation_rx.await.unwrap().end,
                ResponseEnd::ProviderTerminal
            );
            assert!(rx.recv().await.is_none());
        }
        let _ = release.send(());
        server.await.unwrap();
    }
}

#[test]
fn unknown_event_retention_is_fixed_size_and_never_copies_provider_labels() {
    let mut summary = TransportObservation::new(ResponseEnd::CleanEof);
    for _ in 0..100 {
        summary.event(EventKind::from_provider(Some(
            &"private-label-λ".repeat(1000),
        )));
    }
    assert_eq!(summary.unknown_event_count, 100);
    assert_eq!(summary.event_kind_tail.len(), 8);
    assert!(
        summary
            .event_kind_tail
            .iter()
            .all(|kind| matches!(kind, EventKind::Unknown { sha256 } if sha256.len() == 64))
    );
    assert!(summary.last_recognized_event.is_none());
    assert!(
        !serde_json::to_string(&summary)
            .unwrap()
            .contains("private-label")
    );
}

#[test]
fn every_terminal_split_precedes_invalid_utf8_tail() {
    for completed in [false, true] {
        for ending in ["\n", "\r", "\r\n"] {
            let wire = terminal_wire(completed, ending);
            let expected = if completed {
                Disposition::Completed
            } else {
                Disposition::Incomplete
            };
            for split in 1..wire.len() {
                assert_eq!(
                    disposition(&wire, &[split, wire.len()]),
                    expected,
                    "split={split}, completed={completed}, ending={ending:?}"
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn terminal_disposition_is_invariant_under_chunking(completed in any::<bool>(), ending in 0..3_usize, cuts in proptest::collection::vec(1..80_usize, 1..20)) {
        let wire = terminal_wire(completed, ["\n", "\r", "\r\n"][ending]);
        prop_assert_eq!(disposition(&wire, &cuts), if completed { Disposition::Completed } else { Disposition::Incomplete });
    }
}
