//! Abrupt process loss at the Store commit / host wake boundary.
//! The helper is a separate OS process. Its stdout is an explicit barrier;
//! the parent kills it rather than asking it to close the Store cleanly.
use harness::{
    item::Item,
    lifecycle::{CompletionCommit, CompletionProvenance, PublishedAnswer},
    model::{AgentPath, RequestId},
    store::Store,
};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

fn root() -> AgentPath {
    AgentPath("/root".into())
}
fn child() -> AgentPath {
    AgentPath("/root/child".into())
}
fn head() -> RequestId {
    RequestId("child-final".into())
}
fn answer() -> Item {
    PublishedAnswer {
        sender: child().0,
        result: json!({"kind":"completed","value":{"answer":42}}),
        provenance: CompletionProvenance {
            final_request: head(),
            seen_envelopes: vec![7],
            unseen_envelopes: vec![8],
        },
    }
    .to_message_item()
    .unwrap()
}

#[test]
fn process_crash_helper() {
    let Ok(path) = std::env::var("HARNESS_CRASH_DB") else {
        return;
    };
    let stage = std::env::var("HARNESS_CRASH_STAGE").unwrap();
    let store = Store::open(&path).unwrap();
    store
        .admit_agent(&root(), None, None, &json!({}), &json!({}))
        .unwrap();
    store
        .admit_agent(&child(), Some(&root()), None, &json!({}), &json!({}))
        .unwrap();
    store.create_request(&head(), None, &child().0).unwrap();
    if stage == "after" {
        assert!(matches!(
            store
                .complete_agent_with_publication(
                    &child(),
                    None,
                    &head(),
                    Some((&root(), &answer()))
                )
                .unwrap(),
            CompletionCommit::Committed {
                envelope_id: Some(_)
            }
        ));
    }
    println!("CRASH_BARRIER_{stage}");
    std::io::stdout().flush().unwrap();
    // A closed stdin is not a valid release: the parent must terminate us.
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    panic!("crash helper was not abruptly killed");
}

fn crash_at(stage: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "harness-process-recovery-{}-{stage}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_crash_helper", "--nocapture"])
        .env("HARNESS_CRASH_DB", &path)
        .env("HARNESS_CRASH_STAGE", stage)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = helper.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let barrier = format!("CRASH_BARRIER_{stage}");
    let mut observed = false;
    while let Some(line) = lines.next() {
        if line.unwrap().contains(&barrier) {
            observed = true;
            break;
        }
    }
    assert!(observed, "helper exited before {barrier}");
    helper.kill().unwrap();
    assert!(!helper.wait().unwrap().success(), "helper exited cleanly");
    path
}

#[test]
fn process_loss_before_completion_commit_leaves_no_head_or_answer() {
    let path = crash_at("before");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.agent(&child()).unwrap().unwrap().head_request, None);
    assert!(store.inbox(&root().0).unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[test]
fn process_loss_after_completion_commit_before_wake_recovers_typed_answer_once() {
    let path = crash_at("after");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.agent(&child()).unwrap().unwrap().head_request, Some(head()));
    let inbox = store.inbox(&root().0).unwrap();
    assert_eq!(inbox.len(), 1);
    let item = store.get_item(&inbox[0].item_hash).unwrap().unwrap();
    let decoded = PublishedAnswer::from_message_item(&item).unwrap();
    assert_eq!(decoded.result, json!({"kind":"completed","value":{"answer":42}}));
    assert_eq!(decoded.provenance.final_request, head());
    assert_eq!(decoded.provenance.seen_envelopes, vec![7]);
    assert_eq!(decoded.provenance.unseen_envelopes, vec![8]);
    assert_eq!(
        store
            .complete_agent_with_publication(&child(), None, &head(), Some((&root(), &answer())))
            .unwrap(),
        CompletionCommit::HeadMismatch
    );
    assert_eq!(store.inbox(&root().0).unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_file(path);
}
