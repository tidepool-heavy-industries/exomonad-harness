//! Production-path custom scenario regression tests.
//!
//! These tests exercise the actual CLI provider boundary. The deterministic
//! server/browser journey must separately observe the published PROGRESS
//! envelope and reload a persisted Store; reconstructing `ToolJobs` here is only an in-memory
//! projection check. No fixture-side registry is allowed here.

use super::*;
use harness::model::CallId;
use harness::provider::Provider;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn cli_advertises_native_custom_only_with_dev_authority() {
    let disabled = CliProvider(DemoProvider::development(".", false));
    assert!(
        disabled
            .all_tools()
            .iter()
            .all(|tool| tool["name"] != "run"),
        "trusted shell cannot be exposed by default"
    );
    let enabled = CliProvider(DemoProvider::development(".", true));
    let run = enabled
        .all_tools()
        .into_iter()
        .find(|tool| tool["name"] == "run")
        .expect("dev-authorized run advertised");
    assert_eq!(run["type"], "custom", "CLI must not rewrite to function");
    assert!(run.get("parameters").is_none());
    assert!(run.get("strict").is_none());
}

#[tokio::test]
async fn cli_custom_raw_input_reaches_shell_without_json_reinterpretation() {
    let provider = CliProvider(DemoProvider::development(".", true));
    let raw = "printf '%s' 'line 1\n\"quoted\" \\\\ unicode: λ'";
    let (context, _progress) = CallContext::detached_for_test(
        harness::provider::JobHandle("raw-custom-job".into()),
        harness::model::CallId("raw-custom-call".into()),
        AgentPath(ROOT_PATH.into()),
    );
    let result = provider
        .call_custom_with_context("run", raw.to_owned(), context)
        .await
        .expect("typed native raw custom call");
    assert_eq!(result["success"], json!(true));
    assert_eq!(
        result["stdout"],
        json!("line 1\n\"quoted\" \\\\ unicode: λ")
    );
}

#[tokio::test]
async fn cli_custom_raw_input_respects_shell_authority() {
    let provider = CliProvider(DemoProvider::development(".", false));
    let (context, _progress) = CallContext::detached_for_test(
        harness::provider::JobHandle("denied-custom-job".into()),
        harness::model::CallId("denied-custom-call".into()),
        AgentPath(ROOT_PATH.into()),
    );
    let error = provider
        .call_custom_with_context("run", "printf forbidden".into(), context)
        .await;
    assert!(
        error.is_err(),
        "raw input cannot bypass dev shell authority"
    );
}

#[tokio::test]
async fn cli_rejects_raw_custom_input_on_legacy_function_route() {
    let provider = CliProvider(DemoProvider::development(".", true));
    assert!(
        provider
            .call("run", json!("printf must-not-run"))
            .await
            .is_err(),
        "raw custom input must use the typed provider method"
    );
}

