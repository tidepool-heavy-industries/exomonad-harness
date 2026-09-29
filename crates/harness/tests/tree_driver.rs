use async_trait::async_trait;
use harness::{
    agent_runtime::StoreAgentToolService,
    agents::{AgentToolService, Contract, SpawnSource},
    engine::EngineCompletion,
    item::Item,
    mailbox::Envelope,
    model::{AgentPath, RequestId},
    store::Store,
    tree_driver::{Driver, EngineFactory},
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

struct DeterministicFactory {
    started: mpsc::UnboundedSender<String>,
}

#[async_trait]
impl EngineFactory for DeterministicFactory {
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
        self.started
            .send(agent.0.clone())
            .map_err(|error| error.to_string())?;
        while !*cancel.borrow() {
            cancel.changed().await.map_err(|error| error.to_string())?;
        }
        Err("cancelled".into())
    }
}

fn contract() -> Contract {
    Contract {
        clauses: vec![],
        acceptance: vec![],
        owned: vec![],
        must_not: vec![],
        introduces: vec![],
        consumes: vec![],
        boundaries: vec![],
        reply: None,
    }
}

/// This independent harness-library caller owns one supervisor for a durable
/// root and two children; shutdown cancels and joins all three workers.
#[tokio::test]
async fn second_caller_supervises_root_and_two_children_and_joins_shutdown() {
    let store = Arc::new(Store::memory().unwrap());
    let root = AgentPath("/root".into());
    let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let driver = Driver::new(
        store.clone(),
        service.clone(),
        Arc::new(DeterministicFactory {
            started: started_tx,
        }),
    );
    driver.start(vec![]).await.unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(2), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, "/root");
    for child in ["one", "two"] {
        service
            .spawn_agent(&root, child, SpawnSource::Prompt, contract())
            .await
            .unwrap();
    }
    let mut agents = vec![first];
    for _ in 0..2 {
        agents.push(
            tokio::time::timeout(std::time::Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    agents.sort();
    assert_eq!(agents, ["/root", "/root/one", "/root/two"]);
    assert_eq!(driver.active_agent_count().await, 3);

    driver.shutdown().await.unwrap();
    assert_eq!(driver.active_agent_count().await, 0);
}
