//! Browser projection of native async Engine jobs.
//!
//! The Engine and JobScheduler remain authoritative. This ledger is only a
//! durable/UI projection: request provenance comes from `CallContext`, and a
//! provider future being dropped is terminal cancellation, never a release
//! that can later be changed to success.

use harness::{
    model::{CallId, RequestId},
    provider::{CallContext, Provider, ProviderError},
    server::{ToolJobRecord, ToolJobState},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, watch};

/// Control plane for the one deliberately blocked tool in the served async
/// scenario.  It is separate from Engine cancellation so releasing/cancelling
/// A never cancels unrelated work in the Engine run.
#[derive(Clone, Default)]
pub(crate) struct GateControl {
    released: Arc<AtomicBool>,
    notify: Arc<Notify>,
    call_id: Arc<Mutex<Option<CallId>>>,
    progress: Arc<Mutex<Option<Value>>>,
    progress_notify: Arc<Notify>,
    inputs: Arc<Mutex<Vec<Vec<harness::item::Item>>>>,
    requests: Arc<Mutex<Vec<harness::transport::ResponsesRequest>>>,
}

impl GateControl {
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    pub(crate) fn set_call_id(&self, call_id: CallId) {
        *self.call_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(call_id);
        self.notify.notify_one();
    }

    pub(crate) fn call_id(&self) -> Option<CallId> {
        self.call_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn record_progress(&self, event: Value) {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(event);
        self.progress_notify.notify_one();
    }

    pub(crate) async fn wait_until_progress(&self) -> Value {
        loop {
            let notified = self.progress_notify.notified();
            if let Some(event) = self
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return event;
            }
            notified.await;
        }
    }

    pub(crate) fn observe_input(&self, input: &[harness::item::Item]) {
        self.inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(input.to_vec());
    }

    pub(crate) fn observe_request(&self, request: &harness::transport::ResponsesRequest) {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request.clone());
    }

    #[cfg(test)]
    pub(crate) fn inputs(&self) -> Vec<Vec<harness::item::Item>> {
        self.inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn requests(&self) -> Vec<harness::transport::ResponsesRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) async fn wait_until_started(&self) -> CallId {
        loop {
            let notified = self.notify.notified();
            if let Some(call_id) = self.call_id() {
                return call_id;
            }
            notified.await;
        }
    }

    pub(crate) async fn wait(&self) {
        self.wait_until_started().await;
        while !self.released.load(Ordering::Acquire) {
            let notified = self.notify.notified();
            if self.released.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }
}

/// Shared by the deterministic provider, server command loop and snapshot
/// projection. Records are keyed by the original call id, not a generated
/// browser-command id.
#[derive(Clone)]
pub(crate) struct ToolJobs {
    records: Arc<Mutex<BTreeMap<String, ToolJobRecord>>>,
    persist: Option<Arc<dyn Fn(Vec<ToolJobRecord>) -> Result<(), String> + Send + Sync>>,
    delivery_revision: watch::Sender<u64>,
}

impl Default for ToolJobs {
    fn default() -> Self {
        Self {
            records: Arc::new(Mutex::new(BTreeMap::new())),
            persist: None,
            delivery_revision: watch::channel(0).0,
        }
    }
}

