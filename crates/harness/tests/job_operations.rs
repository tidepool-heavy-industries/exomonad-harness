use async_trait::async_trait;
use harness::{
    agents::{
        AgentInvocation, AgentToolService, AgentVerbError, Contract, JobVerbError, SpawnSource,
    },
    model::{AgentPath, CallId, Effort, RequestId},
    provider::{CallContext, JobHandle, Provider, ProviderError},
    turn::{JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingService {
    invocation: Mutex<Option<AgentInvocation>>,
    messages: Mutex<Vec<(AgentPath, AgentPath, String)>>,
}

#[async_trait]
impl AgentToolService for RecordingService {
    async fn spawn_agent(
        &self,
        _: &AgentPath,
        _: &str,
        _: SpawnSource,
        _: Contract,
    ) -> Result<Value, AgentVerbError> {
        Ok(json!({"spawned":true}))
    }
    async fn spawn_agent_from_invocation(
        &self,
        _: &AgentPath,
        _: &str,
        _: SpawnSource,
        _: Contract,
        invocation: Option<&AgentInvocation>,
    ) -> Result<Value, AgentVerbError> {
        *self.invocation.lock().unwrap() = invocation.cloned();
        Ok(json!({"spawned":true}))
    }
    async fn send_message(
        &self,
        sender: &AgentPath,
        target: AgentPath,
        message: String,
    ) -> Result<Value, AgentVerbError> {
        self.messages
            .lock()
            .unwrap()
            .push((sender.clone(), target, message));
        Ok(json!({"accepted":true}))
    }
    async fn followup_task(
        &self,
        _: &AgentPath,
        _: AgentPath,
        _: Contract,
    ) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
    async fn wait_agent(&self, _: &AgentPath) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
    async fn checkpoint(&self, _: &AgentPath, _: String) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
    async fn set_effort(&self, _: &AgentPath, _: Effort) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
    async fn list_agents(
        &self,
        _: &AgentPath,
        _: Option<AgentPath>,
    ) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
    async fn interrupt_agent(&self, _: &AgentPath, _: AgentPath) -> Result<Value, AgentVerbError> {
        Ok(Value::Null)
    }
}

struct JobProvider(Option<Arc<RecordingService>>);

#[async_trait]
impl Provider for JobProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool("unexpected plain call".into()))
    }
    async fn call_with_context(
        &self,
        _: &str,
        _: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        if context.request.is_some() {
            assert!(matches!(
                context.verbs.origin(),
                Some(harness::agents::AgentOperationOrigin::Job { .. })
            ));
        } else {
            assert_eq!(context.verbs.origin(), None);
        }
        context
            .verbs
            .spawn_agent(
                "worker",
                SpawnSource::Here,
                Contract {
                    clauses: vec![],
                    acceptance: vec![],
                    owned: vec![],
                    must_not: vec![],
                    introduces: vec![],
                    consumes: vec![],
                    boundaries: vec![],
                    reply: None,
                },
            )
            .await
            .map_err(|e| ProviderError::Tool(e.to_string().into()))?;
        context
            .verbs
            .send_message(AgentPath("/root/worker/child".into()), "hello")
            .await
            .map_err(|e| ProviderError::Tool(e.to_string().into()))?;
        let denied = context
            .verbs
            .send_message(AgentPath("/root/sibling".into()), "no")
            .await;
        assert!(matches!(denied, Err(JobVerbError::Refused(_))));
        Ok(json!({"done":true}))
    }
    fn job_agent_service(&self) -> Option<Arc<dyn AgentToolService>> {
        self.0
            .clone()
            .map(|service| service as Arc<dyn AgentToolService>)
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn job_verbs_preserve_scheduler_origin_and_restrict_targets() {
    let service = Arc::new(RecordingService::default());
    let jobs = JobScheduler::new(1).unwrap();
    let request = RequestId("request-1".into());
    let call_id = CallId("call-1".into());
    jobs.start_for_agent(
        Arc::new(JobProvider(Some(service.clone()))),
        AgentPath("/root/worker".into()),
        Some(request.clone()),
        call_id.clone(),
        "cell".into(),
        Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), jobs.wait(&call_id))
            .await
            .expect("job operation timed out")
            .unwrap(),
        JobOutput::Completed(Ok(json!({"done":true})))
    );
    assert_eq!(
        service.invocation.lock().unwrap().as_ref(),
        Some(&AgentInvocation { request, call_id })
    );
    let invocation = service.invocation.lock().unwrap().clone().unwrap();
    assert_eq!(
        harness::agents::model_dispatch_origin(Some(&invocation)),
        Some(harness::agents::AgentOperationOrigin::ModelDispatch {
            request: invocation.request,
            call_id: invocation.call_id,
        })
    );
    assert_eq!(
        service.messages.lock().unwrap().as_slice(),
        &[(
            AgentPath("/root/worker".into()),
            AgentPath("/root/worker/child".into()),
            "hello".into(),
        )]
    );
}

#[tokio::test]
async fn job_verbs_refuse_when_request_identity_or_backend_is_missing() {
    for (provider, call_id, expected) in [
        (
            Arc::new(JobProvider(Some(Arc::new(RecordingService::default())))) as Arc<dyn Provider>,
            CallId("no-request".into()),
            "request",
        ),
        (
            Arc::new(JobProvider(None)) as Arc<dyn Provider>,
            CallId("no-backend".into()),
            "backend",
        ),
    ] {
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let no_request = expected == "request";
        scheduler
            .start_for_agent(
                provider,
                AgentPath("/root".into()),
                if no_request {
                    None
                } else {
                    Some(RequestId("request-2".into()))
                },
                call_id.clone(),
                "cell".into(),
                Value::Null,
            )
            .await
            .unwrap();
        // This provider performs an operation and therefore reports its typed
        // refusal as the retained tool error rather than fabricating success.
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(2), scheduler.wait(&call_id))
                .await
                .expect("refusal operation timed out")
                .unwrap();
        assert!(
            matches!(output, JobOutput::Completed(Err(message)) if message.message().contains(expected))
        );
    }
}

#[tokio::test]
async fn detached_test_context_has_bounded_progress_without_job_authority() {
    let (context, mut progress_rx) = CallContext::detached_for_test(
        JobHandle("probe".into()),
        CallId("probe-call".into()),
        AgentPath("/root/probe".into()),
    );
    assert!(context.request.is_none());
    assert_eq!(context.verbs.origin(), None);
    assert!(!context.cancel.is_cancelled());
    assert!(matches!(
        context
            .verbs
            .spawn_agent(
                "worker",
                SpawnSource::Prompt,
                Contract {
                    clauses: vec![],
                    acceptance: vec![],
                    owned: vec![],
                    must_not: vec![],
                    introduces: vec![],
                    consumes: vec![],
                    boundaries: vec![],
                    reply: None,
                },
            )
            .await,
        Err(JobVerbError::MissingBackend)
    ));

    let event = json!({"phase":"probe"});
    context.progress.try_send(event.clone()).unwrap();
    assert_eq!(progress_rx.recv().await, Some(event));
}
