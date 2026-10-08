//! Opt-in measurements of production history readers and scripted Engine rounds.
//! No provider traffic, compiler, compaction, or elapsed-time acceptance threshold.
use super::*;
use serde_json::Value;
use std::{path::PathBuf, sync::Mutex, time::Instant};

const REPETITIONS: usize = 8;

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "harness-history-perf-{}.sqlite",
            uuid::Uuid::new_v4()
        )))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

fn process_cpu_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // The fixture is Linux-only; this process clock includes the blocking workers.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) },
        0
    );
    (time.tv_sec as u64) * 1_000_000_000 + time.tv_nsec as u64
}

struct Meter(Instant, u64);
impl Meter {
    fn start() -> Self {
        Self(Instant::now(), process_cpu_ns())
    }
    fn finish(self) -> Value {
        json!({"wall_ns":self.0.elapsed().as_nanos(),"process_cpu_ns":process_cpu_ns()-self.1})
    }
}

fn message(worker: usize, ordinal: usize, payload_bytes: usize) -> Item {
    Item(
        json!({"type":"message","role":"user","content":format!("{worker}:{ordinal}:{}", "x".repeat(payload_bytes))}),
    )
}

struct Seed {
    store: Arc<Store>,
    head: RequestId,
    identity: ConversationIdentity,
    items: usize,
    history_bytes: usize,
}

fn seed(store: Arc<Store>, worker: usize, depth: usize, payload_bytes: usize) -> Seed {
    let identity = store.standalone_identity(AgentPath(format!("/root/c{worker}")));
    store.initialize_context_model(&identity, "test").unwrap();
    let mut parent = None;
    for ordinal in 0..depth {
        let head = RequestId(format!("seed-{worker}-{ordinal}"));
        store
            .write_request(
                &head,
                parent.as_ref(),
                &identity.actor().0,
                &[message(worker, ordinal, payload_bytes)],
                StoredUsage::default(),
            )
            .unwrap();
        if ordinal == 0 {
            store.set_effort(&head, crate::model::Effort::Low).unwrap();
        }
        parent = Some(head);
    }
    let head = parent.unwrap();
    let history = store.context_request_state(&head, &identity).unwrap();
    let items = history
        .history
        .iter()
        .map(|(_, _, item)| item)
        .collect::<Vec<_>>();
    assert_eq!(items.len(), depth + 1);
    let history_bytes = serde_json::to_vec(&items).unwrap().len();
    Seed {
        store,
        head,
        identity,
        items: items.len(),
        history_bytes,
    }
}

#[derive(Clone, Copy)]
enum Operation {
    CanonicalHistory,
    ProjectedHistory,
    Append,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::CanonicalHistory => "canonical_history",
            Self::ProjectedHistory => "projected_history",
            Self::Append => "append_one_item",
        }
    }
    async fn run(self, seed: &Seed, item: Option<Item>) -> Result<(), EngineError> {
        match self {
            Self::CanonicalHistory => {
                let history = load_history_window(seed.store.clone(), seed.head.clone()).await?;
                assert_eq!(history.items.len(), seed.items);
            }
            Self::ProjectedHistory => {
                let store = seed.store.clone();
                let head = seed.head.clone();
                let identity = seed.identity.clone();
                let history =
                    blocking(move || store.context_request_state(&head, &identity)).await?;
                assert_eq!(history.history.len(), seed.items);
            }
            Self::Append => {
                let item = item.unwrap();
                let store = seed.store.clone();
                let head = seed.head.clone();
                let hashes = blocking(move || store.append_items(&head, &[item])).await?;
                assert_eq!(hashes.len(), 1);
            }
        }
        Ok(())
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    Succeeded,
    SqliteBusy { extended_code: i32 },
    SqliteLocked { extended_code: i32 },
}

#[derive(serde::Serialize)]
struct Sample {
    wall_ns: u128,
    outcome: Outcome,
}

fn contention_refusal(error: EngineError) -> Outcome {
    match error {
        EngineError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(error, _))) => {
            match error.code {
                rusqlite::ErrorCode::DatabaseBusy => Outcome::SqliteBusy {
                    extended_code: error.extended_code,
                },
                rusqlite::ErrorCode::DatabaseLocked => Outcome::SqliteLocked {
                    extended_code: error.extended_code,
                },
                _ => panic!("unexpected SQLite failure: {error:?}"),
            }
        }
        error => panic!("unexpected measurement failure: {error:?}"),
    }
}

