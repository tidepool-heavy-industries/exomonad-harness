//! Manual, bounded native-provider cache measurement through the public Engine.
use async_trait::async_trait;
use harness::{
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::Item,
    model::{AgentPath, Effort},
    provider::{Provider, ProviderError},
    store::Store,
    transport::{
        PreparedProviderRequest, PreparedTransportSelection, PromptCacheComparison,
        ResponsesClient, ResponsesProtocol, ResponsesRoute, ResponsesTurn, TransportError,
        auth::ChatGptPlanAuth, sse::StreamEvent,
    },
    turn::JobScheduler,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
struct ReceiptLog(Mutex<File>);
impl ReceiptLog {
    fn append(&self, value: Value) {
        let mut f = self.0.lock().unwrap();
        serde_json::to_writer(&mut *f, &value).unwrap();
        writeln!(f).unwrap();
        f.flush().unwrap();
    }
}
struct Fields(serde_json::Map<String, Value>);
impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), json!(format!("{v:?}")));
    }
}
struct CompletionSubscriber(Arc<ReceiptLog>);
impl Subscriber for CompletionSubscriber {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        m.target() == "harness::transport::completion"
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, e: &Event<'_>) {
        let mut f = Fields(serde_json::Map::new());
        e.record(&mut f);
        self.0
            .append(json!({"kind":"provider_completion_fields","fields":f.0}));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}