impl ToolJobs {
    fn epoch_ms() -> Option<i64> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
    }

    pub(crate) fn reopened_with_persistence(
        records: impl IntoIterator<Item = ToolJobRecord>,
        persist: impl Fn(Vec<ToolJobRecord>) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let mut jobs = Self::reopen(records);
        jobs.persist = Some(Arc::new(persist));
        jobs
    }

    fn persist(&self) -> Result<(), String> {
        if let Some(persist) = &self.persist {
            persist(self.records())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn start(
        &self,
        conversation_id: &str,
        request: Option<&RequestId>,
        call_id: &CallId,
        tool_name: &str,
    ) -> Result<(), String> {
        self.start_with_kind(conversation_id, request, call_id, tool_name, None)
    }

    fn start_with_kind(
        &self,
        conversation_id: &str,
        request: Option<&RequestId>,
        call_id: &CallId,
        tool_name: &str,
        tool_kind: Option<harness::item::ToolKind>,
    ) -> Result<(), String> {
        let record = ToolJobRecord {
            id: format!("tool/{conversation_id}/{}", call_id.0),
            conversation_id: conversation_id.to_owned(),
            request_id: request.map(|id| id.0.clone()).unwrap_or_default(),
            call_id: call_id.0.clone(),
            tool_name: tool_name.to_owned(),
            tool_kind,
            state: ToolJobState::Running,
            delivered: false,
            started_at_ms: Self::epoch_ms(),
            ended_at_ms: None,
            output: None,
        };
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if records.contains_key(&call_id.0) {
            return Err(format!(
                "tool call identity {} already exists; refusing to overwrite its history",
                call_id.0
            ));
        }
        records.insert(call_id.0.clone(), record);
        drop(records);
        self.persist()
    }

    pub(crate) fn settle(&self, call_id: &CallId, output: Value) -> Result<(), String> {
        let mut changed = false;
        if let Some(record) = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&call_id.0)
        {
            if record.state == ToolJobState::Running {
                record.output = Some(output);
                record.state = ToolJobState::Settled;
                record.ended_at_ms = Self::epoch_ms();
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Wait for the persisted output to appear in a later Engine input.
    /// Subscribe before inspecting the record: watch broadcasts every change
    /// and remembers it if delivery races the inspection.
    pub(crate) async fn wait_for_delivered(&self, call_id: &CallId) {
        let mut changes = self.delivery_revision.subscribe();
        loop {
            if self
                .records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&call_id.0)
                .is_some_and(|record| record.delivered)
            {
                return;
            }
            changes
                .changed()
                .await
                .expect("ToolJobs owns the delivery change sender");
        }
    }

    pub(crate) fn cancel(&self, call_id: &CallId) {
        let mut changed = false;
        if let Some(record) = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&call_id.0)
        {
            if record.state == ToolJobState::Running {
                record.state = ToolJobState::Cancelled;
                record.ended_at_ms = Self::epoch_ms();
                changed = true;
            }
        }
        if changed {
            let _ = self.persist();
        }
    }

    /// An output is delivered only when it occurs in a later full model input.
    /// The Engine/Store validate output kind against the durable invocation;
    /// this projection only observes either accepted wire output shape.
    pub(crate) fn observe_input(&self, input: &[harness::item::Item]) -> Result<(), String> {
        let delivered: std::collections::HashSet<&str> = input
            .iter()
            .filter(|item| {
                matches!(
                    item.0["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                )
            })
            .filter_map(|item| item.0["call_id"].as_str())
            .collect();
        let mut changed = false;
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        for record in records.values_mut() {
            if record.state == ToolJobState::Settled
                && !record.delivered
                && delivered.contains(record.call_id.as_str())
            {
                record.delivered = true;
                changed = true;
            }
        }
        drop(records);
        if changed {
            self.persist()?;
            self.delivery_revision.send_modify(|revision| {
                *revision = revision.wrapping_add(1);
            });
        }
        Ok(())
    }

    /// On process reopen, a running row cannot be resumed safely: provider
    /// side effects may have happened. Keep its persisted state until Engine's
    /// inherited-claim recovery has durably classified it.
    pub(crate) fn reopen(records: impl IntoIterator<Item = ToolJobRecord>) -> Self {
        let records = records
            .into_iter()
            .map(|record| (record.call_id.clone(), record));
        Self {
            records: Arc::new(Mutex::new(records.collect())),
            persist: None,
            delivery_revision: watch::channel(0).0,
        }
    }

    pub(crate) fn mark_interrupted_after_recovery(&self) -> Result<(), String> {
        let mut changed = false;
        for record in self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            if record.state == ToolJobState::Running {
                record.state = ToolJobState::Interrupted;
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    pub(crate) fn records(&self) -> Vec<ToolJobRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
}

/// Provider adapter that records the Engine's original call/request identity.
/// Drop is significant: JobScheduler cancellation aborts this future, and the
/// guard records that terminal outcome without allowing a late completion.
pub(crate) struct TrackedProvider<P> {
    pub(crate) inner: P,
    pub(crate) jobs: ToolJobs,
    pub(crate) conversation_id: String,
    pub(crate) store: Arc<Store>,
}

struct CancellationGuard {
    jobs: ToolJobs,
    call_id: CallId,
    completed: bool,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.jobs.cancel(&self.call_id);
        }
    }
}

impl<P: Provider> TrackedProvider<P> {
    async fn track_call<F>(
        &self,
        name: &str,
        context: &CallContext,
        call: F,
    ) -> Result<Value, ProviderError>
    where
        F: Future<Output = Result<Value, ProviderError>>,
    {
        let request = context.request.as_ref().ok_or_else(|| {
            ProviderError::Tool("tool job has no invocation request provenance".into())
        })?;
        let kind = self
            .store
            .tool_invocation_kind(request, &context.call_id)
            .map_err(|error| ProviderError::Tool(format!("scoped invocation kind: {error}")))?
            .ok_or_else(|| {
                ProviderError::Tool("tool job has no durable scoped invocation".into())
            })?;
        self.jobs
            .start_with_kind(
                &self.conversation_id,
                context.request.as_ref(),
                &context.call_id,
                name,
                Some(kind),
            )
            .map_err(ProviderError::Tool)?;
        let mut guard = CancellationGuard {
            jobs: self.jobs.clone(),
            call_id: context.call_id.clone(),
            completed: false,
        };
        let result = call.await;
        if let Ok(value) = &result {
            self.jobs
                .settle(&context.call_id, json!(value))
                .map_err(ProviderError::Tool)?;
        } else if let Err(error) = &result {
            self.jobs
                .settle(&context.call_id, json!({"error":error.to_string()}))
                .map_err(ProviderError::Tool)?;
        }
        guard.completed = true;
        result
    }
}

#[async_trait::async_trait]
impl<P: Provider> Provider for TrackedProvider<P> {
    async fn before_request(
        &self,
        plan: &harness::hooks::RequestPlan,
    ) -> harness::hooks::BeforeRequestResult {
        self.inner.before_request(plan).await
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
        self.inner.call(name, args).await
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.track_call(
            name,
            &context,
            self.inner.call_with_context(name, args, context.clone()),
        )
        .await
    }

    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.track_call(
            name,
            &context,
            self.inner
                .call_custom_with_context(name, input, context.clone()),
        )
        .await
    }

    fn tools(&self) -> Vec<Value> {
        self.inner.tools()
    }

    fn all_tools(&self) -> Vec<Value> {
        self.inner.all_tools()
    }

    async fn call_agent_verb(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.inner.call_agent_verb(name, args, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use harness::{
        engine::{Engine, EngineConfig, ResponsesTransport},
        item::Item,
        provider::Provider,
        store::Store,
        transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
        turn::JobScheduler,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Notify, watch};

    fn record(call_id: &str, state: ToolJobState, output: Option<Value>) -> ToolJobRecord {
        ToolJobRecord {
            id: format!("tool/conversation/{call_id}"),
            conversation_id: "conversation".into(),
            request_id: "request-9".into(),
            call_id: call_id.into(),
            tool_name: "gate".into(),
            tool_kind: None,
            state,
            delivered: false,
            started_at_ms: None,
            ended_at_ms: None,
            output,
        }
    }

    #[test]
    fn tool_job_timing_tracks_first_terminal_and_unknown_reopen() {
        let persisted = Arc::new(Mutex::new(Vec::<ToolJobRecord>::new()));
        let persisted_for_write = persisted.clone();
        let jobs = ToolJobs::reopened_with_persistence([], move |records| {
            *persisted_for_write.lock().unwrap() = records;
            Ok(())
        });
        let settled = CallId("settled".into());
        jobs.start("conversation/root", None, &settled, "echo")
            .unwrap();
        let started = jobs.records()[0].started_at_ms.expect("observed start");
        assert!(jobs.records()[0].ended_at_ms.is_none());
        jobs.settle(&settled, json!({"ok":true})).unwrap();
        let finished = jobs.records()[0].ended_at_ms.expect("observed end");
        assert!(finished >= started);
        jobs.cancel(&settled);
        assert_eq!(jobs.records()[0].ended_at_ms, Some(finished));
        assert_eq!(persisted.lock().unwrap()[0].ended_at_ms, Some(finished));

        let orphan = CallId("orphan".into());
        jobs.start("conversation/root", None, &orphan, "gate")
            .unwrap();
        let reopened = ToolJobs::reopen(jobs.records());
        reopened.mark_interrupted_after_recovery().unwrap();
        let interrupted = reopened
            .records()
            .into_iter()
            .find(|record| record.call_id == orphan.0)
            .unwrap();
        assert_eq!(interrupted.state, ToolJobState::Interrupted);
        assert!(interrupted.started_at_ms.is_some());
        assert!(interrupted.ended_at_ms.is_none());
    }

    #[test]
    fn cancel_is_terminal_and_reopen_waits_for_engine_recovery() {
        let jobs = ToolJobs::reopen([
            record("a", ToolJobState::Running, None),
            record("b", ToolJobState::Settled, Some(json!({"ok":true}))),
        ]);
        let before = jobs.records();
        assert_eq!(before[0].state, ToolJobState::Running);
        assert_eq!(before[1].output, Some(json!({"ok":true})));
        jobs.mark_interrupted_after_recovery().unwrap();
        assert_eq!(jobs.records()[0].state, ToolJobState::Interrupted);

        let jobs = ToolJobs::default();
        jobs.start(
            "conversation",
            Some(&RequestId("request-9".into())),
            &CallId("a".into()),
            "gate",
        )
        .unwrap();
        jobs.cancel(&CallId("a".into()));
        jobs.settle(&CallId("a".into()), json!({"late":true}))
            .unwrap();
        let cancelled = jobs.records().pop().unwrap();
        assert_eq!(cancelled.state, ToolJobState::Cancelled);
        assert_eq!(cancelled.output, None);
    }

    #[test]
    fn delivery_requires_original_call_output_in_full_input() {
        let jobs = ToolJobs::default();
        jobs.start(
            "conversation",
            Some(&RequestId("request-9".into())),
            &CallId("a".into()),
            "gate",
        )
        .unwrap();
        jobs.settle(&CallId("a".into()), json!({"ok":true}))
            .unwrap();
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"other","output":"ignored"
        }))])
        .unwrap();
        assert!(!jobs.records()[0].delivered);
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"a","output":"original"
        }))])
        .unwrap();
        assert!(jobs.records()[0].delivered);
    }

    #[tokio::test]
    async fn delivery_wait_barrier_requires_original_call_without_sleep() {
        let jobs = ToolJobs::default();
        let call_id = CallId("barrier".into());
        jobs.start(
            "conversation",
            Some(&RequestId("request-9".into())),
            &call_id,
            "echo",
        )
        .unwrap();
        jobs.settle(&call_id, json!("done")).unwrap();
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"other","output":"ignored"
        }))])
        .unwrap();
        assert!(!jobs.records()[0].delivered);
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"barrier","output":"done"
        }))])
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            jobs.wait_for_delivered(&call_id),
        )
        .await
        .expect("durable output delivery opens the barrier");
    }

    #[tokio::test]
    async fn delivery_broadcast_wakes_only_the_matching_call_waiter() {
        let jobs = ToolJobs::default();
        let a = CallId("wait-a".into());
        let b = CallId("wait-b".into());
        for call in [&a, &b] {
            jobs.start(
                "conversation",
                Some(&RequestId("request-9".into())),
                call,
                "echo",
            )
            .unwrap();
            jobs.settle(call, json!("done")).unwrap();
        }
        let a_wait = {
            let jobs = jobs.clone();
            let a = a.clone();
            tokio::spawn(async move { jobs.wait_for_delivered(&a).await })
        };
        let b_wait = {
            let jobs = jobs.clone();
            let b = b.clone();
            tokio::spawn(async move { jobs.wait_for_delivered(&b).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while jobs.delivery_revision.receiver_count() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both call waiters subscribed");
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"wait-b","output":"done"
        }))])
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), b_wait)
            .await
            .expect("B delivery must wake B's waiter")
            .expect("B waiter task");
        assert!(!a_wait.is_finished(), "B delivery cannot satisfy A");
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"function_call_output","call_id":"wait-a","output":"done"
        }))])
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), a_wait)
            .await
            .expect("A delivery must wake A's waiter")
            .expect("A waiter task");
    }

    #[test]
    fn custom_delivery_requires_original_output_and_survives_reopen() {
        let jobs = ToolJobs::default();
        let call_id = CallId("raw-custom".into());
        jobs.start(
            "conversation",
            Some(&RequestId("request-9".into())),
            &call_id,
            "run",
        )
        .unwrap();
        jobs.settle(&call_id, json!({"result":"done"})).unwrap();
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"custom_tool_call_output","call_id":"different","output":"ignored"
        }))])
        .unwrap();
        assert!(!jobs.records()[0].delivered);
        jobs.observe_input(&[harness::item::Item(json!({
            "type":"custom_tool_call_output","call_id":"raw-custom","output":"done"
        }))])
        .unwrap();
        let reopened = ToolJobs::reopen(jobs.records());
        let record = &reopened.records()[0];
        assert_eq!(record.state, ToolJobState::Settled);
        assert!(record.delivered);
        assert_eq!(record.output, Some(json!({"result":"done"})));
    }

    #[tokio::test]
    async fn tracked_jobs_take_kind_from_emitting_request_not_reused_call_id() {
        let store = Arc::new(Store::memory().unwrap());
        let shared_call = CallId("same-call-id".into());
        let cases = [
            (
                RequestId("function-request".into()),
                "/root/function",
                Item(
                    json!({"type":"function_call","call_id":"same-call-id","name":"echo","arguments":"{}"}),
                ),
                json!({}),
                harness::item::ToolKind::Function,
            ),
            (
                RequestId("custom-request".into()),
                "/root/custom",
                Item(
                    json!({"type":"custom_tool_call","call_id":"same-call-id","name":"echo","input":"raw\n雪"}),
                ),
                json!("raw\n雪"),
                harness::item::ToolKind::Custom,
            ),
        ];
        for (request, branch, item, args, expected) in cases {
            store
                .write_request(
                    &request,
                    None,
                    branch,
                    &[item],
                    harness::store::Usage::default(),
                )
                .unwrap();
            let jobs = ToolJobs::default();
            let tracked = TrackedProvider {
                inner: SequenceProvider,
                jobs: jobs.clone(),
                conversation_id: format!("conversation/{branch}"),
                store: store.clone(),
            };
            let (mut context, _progress) = CallContext::detached_for_test(
                harness::provider::JobHandle(format!("job/{branch}")),
                shared_call.clone(),
                harness::model::AgentPath(branch.into()),
            );
            context.request = Some(request.clone());
            match expected {
                harness::item::ToolKind::Function => {
                    tracked.call_with_context("echo", args, context).await
                }
                harness::item::ToolKind::Custom => {
                    tracked
                        .call_custom_with_context("echo", "raw\n雪".into(), context)
                        .await
                }
            }
            .expect("scoped invocation dispatch");
            let records = jobs.records();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].request_id, request.0);
            assert_eq!(records[0].call_id, shared_call.0);
            assert_eq!(records[0].tool_kind, Some(expected));
        }
    }

    #[derive(Clone)]
    struct OfflineAuth;
    impl Auth for OfflineAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Err(TransportError::Authentication)
        }
    }

    struct SequenceTransport {
        calls: AtomicUsize,
        inputs: Arc<Mutex<Vec<Vec<Item>>>>,
        third_request: Arc<Notify>,
    }

    #[async_trait]
    impl ResponsesTransport for SequenceTransport {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            self.inputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.input.clone());
            let turn = match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => vec![Item(json!({
                    "type":"function_call","call_id":"call-A","name":"gate","arguments":"{}","async":true
                }))],
                1 => vec![Item(json!({
                    "type":"function_call","call_id":"call-B","name":"echo","arguments":"{}","async":true
                }))],
                _ => {
                    self.third_request.notify_one();
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer",
                        "content":[{"type":"output_text","text":"complete"}]
                    }))]
                }
            };
            Ok(ResponsesTurn {
                response_id: format!("sequence-{}", self.calls.load(Ordering::SeqCst)),
                items: turn,
                usage: Usage::default(),
            })
        }
    }

    struct SequenceProvider;
    #[async_trait]
    impl Provider for SequenceProvider {
        async fn call(&self, name: &str, _args: Value) -> Result<Value, ProviderError> {
            match name {
                "gate" => std::future::pending().await,
                "echo" => Ok(json!("B output")),
                _ => Err(ProviderError::Tool("unexpected tool".into())),
            }
        }
        async fn call_custom_with_context(
            &self,
            name: &str,
            input: String,
            _context: CallContext,
        ) -> Result<Value, ProviderError> {
            if name == "echo" && input == "raw\n雪" {
                Ok(json!("B output"))
            } else {
                Err(ProviderError::Tool("unexpected custom tool".into()))
            }
        }
        fn tools(&self) -> Vec<Value> {
            ["gate", "echo"]
                .into_iter()
                .map(|name| {
                    json!({
                        "type":"function","name":name,"async":true,"parameters":{"type":"object","properties":{}}
                    })
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn wave18_async_demo_engine_sequence() {
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let third_request = Arc::new(Notify::new());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let jobs = ToolJobs::default();
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<OfflineAuth, TrackedProvider<SequenceProvider>, _>::with_transport(
            SequenceTransport {
                calls: AtomicUsize::new(0),
                inputs: inputs.clone(),
                third_request: third_request.clone(),
            },
            store.clone(),
            scheduler,
            Arc::new(TrackedProvider {
                inner: SequenceProvider,
                jobs: jobs.clone(),
                conversation_id: "async-sequence".into(),
                store,
            }),
            EngineConfig {
                instructions: "sequence test".into(),
                tools: vec![],
                model: "local".into(),
                effort: harness::model::Effort::Low,
                session_id: "async-sequence".into(),
                agent: harness::model::AgentPath("/root".into()),
            },
        );
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(
                        json!({"type":"message","role":"user","content":"start"}),
                    )],
                    cancel_rx,
                    tokio::sync::mpsc::unbounded_channel().1,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), third_request.notified())
            .await
            .expect("Engine did not issue third request");
        let observed = inputs.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            observed.len() >= 3,
            "expected A, B, and continuation requests"
        );
        for (index, request_input) in observed.iter().enumerate().skip(1) {
            assert!(
                request_input.iter().any(|item| {
                    item.0["type"] == "function_call" && item.0["call_id"] == "call-A"
                }),
                "request {index} omitted original unanswered A: {request_input:#?}"
            );
            assert!(
                !request_input.iter().any(|item| {
                    item.0["type"] == "function_call_output" && item.0["call_id"] == "call-A"
                }),
                "request {index} fabricated A output: {request_input:#?}"
            );
        }
        let latest = observed.last().expect("third request input");
        assert!(
            latest.iter().any(|item| {
                item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "call-B"
                    && item.0["output"] == "\"B output\""
            }),
            "third Engine input: {latest:#?}"
        );
        assert_eq!(
            latest
                .iter()
                .filter(|item| {
                    item.0["type"] == "function_call_output" && item.0["call_id"] == "call-B"
                })
                .count(),
            1,
            "B's settled output must be delivered exactly once in full input"
        );
        assert!(!latest.iter().any(|item| {
            item.0["type"] == "function_call_output" && item.0["call_id"] == "call-A"
        }));
        jobs.observe_input(latest).unwrap();
        let delivered_b = jobs
            .records()
            .into_iter()
            .find(|job| job.call_id == "call-B")
            .unwrap();
        assert_eq!(delivered_b.state, ToolJobState::Settled);
        assert!(delivered_b.delivered);
        assert!(!delivered_b.request_id.is_empty());
        assert_eq!(
            delivered_b.tool_kind,
            Some(harness::item::ToolKind::Function),
            "projection kind must come from B's persisted invocation"
        );
        drop(observed);
        cancel_tx.send(true).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), run)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let cancelled_a = jobs
            .records()
            .into_iter()
            .find(|job| job.call_id == "call-A")
            .unwrap();
        assert_eq!(cancelled_a.state, ToolJobState::Cancelled);
        assert_eq!(cancelled_a.output, None);
        assert_eq!(
            cancelled_a.tool_kind,
            Some(harness::item::ToolKind::Function),
            "cancellation must retain the original invocation kind"
        );
        assert!(!cancelled_a.request_id.is_empty());
    }

    #[tokio::test]
    async fn wave18_async_demo_cancel_and_release() {
        let jobs = ToolJobs::default();
        jobs.start(
            "conversation",
            Some(&RequestId("request-9".into())),
            &CallId("call-A".into()),
            "gate",
        )
        .unwrap();
        jobs.cancel(&CallId("call-A".into()));
        // A late completion/release is ignored after cancellation.
        jobs.settle(&CallId("call-A".into()), json!({"late":true}))
            .unwrap();
        let cancelled = jobs.records().pop().unwrap();
        assert_eq!(cancelled.state, ToolJobState::Cancelled);
        assert_eq!(cancelled.output, None);

        let reopened = ToolJobs::reopen([
            cancelled,
            record("call-B", ToolJobState::Settled, Some(json!({"kept":true}))),
            record("call-C", ToolJobState::Running, None),
        ]);
        assert!(
            reopened
                .records()
                .iter()
                .any(|job| { job.call_id == "call-C" && job.state == ToolJobState::Running })
        );
        reopened.mark_interrupted_after_recovery().unwrap();
        let records = reopened.records();
        assert!(records.iter().any(|job| {
            job.call_id == "call-B"
                && job.state == ToolJobState::Settled
                && job.output == Some(json!({"kept":true}))
        }));
        assert!(
            records
                .iter()
                .any(|job| { job.call_id == "call-C" && job.state == ToolJobState::Interrupted })
        );
        assert!(
            records
                .iter()
                .any(|job| { job.call_id == "call-A" && job.state == ToolJobState::Cancelled })
        );
    }
}