fn emit(mut row: Value, samples: Vec<Sample>, meter: Value) {
    let mut successes = samples
        .iter()
        .filter(|sample| matches!(sample.outcome, Outcome::Succeeded))
        .map(|sample| sample.wall_ns)
        .collect::<Vec<_>>();
    let mut refusals = samples
        .iter()
        .filter(|sample| !matches!(sample.outcome, Outcome::Succeeded))
        .map(|sample| sample.wall_ns)
        .collect::<Vec<_>>();
    successes.sort_unstable();
    refusals.sort_unstable();
    row["repetitions_per_conversation"] = json!(REPETITIONS);
    row["operation_count"] = json!(samples.len());
    row["successful_operation_count"] = json!(successes.len());
    row["refused_operation_count"] = json!(refusals.len());
    row["median_success_wall_ns"] = json!(successes.get(successes.len() / 2));
    row["max_success_wall_ns"] = json!(successes.last());
    row["median_refusal_wall_ns"] = json!(refusals.get(refusals.len() / 2));
    row["max_refusal_wall_ns"] = json!(refusals.last());
    row["attempts"] = json!(samples);
    row["cohort"] = meter;
    println!(
        "HARNESS_HISTORY_PERF {}",
        serde_json::to_string(&row).unwrap()
    );
}

async fn store_cohort(depth: usize, payload_bytes: usize, workers: usize, separate_handles: bool) {
    let database = Database::new();
    let store = Arc::new(Store::open(&database.0).unwrap());
    let seeds = (0..workers)
        .map(|worker| {
            let handle = if separate_handles {
                Arc::new(Store::open(&database.0).unwrap())
            } else {
                store.clone()
            };
            seed(handle, worker, depth, payload_bytes)
        })
        .collect::<Vec<_>>();
    let history_bytes = seeds
        .iter()
        .map(|seed| seed.history_bytes)
        .collect::<Vec<_>>();
    let topology = if separate_handles {
        "separate_connections_same_database"
    } else {
        "shared_connection"
    };
    // Appends run last so both history readers see the same immutable transcript.
    for operation in [
        Operation::CanonicalHistory,
        Operation::ProjectedHistory,
        Operation::Append,
    ] {
        let start = Arc::new(tokio::sync::Barrier::new(workers + 1));
        let mut tasks = Vec::new();
        for (worker, seed) in seeds.iter().enumerate() {
            let seed = Seed {
                store: seed.store.clone(),
                head: seed.head.clone(),
                identity: seed.identity.clone(),
                items: seed.items,
                history_bytes: seed.history_bytes,
            };
            let start = start.clone();
            let inputs = (0..REPETITIONS)
                .map(|ordinal| {
                    matches!(operation, Operation::Append)
                        .then(|| message(worker, depth + ordinal, payload_bytes))
                })
                .collect::<Vec<_>>();
            tasks.push(tokio::spawn(async move {
                let mut samples = Vec::new();
                start.wait().await;
                for item in inputs {
                    let started = Instant::now();
                    let result = operation.run(&seed, item).await;
                    let wall_ns = started.elapsed().as_nanos();
                    let outcome = match result {
                        Ok(()) => Outcome::Succeeded,
                        Err(error)
                            if separate_handles
                                && workers > 1
                                && matches!(operation, Operation::Append) =>
                        {
                            contention_refusal(error)
                        }
                        Err(error) => panic!("unexpected measurement failure: {error:?}"),
                    };
                    samples.push(Sample { wall_ns, outcome });
                }
                samples
            }));
        }
        let meter = Meter::start();
        start.wait().await;
        let mut samples = Vec::new();
        for task in tasks {
            samples.extend(task.await.unwrap());
        }
        let meter = meter.finish();
        let resulting_history_items = if matches!(operation, Operation::Append) {
            seeds
                .iter()
                .zip(samples.chunks_exact(REPETITIONS))
                .map(|(seed, attempts)| {
                    let successful_appends = attempts
                        .iter()
                        .filter(|sample| matches!(sample.outcome, Outcome::Succeeded))
                        .count();
                    let actual = seed
                        .store
                        .context_request_state(&seed.head, &seed.identity)
                        .unwrap()
                        .history
                        .len();
                    assert_eq!(actual, seed.items + successful_appends);
                    actual
                })
                .collect::<Vec<_>>()
        } else {
            seeds.iter().map(|seed| seed.items).collect()
        };
        emit(
            json!({"operation":operation.name(),"connections":topology,"conversations":workers,"lineage_depth":depth,"payload_bytes":payload_bytes,"initial_history_items":depth+1,"initial_history_json_bytes":history_bytes,"resulting_history_items":resulting_history_items,"mutex_wait_and_hold_ns":null,"boundary":"Engine blocking reader/append; wall includes blocking-pool dispatch; seed and JSON byte accounting excluded; separate-connection concurrent writer busy/locked refusal retained without retry"}),
            samples,
            meter,
        );
    }
}

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline measurement")
    }
}
struct NoTools;
#[async_trait::async_trait]
impl Provider for NoTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, crate::provider::ProviderError> {
        panic!("no tool calls in measurement")
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    fn all_tools(&self) -> Vec<Value> {
        vec![]
    }
}
struct Scripted(Arc<Mutex<Vec<ResponsesRequest>>>);
#[async_trait::async_trait]
impl ResponsesTransport for Scripted {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.0.lock().unwrap().push(request);
        Ok(ResponsesTurn {
            response_id: uuid::Uuid::new_v4().to_string(),
            items: vec![Item(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
            )],
            usage: Usage::default(),
        })
    }
}

