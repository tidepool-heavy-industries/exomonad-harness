//! A process is killed after Store commits a typed child answer but before any
//! Driver wake. A new Driver must discover it from the file Store.
use crate::driver::{Driver, EngineFactory};
use async_trait::async_trait;
use harness::{
    agent_runtime::StoreAgentToolService,
    engine::EngineCompletion,
    item::Item,
    lifecycle::{CompletionCommit, CompletionProvenance, PublishedAnswer},
    mailbox::Envelope,
    model::{AgentPath, RequestId},
    store::Store,
};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
};
use tokio::sync::{mpsc, watch};

fn root() -> AgentPath {
    AgentPath("/root".into())
}
fn child() -> AgentPath {
    AgentPath("/root/child".into())
}
fn head() -> RequestId {
    RequestId("committed-child-head".into())
}
fn answer() -> Item {
    PublishedAnswer {
        sender: child().0,
        result: json!({"kind":"completed","value":{"answer":42}}),
        provenance: CompletionProvenance {
            final_request: head(),
            seen_envelopes: vec![],
            unseen_envelopes: vec![],
        },
    }
    .to_message_item()
    .unwrap()
}

#[test]
fn process_restart_helper() {
    let Ok(path) = std::env::var("HARNESS_DRIVER_CRASH_DB") else {
        return;
    };
    let store = Store::open(path).unwrap();
    store
        .admit_agent(&root(), None, None, &json!({}), &json!({}))
        .unwrap();
    store
        .admit_agent(&child(), Some(&root()), None, &json!({}), &json!({}))
        .unwrap();
    store.create_request(&head(), None, &child().0).unwrap();
    assert!(matches!(
        store
            .complete_agent_with_publication(&child(), None, &head(), Some((&root(), &answer())))
            .unwrap(),
        CompletionCommit::Committed {
            envelope_id: Some(_)
        }
    ));
    println!("COMMITTED_BEFORE_WAKE");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    panic!("helper should be killed, not shut down");
}

fn kill_after_commit(path: &PathBuf) {
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process_restart_tests::process_restart_helper",
            "--nocapture",
        ])
        .env("HARNESS_DRIVER_CRASH_DB", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = helper.stdout.take().unwrap();
    let barrier = BufReader::new(stdout)
        .lines()
        .take_while(Result::is_ok)
        .any(|line| line.unwrap().contains("COMMITTED_BEFORE_WAKE"));
    assert!(barrier, "helper did not reach post-commit barrier");
    helper.kill().unwrap();
    assert!(!helper.wait().unwrap().success(), "helper exited cleanly");
}

struct RestartFactory {
    store: Arc<Store>,
    started: mpsc::UnboundedSender<(AgentPath, bool)>,
}

#[async_trait]
impl EngineFactory for RestartFactory {
    type Engine = AgentPath;

    async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
        Ok(agent)
    }

    async fn run(
        &self,
        agent: &Self::Engine,
        _head: Option<RequestId>,
        _initial: Vec<Item>,
        mut cancel: watch::Receiver<bool>,
        _inbox: mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, String> {
        let typed_answer_found = self
            .store
            .unread(&agent.0)
            .map_err(|e| e.to_string())?
            .into_iter()
            .any(|entry| {
                self.store
                    .get_item(&entry.item_hash)
                    .ok()
                    .flatten()
                    .and_then(|item| PublishedAnswer::from_message_item(&item).ok())
                    .is_some_and(|answer| {
                        answer.result == json!({"kind":"completed","value":{"answer":42}})
                            && answer.provenance.final_request == head()
                    })
            });
        self.started
            .send((agent.clone(), typed_answer_found))
            .map_err(|e| e.to_string())?;
        while !*cancel.borrow() {
            cancel.changed().await.map_err(|e| e.to_string())?;
        }
        Err("cancelled by test shutdown".into())
    }
}

#[tokio::test]
async fn process_restart_driver_discovers_committed_answer_without_wake() {
    let path = std::env::temp_dir().join(format!(
        "harness-driver-process-restart-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    kill_after_commit(&path);
    let store = Arc::new(Store::open(&path).unwrap());
    assert_eq!(store.inbox(&root().0).unwrap().len(), 1);
    let service = Arc::new(StoreAgentToolService::new(store.clone(), root()));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let driver = Driver::new(
        store.clone(),
        service,
        Arc::new(RestartFactory {
            store: store.clone(),
            started: started_tx,
        }),
    );
    driver.start(Vec::new()).await.unwrap();
    let (agent, saw_typed_answer) =
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx.recv())
            .await
            .expect("explicit root-start barrier")
            .expect("root start reported");
    assert_eq!(agent, root());
    assert!(
        saw_typed_answer,
        "restarted Driver finds typed durable answer"
    );
    assert_eq!(store.inbox(&root().0).unwrap().len(), 1);
    assert_eq!(
        store.agent(&child()).unwrap().unwrap().head_request,
        Some(head())
    );
    driver.shutdown().await.unwrap();
    assert!(
        started_rx.try_recv().is_err(),
        "child with committed head and no unread work was not replayed"
    );
    drop(driver);
    // A second startup after joined shutdown still sees the one committed
    // answer, never republishes it or admits the child a second time.
    let service = Arc::new(StoreAgentToolService::new(store.clone(), root()));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let driver = Driver::new(
        store.clone(),
        service,
        Arc::new(RestartFactory {
            store: store.clone(),
            started: started_tx,
        }),
    );
    driver.start(Vec::new()).await.unwrap();
    let (agent, saw_typed_answer) =
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx.recv())
            .await
            .expect("second root-start barrier")
            .expect("second root start reported");
    assert_eq!(agent, root());
    assert!(saw_typed_answer);
    driver.shutdown().await.unwrap();
    assert!(started_rx.try_recv().is_err(), "no duplicate child run");
    assert_eq!(store.inbox(&root().0).unwrap().len(), 1);
    assert_eq!(
        store.agent(&child()).unwrap().unwrap().head_request,
        Some(head())
    );
    drop(driver);
    drop(store);
    let _ = std::fs::remove_file(path);
}
