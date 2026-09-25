//! Driver restart and cancellation tests.
use super::*;

/// A restarted Driver must reconstruct its inbox from the durable Store, even
/// when the notification which originally announced the row predates it.
struct RecoveryDiscoveryFactory {
    started: tokio::sync::mpsc::UnboundedSender<(String, bool)>,
}

#[async_trait]
impl EngineFactory for RecoveryDiscoveryFactory {
    type Engine = AgentPath;

    async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
        Ok(agent)
    }

    async fn run(
        &self,
        engine: &Self::Engine,
        _head: Option<RequestId>,
        _initial: Vec<Item>,
        mut cancel: watch::Receiver<bool>,
        mut inbox: mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, String> {
        let durable_message_was_seeded = inbox.try_recv().is_ok();
        self.started
            .send((engine.0.clone(), durable_message_was_seeded))
            .map_err(|error| error.to_string())?;
        while !*cancel.borrow() {
            cancel.changed().await.map_err(|error| error.to_string())?;
        }
        Err("cancelled".into())
    }
}

#[tokio::test]
async fn recovery_discovers_durable_inbox_without_a_live_wake() {
    let path = std::env::temp_dir().join(format!(
        "harness-driver-recovery-{}.sqlite",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = AgentPath("/root".into());
    {
        let store = Store::open(&path).unwrap();
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .add_envelope(
                "/root",
                "/root",
                "AtBoundary",
                &Item(json!({"type":"message","text":"durable answer"})),
                None,
            )
            .unwrap();
        assert_eq!(store.unread("/root").unwrap().len(), 1);
    } // close the first process's Store before opening the restarted Driver

    let store = Arc::new(Store::open(&path).unwrap());
    let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let driver = Driver::new(
        store,
        service,
        Arc::new(RecoveryDiscoveryFactory {
            started: started_tx,
        }),
    );
    // The service subscribes only after the persisted envelope was written;
    // this barrier proves discovery comes from the Store scan, not a wake.
    driver.start(Vec::new()).await.unwrap();
    let (agent, saw_durable_message) =
        tokio::time::timeout(std::time::Duration::from_secs(2), started_rx.recv())
            .await
            .expect("explicit engine-start barrier")
            .expect("engine reports startup");
    assert_eq!(agent, "/root");
    assert!(
        saw_durable_message,
        "recovered run receives persisted inbox row"
    );
    driver.shutdown().await.unwrap();
    drop(driver);
    let _ = std::fs::remove_file(path);
}