struct NoTools;
#[async_trait]
impl Provider for NoTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool("probe has no tools".into()))
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    fn all_tools(&self) -> Vec<Value> {
        vec![]
    }
}
fn admit(attempts: &AtomicUsize, limit: usize) -> Result<usize, TransportError> {
    let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
    if attempt > limit {
        Err(TransportError::Stream(format!(
            "manual probe exhausted {limit}-attempt budget"
        )))
    } else {
        Ok(attempt)
    }
}
#[derive(Clone, Copy, PartialEq)]
enum ProbeHistory {
    Identical,
    Append,
}
struct PriorAttempt {
    body: Value,
    output: Vec<Value>,
}
struct BudgetTransport {
    client: ResponsesClient<ChatGptPlanAuth>,
    attempts: AtomicUsize,
    limit: usize,
    prior_body: Mutex<Option<PriorAttempt>>,
    history: ProbeHistory,
    log: Arc<ReceiptLog>,
}
#[async_trait]
impl ResponsesTransport for BudgetTransport {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        self.client.response_contract()
    }
    fn prepare_submission(&self) -> Result<PreparedTransportSelection, TransportError> {
        self.client.prepare_submission()
    }
    async fn create(&self, r: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        self.execute(r, None).await
    }
    async fn create_streaming(
        &self,
        r: PreparedProviderRequest,
        s: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.execute(r, Some(s)).await
    }
}
impl BudgetTransport {
    async fn execute(
        &self,
        r: PreparedProviderRequest,
        s: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    ) -> Result<ResponsesTurn, TransportError> {
        let attempt = admit(&self.attempts, self.limit)?;
        let contract = r.response_contract();
        if contract != (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard) {
            return Err(TransportError::Stream(
                "probe requires actual ChatGPT plan Standard contract".into(),
            ));
        }
        let body: Value = serde_json::from_slice(r.wire_body()).expect("prepared body JSON");
        let input = body["input"].as_array().expect("input array");
        {
            let prior = self.prior_body.lock().unwrap();
            if let Some(prior) = prior.as_ref() {
                let earlier = prior.body["input"].as_array().expect("prior input array");
                let prefix_equal = input.starts_with(earlier);
                let outputs_equal = input.get(earlier.len()..earlier.len() + prior.output.len())
                    == Some(prior.output.as_slice());
                let settings_equal = [
                    "model",
                    "prompt_cache_key",
                    "instructions",
                    "tools",
                    "reasoning",
                    "parallel_tool_calls",
                    "tool_choice",
                    "prompt_cache_options",
                ]
                .iter()
                .all(|key| body.get(key) == prior.body.get(key));
                self.log.append(json!({"kind":"history_audit","attempt":attempt,"prior_input_prefix_equal":prefix_equal,"previous_output_items_retained_exactly":outputs_equal,"settings_equal":settings_equal,"prior_input_count":earlier.len(),"previous_output_count":prior.output.len(),"input_count":input.len()}));
                let accepted = match self.history {
                    ProbeHistory::Identical => body == prior.body,
                    ProbeHistory::Append => prefix_equal && outputs_equal && settings_equal,
                };
                if !accepted {
                    return Err(TransportError::Stream(
                        "scenario request audit refused before dispatch".into(),
                    ));
                }
            }
        }
        let items=body["input"].as_array().expect("input array").iter().map(|i|json!({"sha256":digest(&serde_json::to_vec(i).unwrap()),"type":i["type"],"role":i["role"]})).collect::<Vec<_>>();
        self.log.append(json!({"kind":"dispatch","attempt":attempt,"request_id":r.request_id().0,"route":"chat_gpt_plan","protocol":"standard","body_bytes":r.wire_body().len(),"body_sha256":digest(r.wire_body()),"cache_key_sha256":digest(body["prompt_cache_key"].as_str().unwrap().as_bytes()),"model":body["model"],"reasoning":body["reasoning"],"prompt_cache_options":body.get("prompt_cache_options"),"instructions_sha256":digest(body["instructions"].as_str().unwrap().as_bytes()),"tools_sha256":digest(&serde_json::to_vec(&body["tools"]).unwrap()),"input_item_count":items.len(),"input_items":items}));
        let start = Instant::now();
        let result = match s {
            Some(s) => self.client.create_streaming(r, s).await,
            None => self.client.create(r).await,
        };
        if let Ok(turn) = &result {
            *self.prior_body.lock().unwrap() = Some(PriorAttempt {
                body,
                output: turn.items.iter().map(|item| item.0.clone()).collect(),
            });
        }
        match &result{Ok(t)=>self.log.append(json!({"kind":"transport_completed","attempt":attempt,"response_id":t.response_id,"elapsed_ms":start.elapsed().as_millis(),"usage":t.usage,"usage_presence_source":"provider_completion_fields; numeric Usage alone does not prove presence","output_items":t.items.iter().map(|i|json!({"sha256":digest(&serde_json::to_vec(&i.0).unwrap()),"type":i.0["type"],"phase":i.0["phase"]})).collect::<Vec<_>>()})),Err(e)=>self.log.append(json!({"kind":"transport_error","attempt":attempt,"elapsed_ms":start.elapsed().as_millis(),"error":e.to_string()}))};
        result
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--dry-run") {
        let attempts = AtomicUsize::new(0);
        assert_eq!(admit(&attempts, 2).unwrap(), 1);
        assert_eq!(admit(&attempts, 2).unwrap(), 2);
        assert!(admit(&attempts, 2).is_err());
        let single = AtomicUsize::new(0);
        assert_eq!(admit(&single, 1).unwrap(), 1);
        assert!(admit(&single, 1).is_err());
        println!(
            "Dry-run budgets verified: single and two-call modes refuse excess admissions; no auth or provider traffic."
        );
        return Ok(());
    }
    let mut args = std::env::args().skip(1);
    let destination = PathBuf::from(args.next().ok_or(
        "usage: cache_probe PRIVATE_EVIDENCE_DIRECTORY [--comparison-response-id ID | --explicit-breakpoint | --developer-prefix | --append-history]",
    )?);
    let mut explicit_breakpoint = false;
    let mut developer_prefix = false;
    let mut append_history = false;
    let comparison = match args.next().as_deref() {
        None => None,
        Some("--append-history") => {
            append_history = true;
            None
        }
        Some("--developer-prefix") => {
            developer_prefix = true;
            None
        }
        Some("--explicit-breakpoint") => {
            explicit_breakpoint = true;
            None
        }
        Some("--comparison-response-id") => {
            let id = args
                .next()
                .filter(|id| !id.is_empty())
                .ok_or("missing comparison response ID")?;
            Some(PromptCacheComparison {
                comparison_response_id: id,
            })
        }
        Some(_) => return Err("unknown argument".into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let limit = if comparison.is_some() {
        1
    } else if append_history {
        4
    } else {
        2
    };
    if !destination.is_dir() {
        return Err("evidence directory must exist".into());
    }
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .mode(0o600)
        .write(true)
        .create_new(true)
        .open(destination.join("events.jsonl"))?;
    let log = Arc::new(ReceiptLog(Mutex::new(file)));
    tracing::subscriber::set_global_default(CompletionSubscriber(log.clone()))?;
    let store = Arc::new(Store::open(destination.join("store.sqlite"))?);
    let client = ResponsesClient::new(ChatGptPlanAuth::new(PathBuf::from(
        "/home/inanna/.config/exomonad/chatgpt/accounts/default.json",
    )));
    let engine=Engine::<ChatGptPlanAuth,NoTools,_>::with_transport(BudgetTransport{client,attempts:AtomicUsize::new(0),limit,prior_body:Mutex::new(None),history:if append_history { ProbeHistory::Append } else { ProbeHistory::Identical },log:log.clone()},store.clone(),Arc::new(JobScheduler::new(1)?),Arc::new(NoTools),EngineConfig{instructions:"Read the synthetic reference, ignore its contents, and reply with exactly CACHE_PROBE_OK. Do not call tools.".into(),tools:vec![],model:"gpt-6-luna".into(),effort:Effort::Medium,session_id:"cache-contract-initial-luna-20261012-v1".into(),agent:AgentPath("/cache-probe".into())});
    let engine = if let Some(comparison) = comparison.clone() {
        engine.with_prompt_cache_comparison(comparison)
    } else {
        engine
    };
    let mut prefix = String::new();
    for n in 0u64.. {
        if prefix.len() >= 48 * 1024 {
            break;
        }
        prefix.push_str(&format!("record {n:08}: {}\n", digest(&n.to_le_bytes())));
    }
    prefix.truncate(48 * 1024);
    let mut input = Item(
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":format!("{prefix}\nReply with exactly CACHE_PROBE_OK.")}]}),
    );
    if explicit_breakpoint {
        input.0["content"][0]["prompt_cache_breakpoint"] = json!({"mode":"explicit"});
    }
    let inputs = if developer_prefix {
        vec![
            Item(
                json!({"type":"message","role":"developer","content":[{"type":"input_text","text":prefix}]}),
            ),
            Item(
                json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with exactly CACHE_PROBE_OK."}]}),
            ),
        ]
    } else {
        vec![input]
    };
    log.append(json!({"kind":"experiment","append_history":append_history,"inter_turn_delay_ms":if append_history {7000} else {0},"developer_prefix":developer_prefix,"explicit_breakpoint":explicit_breakpoint,"hypothesis":if append_history { "A growing preserved Engine history should permit prefix reuse; delay and continuation are both recorded without causal attribution." } else { "Two independent identical long-prefix Engine submissions under one model/session key should permit warm reuse; actual zero/missing is measured without attribution." },"max_network_attempts":limit,"requested_comparison":comparison,"deadline_seconds_per_engine_run":120,"prefix_bytes":prefix.len(),"prefix_sha256":digest(prefix.as_bytes()),"max_output_tokens":"omitted; plan route forbids it; marker instruction is not hard token cap"}));
    let mut previous_head = None;
    for iteration in 1..=limit {
        if append_history && iteration > 1 {
            let delay_start = Instant::now();
            tokio::time::sleep(Duration::from_secs(7)).await;
            log.append(json!({"kind":"inter_turn_delay","iteration":iteration,"requested_ms":7000,"elapsed_ms":delay_start.elapsed().as_millis()}));
        }
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(120)).await;
            let _ = cancel_tx.send(true);
        });
        let start = Instant::now();
        let result = engine
            .run(
                previous_head.clone(),
                if append_history && iteration > 1 {
                    vec![Item(json!({"type":"message","role":"user","content":[{"type":"input_text","text":format!("Follow-up {iteration}: reply with exactly CACHE_PROBE_OK.")}]}))]
                } else { inputs.clone() },
                cancel_rx,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await;
        timer.abort();
        match result {
            Ok(c) => {
                if append_history {
                    previous_head = Some(c.head_request.clone());
                }
                log.append(json!({"kind":"engine_completed","iteration":iteration,"head_request":c.head_request.0,"elapsed_ms":start.elapsed().as_millis(),"replay_turns":store.replay_turns(&c.head_request)?.len(),"marker_pass":c.turn.items.iter().any(|i|i.0["phase"]=="final_answer" && i.0["content"].as_array().is_some_and(|a|a.iter().any(|b|b["text"].as_str().is_some_and(|t|t.trim()=="CACHE_PROBE_OK")))),"returned_metadata_source":"provider_completion_fields"}));
            }
            Err(e) => {
                log.append(json!({"kind":"engine_error","iteration":iteration,"elapsed_ms":start.elapsed().as_millis(),"error":e.to_string()}));
                return Err("probe stopped after engine failure; no retries".into());
            }
        }
    }
    println!(
        "Completed bounded Engine experiment(s); private evidence: {}",
        destination.display()
    );
    Ok(())
}