async fn engine_cohort(depth: usize, payload_bytes: usize, workers: usize) {
    let database = Database::new();
    let store = Arc::new(Store::open(&database.0).unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seeds = (0..workers)
        .map(|worker| seed(store.clone(), worker, depth, payload_bytes))
        .collect::<Vec<_>>();
    let initial_bytes = seeds
        .iter()
        .map(|seed| seed.history_bytes)
        .collect::<Vec<_>>();
    let scheduler = Arc::new(JobScheduler::new(workers).unwrap());
    let start = Arc::new(tokio::sync::Barrier::new(workers + 1));
    let mut tasks = Vec::new();
    for (worker, seed) in seeds.into_iter().enumerate() {
        let requests = requests.clone();
        let scheduler = scheduler.clone();
        let start = start.clone();
        tasks.push(tokio::spawn(async move {
            let engine = Engine::<Offline, NoTools, _>::with_transport(
                Scripted(requests),
                seed.store,
                scheduler,
                Arc::new(NoTools),
                EngineConfig {
                    instructions: "offline measurement".into(),
                    tools: vec![],
                    model: "test".into(),
                    effort: crate::model::Effort::Low,
                    session_id: format!("perf-{worker}"),
                    agent: seed.identity.actor().clone(),
                },
            );
            let mut head = seed.head;
            let mut samples = Vec::new();
            start.wait().await;
            for ordinal in 0..REPETITIONS {
                let (_cancel, cancelled) = watch::channel(false);
                let (_incoming, envelopes) = tokio::sync::mpsc::unbounded_channel();
                let item = message(worker, depth + ordinal, payload_bytes);
                let started = Instant::now();
                let completion = engine
                    .run(Some(head), vec![item], cancelled, envelopes)
                    .await
                    .unwrap();
                samples.push(Sample {
                    wall_ns: started.elapsed().as_nanos(),
                    outcome: Outcome::Succeeded,
                });
                assert_eq!(completion.transcript.len(), depth + 1 + (ordinal + 1) * 2);
                head = completion.head_request;
            }
            samples
        }));
    }
    let meter = Meter::start();
    start.wait().await;
    let mut samples = Vec::new();
    for task in tasks {
        samples.extend(task.await.unwrap());
    }
    let meter = meter.finish();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), workers * REPETITIONS);
    let issued = requests.iter().map(|request| json!({"session":request.session_id,"items":request.input.len(),"history_json_bytes":serde_json::to_vec(&request.input).unwrap().len()})).collect::<Vec<_>>();
    emit(
        json!({"operation":"scripted_engine_round","connections":"shared_connection","conversations":workers,"initial_lineage_depth":depth,"payload_bytes":payload_bytes,"initial_history_json_bytes":initial_bytes,"issued_requests":issued,"mutex_wait_and_hold_ns":null,"boundary":"real standalone Engine+Store; scripted final response; no tools/network/auth/compiler/wire serializer; request bytes counted after timing"}),
        samples,
        meter,
    );
}

async fn measure_grid(depths: &[usize], payload_sizes: &[usize], conversations: &[usize]) {
    let source = std::env::var("HARNESS_PERF_SOURCE")
        .expect("record exact built source via HARNESS_PERF_SOURCE");
    let runtime_cost_trace_enabled =
        tracing::enabled!(target: "harness::runtime_cost", tracing::Level::DEBUG);
    println!(
        "HARNESS_HISTORY_PERF {}",
        json!({"schema":2,"source_revision_label":source,"pid":std::process::id(),"debug_assertions":cfg!(debug_assertions),"runtime_cost_trace_enabled":runtime_cost_trace_enabled,"lineage_depths":depths,"payload_sizes":payload_sizes,"conversations":conversations,"expected_measurement_rows":depths.len()*payload_sizes.len()*conversations.len()*7,"expected_store_attempts":depths.len()*payload_sizes.len()*conversations.iter().sum::<usize>()*REPETITIONS*6,"expected_engine_rounds":depths.len()*payload_sizes.len()*conversations.iter().sum::<usize>()*REPETITIONS,"fixture":"ordinary editable messages; WAL/NORMAL durable Store; seeded histories warmed once; successful and refused attempts separate; no retry or timing acceptance threshold"})
    );
    for &depth in depths {
        for &payload_bytes in payload_sizes {
            for &workers in conversations {
                store_cohort(depth, payload_bytes, workers, false).await;
                store_cohort(depth, payload_bytes, workers, true).await;
                engine_cohort(depth, payload_bytes, workers).await;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bounded performance fixture; retain emitted JSON and executable identity"]
async fn store_and_engine_transcript_scaling() {
    measure_grid(&[1, 32, 128], &[256, 4096], &[1, 4]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in protocol smoke control before the complete scaling matrix"]
async fn store_and_engine_transcript_scaling_smoke() {
    // Exercise both connection topologies, all readers, concurrent writer
    // refusals and real Engine rounds without claiming a scaling comparison.
    measure_grid(&[32], &[4096], &[4]).await;
}