#[tokio::test]
async fn deterministic_custom_scenario_retains_kind_concurrency_and_first_terminal_projection() {
    for cancel in [false, true] {
        let command_id = if cancel {
            "custom-cancel-test"
        } else {
            "custom-release-test"
        };
        let raw = "line 1\n\"quoted\" \\\\ unicode: λ";
        let command = format!("custom {raw}");
        let (a_id, b_id) = async_scenario_call_ids(command_id);
        let store = Arc::new(Store::memory().expect("memory Store"));
        let root = AgentPath(ROOT_PATH.into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .expect("register root");
        let scheduler = Arc::new(JobScheduler::new(2).expect("scheduler"));
        let jobs = async_demo::ToolJobs::default();
        let gate = async_demo::GateControl::default();
        let task_store = store.clone();
        let task_scheduler = scheduler.clone();
        let task_jobs = jobs.clone();
        let task_gate = gate.clone();
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            run_async_scenario_turn_for_command(
                task_store,
                task_scheduler,
                None,
                command_id,
                &command,
                task_jobs,
                task_gate,
                cancel_rx,
            )
            .await
        });
        let started = tokio::time::timeout(Duration::from_secs(2), gate.wait_until_started())
            .await
            .expect("custom call A did not start");
        assert_eq!(started.0, a_id);
        let progress = tokio::time::timeout(Duration::from_secs(2), gate.wait_until_progress())
            .await
            .expect("provider did not emit custom_started progress");
        assert_eq!(progress["event"], "custom_started");
        assert_eq!(progress["callId"], a_id);
        assert_eq!(progress["inputLength"], json!(raw.len()));
        tokio::time::timeout(
            Duration::from_secs(2),
            jobs.wait_for_delivered(&CallId(b_id.clone())),
        )
        .await
        .expect("function B did not complete while custom A remained pending");
        let pending = jobs
            .records()
            .into_iter()
            .find(|job| job.call_id == a_id)
            .unwrap();
        assert_eq!(pending.tool_name, "run");
        assert_eq!(pending.tool_kind, Some(harness::item::ToolKind::Custom));
        assert_eq!(pending.state, harness::server::ToolJobState::Running);
        assert!(pending.output.is_none());
        assert!(!pending.request_id.is_empty());
        let input = gate.inputs();
        assert!(
            input.iter().flatten().any(|item| {
                item.0["type"] == "custom_tool_call"
                    && item.0["call_id"] == a_id
                    && item.0["name"] == "run"
                    && item.0["input"] == raw
            }),
            "raw custom call missing from Engine input: {input:#?}"
        );
        assert!(
            input
                .iter()
                .flatten()
                .any(|item| { item.0["type"] == "function_call" && item.0["call_id"] == b_id }),
            "function B did not coexist with custom A"
        );

        if cancel {
            let operation = store.claims(&CallId(a_id.clone())).expect("load A claim")[0]
                .operation
                .clone();
            scheduler.cancel(&operation).await.expect("cancel A");
            gate.release(); // A late release must not replace cancellation.
        } else {
            gate.release();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("scenario completion timeout")
            .expect("scenario join")
            .expect("Engine completion");
        assert_eq!(result.0, "Async scenario completed.");
        // This reconstructs the projection from its records; it is not a
        // process restart or durable server/Store reopen test.
        let retained = async_demo::ToolJobs::reopen(jobs.records());
        let a = retained
            .records()
            .into_iter()
            .find(|job| job.call_id == a_id)
            .unwrap();
        let expected = if cancel {
            harness::server::ToolJobState::Cancelled
        } else {
            harness::server::ToolJobState::Settled
        };
        assert_eq!(
            a.state, expected,
            "first terminal result changed on projection reconstruction"
        );
        if cancel {
            assert!(a.output.is_none(), "late success replaced cancellation");
        } else {
            assert!(a.output.is_some(), "settled custom output was lost");
        }
        let items = store
            .items(&harness::model::RequestId(a.request_id.clone()))
            .expect("durable request items");
        assert_eq!(
            items
                .iter()
                .filter(|item| {
                    item.0["type"] == "custom_tool_call"
                        && item.0["call_id"] == a_id
                        && item.0["input"] == raw
                })
                .count(),
            1,
            "original request must retain exactly one raw custom call"
        );
        // Inspect only the completed request's same-branch lineage back to
        // the emitting request. A global CallId lookup is not scoped evidence:
        // another branch may reuse this identifier.
        let origin = harness::model::RequestId(a.request_id.clone());
        let origin_branch = store.request(&origin).unwrap().unwrap().branch;
        let mut cursor = result.2.clone();
        let mut later_outputs = Vec::new();
        while cursor != origin {
            let request = store
                .request(&cursor)
                .unwrap()
                .expect("same-branch request");
            assert_eq!(request.branch, origin_branch, "crossed a branch boundary");
            later_outputs.extend(store.items(&cursor).unwrap().into_iter().filter(|item| {
                item.0["type"] == "custom_tool_call_output" && item.0["call_id"] == a_id
            }));
            cursor = request
                .parent
                .expect("original call is ancestor of completion");
        }
        if !cancel {
            assert_eq!(
                later_outputs.len(),
                1,
                "exactly one kind-matched custom output must persist after the emitting request"
            );
            let output = &later_outputs[0];
            assert_eq!(output.0["type"], "custom_tool_call_output");
            assert_eq!(output.0["call_id"], a_id);
        } else {
            // The scheduler may persist cancellation, but never late success.
            assert!(later_outputs.len() <= 1, "duplicate cancelled output");
            for output in later_outputs {
                assert_eq!(output.0["type"], "custom_tool_call_output");
                assert_eq!(output.0["call_id"], a_id);
                assert_eq!(
                    serde_json::from_str::<Value>(output.0["output"].as_str().unwrap())
                        .expect("typed cancelled output JSON"),
                    json!({"error":"job cancelled"})
                );
            }
        }
    }
}
