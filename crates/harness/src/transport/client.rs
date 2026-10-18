use super::{
    Auth, ResponsesProtocol, ResponsesRequest, ResponsesRoute, ResponsesTurn, TransportError,
    sse::{ResponseAssembly, StreamEvent},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{borrow::Cow, time::Duration};

/// Only transport normalization constructs a body admitted for dispatch.
pub(crate) struct NormalizedProviderBody(Vec<u8>);

impl NormalizedProviderBody {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

pub(crate) fn normalize_for_dispatch(
    request: &ResponsesRequest,
    contract: (ResponsesRoute, ResponsesProtocol),
) -> Result<NormalizedProviderBody, TransportError> {
    if (contract.0 == ResponsesRoute::GeminiInteractions)
        != (contract.1 == ResponsesProtocol::Interactions)
    {
        return Err(TransportError::InvalidAdmission);
    }
    let bytes = match contract.0 {
        // Keep the existing struct serializer and field order byte-for-byte.
        ResponsesRoute::ChatGptPlan => serde_json::to_vec(&normalized_request(request)?),
        ResponsesRoute::GeminiInteractions => serde_json::to_vec(&super::interactions::request(
            request,
            super::InferenceLimits::gemini_default(),
            &vec![false; request.input.len()],
        )?),
        ResponsesRoute::DeepSeek => serde_json::to_vec(&deepseek_request(
            request,
            super::InferenceLimits::deepseek_default(),
            &vec![false; request.input.len()],
        )?),
    }
    .map_err(|_| TransportError::Stream("request serialization failed".into()))?;
    Ok(NormalizedProviderBody(bytes))
}

pub(crate) fn normalize_for_selection(
    request: &ResponsesRequest,
    selection: &super::PreparedTransportSelection,
    authored: &[bool],
) -> Result<NormalizedProviderBody, TransportError> {
    let contract = selection.response_contract();
    if (contract.0 == ResponsesRoute::GeminiInteractions)
        != (contract.1 == ResponsesProtocol::Interactions)
    {
        return Err(TransportError::InvalidAdmission);
    }
    if let Some(admission) = selection.native_admission() {
        let contract = admission.contract();
        contract.validate()?;
        if contract.route == ResponsesRoute::ChatGptPlan
            && contract.limits.max_output_tokens.is_none()
        {
            return normalize_for_dispatch(request, selection.response_contract());
        }
        let mut body = match contract.route {
            ResponsesRoute::DeepSeek => deepseek_request(request, contract.limits, authored)?,
            ResponsesRoute::GeminiInteractions => {
                super::interactions::request(request, contract.limits, authored)?
            }
            ResponsesRoute::ChatGptPlan => serde_json::to_value(normalized_request(request)?)
                .map_err(|_| TransportError::InvalidAdmission)?,
        };
        if let Some(limit) = contract.limits.max_output_tokens {
            if contract.route == ResponsesRoute::GeminiInteractions {
                body["generation_config"]["max_output_tokens"] = Value::from(limit);
            } else {
                body["max_output_tokens"] = Value::from(limit);
            }
        }
        return serde_json::to_vec(&body)
            .map(NormalizedProviderBody)
            .map_err(|_| TransportError::InvalidAdmission);
    }
    if selection.response_contract().0 == ResponsesRoute::GeminiInteractions {
        return serde_json::to_vec(&super::interactions::request(
            request,
            super::InferenceLimits::gemini_default(),
            authored,
        )?)
        .map(NormalizedProviderBody)
        .map_err(|_| TransportError::InvalidAdmission);
    }
    if selection.response_contract().0 == ResponsesRoute::DeepSeek {
        return serde_json::to_vec(&deepseek_request(
            request,
            super::InferenceLimits::deepseek_default(),
            authored,
        )?)
        .map(NormalizedProviderBody)
        .map_err(|_| TransportError::InvalidAdmission);
    }
    normalize_for_dispatch(request, selection.response_contract())
}

pub const GEMINI_INTERACTIONS_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/interactions";

pub const DEEPSEEK_ENDPOINT: &str = "https://api.deepseek.com/responses";

pub const CHATGPT_PLAN_ENDPOINT: &str = "https://api.openai.com/v1/responses";

#[cfg(test)]
#[path = "client_deepseek_tests.rs"]
mod deepseek_tests;

#[cfg(test)]
#[path = "client_portability_tests.rs"]
mod portability_tests;

#[cfg(test)]
#[path = "client_provider_failure_tests.rs"]
mod provider_failure_tests;

#[cfg(test)]
#[path = "client_continuation_tests.rs"]
mod continuation_tests;

/// Only this transport builds the backend request; the stable session id is
/// shared with prompt_cache_key, while caller controls the complete stateless
/// item window. Never send previous_response_id or store:true.
pub fn request_body(request: &ResponsesRequest) -> Result<Value, TransportError> {
    serde_json::to_value(normalized_request(request)?)
        .map_err(|_| TransportError::Stream("request serialization failed".into()))
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    input: Cow<'a, [crate::item::Item]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<RequestTools>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    reasoning: Reasoning,
    stream: bool,
    store: bool,
    prompt_cache_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_options: Option<&'a super::PromptCacheComparison>,
    include: [&'static str; 1],
}

#[derive(Serialize)]
#[serde(untagged)]
enum RequestTools {
    Host(Vec<Value>),
}

#[derive(Serialize)]
struct Reasoning {
    effort: crate::model::Effort,
    summary: &'static str,
}

#[derive(Serialize)]
struct ToolChoice(&'static str);

fn normalized_request(request: &ResponsesRequest) -> Result<RequestBody<'_>, TransportError> {
    if matches!(
        crate::item::Item::effective_effort(&request.input, request.pinned_effort),
        crate::model::Effort::None | crate::model::Effort::Max
    ) {
        return Err(TransportError::UnsupportedSemantics(
            super::UnsupportedSemantics::ReasoningEffort,
        ));
    }
    if request.model.is_empty() || !request.session_id.valid() {
        return Err(TransportError::Stream(
            "empty model or invalid session identity headers".into(),
        ));
    }
    if request
        .prompt_cache_comparison
        .as_ref()
        .is_some_and(|comparison| comparison.comparison_response_id.is_empty())
    {
        return Err(TransportError::Stream(
            "empty cache diagnostic comparison response id".into(),
        ));
    }
    if request
        .input
        .iter()
        .any(|item| item.0.get("gemini_step").is_some())
    {
        return Err(TransportError::UnsupportedSemantics(
            super::UnsupportedSemantics::InputItem,
        ));
    }
    request.tools.strict_tools()?;
    if request.input.iter().any(|item| item.0["role"] == "system") {
        return Err(TransportError::Stream(
            "ChatGPT plan input requires instructions or developer messages".into(),
        ));
    }
    let input = Cow::Owned(
        request
            .input
            .iter()
            .filter(|item| !item.is_configuration_update())
            .map(|item| {
                let mut item = item.clone();
                if let Some(fields) = item.0.as_object_mut() {
                    if matches!(
                        fields.get("type").and_then(Value::as_str),
                        Some("function_call_output" | "custom_tool_call_output")
                    ) {
                        // Settlement provenance is durable metadata, outside the provider item schema.
                        fields.remove("output_format");
                    }
                }
                item
            })
            .collect(),
    );
    let mut tools = Vec::new();
    for tool in request.tools.iter() {
        if !matches!(tool["type"].as_str(), Some("function" | "custom")) {
            return Err(TransportError::Stream(
                "ChatGPT plan route requires function or custom host tools".into(),
            ));
        }
        if request
            .tools_allowed
            .as_ref()
            .is_none_or(|allowed| allowed.iter().any(|name| tool["name"] == name.as_str()))
        {
            tools.push(tool.clone());
        }
    }
    let has_tools = !tools.is_empty();
    Ok(RequestBody {
        model: &request.model,
        instructions: (!request.instructions.is_empty()).then_some(request.instructions.as_str()),
        input,
        tools: has_tools.then(|| RequestTools::Host(vec![serde_json::json!({"type":"namespace","name":"functions","description":"Exomonad host tools","tools":tools})])),
        tool_choice: has_tools.then_some(ToolChoice("auto")),
        reasoning: Reasoning {
            effort: crate::item::Item::effective_effort(&request.input, request.pinned_effort),
            summary: "auto",
        },
        stream: true,
        store: false,
        prompt_cache_key: request.session_id.cache_key(),
        prompt_cache_options: request.prompt_cache_comparison.as_ref(),
        include: ["reasoning.encrypted_content"],
    })
}

fn deepseek_request(
    request: &ResponsesRequest,
    limits: super::InferenceLimits,
    authored: &[bool],
) -> Result<Value, TransportError> {
    use super::UnsupportedSemantics as Unsupported;
    let refuse = |kind| TransportError::UnsupportedSemantics(kind);
    if request.model.is_empty()
        || !request.session_id.valid()
        || authored.len() != request.input.len()
    {
        return Err(TransportError::InvalidAdmission);
    }
    if crate::item::Item::effective_effort(&request.input, request.pinned_effort)
        == crate::model::Effort::Medium
    {
        return Err(refuse(Unsupported::ReasoningEffort));
    }
    if request.prompt_cache_comparison.is_some() {
        return Err(refuse(Unsupported::CacheComparison));
    }
    if request
        .input
        .iter()
        .any(|item| item.0.get("gemini_step").is_some())
    {
        return Err(TransportError::UnsupportedSemantics(
            super::UnsupportedSemantics::InputItem,
        ));
    }
    request.tools.strict_tools()?;
    if request.tools.has_duplicate_names() {
        return Err(refuse(Unsupported::ToolDeclaration));
    }
    let mut tools = Vec::new();
    for tool in request.tools.iter() {
        let name = tool["name"]
            .as_str()
            .ok_or_else(|| refuse(Unsupported::ToolDeclaration))?;
        if tool["type"] != "function"
            || name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || tool.get("namespace").is_some()
            || tool.get("async").is_some()
        {
            return Err(refuse(Unsupported::ToolDeclaration));
        }
        if request
            .tools_allowed
            .as_ref()
            .is_none_or(|allowed| allowed.iter().any(|allowed| allowed == name))
        {
            tools.push(tool.clone());
        }
    }
    let mut input = Vec::new();
    for (original, authored) in request.input.iter().zip(authored) {
        if original.is_configuration_update() {
            continue;
        }
        let mut item = original.0.clone();
        match item["type"].as_str() {
            Some("message") => {
                let role = item["role"]
                    .as_str()
                    .ok_or_else(|| refuse(Unsupported::Message))?;
                if !matches!(role, "system" | "developer" | "user" | "assistant") {
                    return Err(refuse(Unsupported::Message));
                }
                let valid_content = item["content"].is_string()
                    || item["content"].as_array().is_some_and(|parts| {
                        parts.iter().all(|part| {
                            (matches!(part["type"].as_str(), Some("input_text" | "output_text"))
                                && part["text"].is_string())
                                || (role == "user" && part["type"] == "input_image")
                        })
                    });
                if !valid_content {
                    return Err(refuse(Unsupported::Message));
                }
                // DeepSeek treats developer as user. Keep authored policy at its
                // original position by explicitly selecting the supported system role.
                if role == "developer" {
                    if !authored {
                        return Err(refuse(Unsupported::Message));
                    }
                    item["role"] = Value::String("system".into());
                }
            }
            Some("function_call" | "function_call_output") => {
                if item.get("namespace").is_some() || item.get("async").is_some() {
                    return Err(refuse(Unsupported::InputItem));
                }
                if let Some(fields) = item.as_object_mut() {
                    fields.remove("output_format");
                }
            }
            Some("reasoning") => {
                if item.get("encrypted_content").is_some_and(|v| !v.is_null())
                    || item
                        .get("summary")
                        .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                {
                    return Err(refuse(Unsupported::ReasoningContent));
                }
                if !item["content"].as_array().is_some_and(|parts| {
                    parts
                        .iter()
                        .all(|part| part["type"] == "reasoning_text" && part["text"].is_string())
                }) {
                    return Err(refuse(Unsupported::ReasoningContent));
                }
            }
            _ => return Err(refuse(Unsupported::InputItem)),
        }
        input.push(item);
    }
    let mut body = serde_json::json!({"model":request.model,"input":input,"stream":true,"max_output_tokens":limits.max_output_tokens,"reasoning":{"effort":crate::item::Item::effective_effort(&request.input, request.pinned_effort)}});
    if !request.instructions.is_empty() {
        body["instructions"] = Value::String(request.instructions.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String("auto".into());
    }
    Ok(body)
}

/// A small SSE framer for the public Responses endpoint. We keep it here rather than
/// splitting by network chunk or by single line: one event may contain
/// multiple `data:` lines and span arbitrary chunks. The caller feeds events
/// to `ResponseAssembly` without waiting for the stream to end.
struct SseFramer {
    line: Vec<u8>,
    data: Vec<String>,
    event_name: Option<String>,
    skip_lf: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct FramedEvent {
    data: String,
    event_name: Option<String>,
}

impl SseFramer {
    fn new() -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            event_name: None,
            skip_lf: false,
        }
    }

    // Preserve earlier frames if a later line in the same network chunk is
    // malformed. Terminal interpretation must not depend on network chunking.
    fn push(&mut self, chunk: &[u8]) -> (Vec<FramedEvent>, Option<TransportError>) {
        let mut events = Vec::new();
        for byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if *byte == b'\r' || *byte == b'\n' {
                self.skip_lf = *byte == b'\r';
                match self.end_line() {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => {}
                    Err(error) => return (events, Some(error)),
                }
            } else {
                self.line.push(*byte);
                // Existing runaway safeguard for a single provider-controlled line.
                if self.line.len() > 1024 * 1024 {
                    return (
                        events,
                        Some(TransportError::Stream("SSE line too large".into())),
                    );
                }
            }
        }
        (events, None)
    }

    fn end_line(&mut self) -> Result<Option<FramedEvent>, TransportError> {
        let line = std::str::from_utf8(&self.line)
            .map_err(|_| TransportError::Stream("invalid SSE UTF-8".into()))?;
        let event = if line.is_empty() {
            let event_name = self.event_name.take();
            if self.data.is_empty() {
                None
            } else {
                let data = self.data.join("\n");
                self.data.clear();
                Some(FramedEvent { data, event_name })
            }
        } else {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => self.data.push(value.to_owned()),
                "event" => self.event_name = Some(value.to_owned()),
                _ => {}
            }
            None
        };
        self.line.clear();
        Ok(event)
    }

    fn observe_tail(&self, summary: &mut super::TransportObservation) {
        // WHATWG requires discarding this unfinished event at EOF. Retain only
        // its lengths; never promote partial JSON or arguments into an item.
        summary.unframed_line_bytes = self.line.len() as u64;
        summary.unframed_data_bytes = self.data.iter().map(|line| line.len() as u64).sum();
        summary.unframed_event_name = self.event_name.is_some();
    }
}

fn http_request(
    http: &reqwest::Client,
    request: &ResponsesRequest,
    body: Vec<u8>,
    token: &str,
) -> reqwest::RequestBuilder {
    let call = http
        .post(CHATGPT_PLAN_ENDPOINT)
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("session-id", request.session_id.affinity())
        .header("thread-id", request.session_id.thread())
        .header("x-client-request-id", request.session_id.thread())
        .header(reqwest::header::USER_AGENT, "exomonad-harness/0.1.0");
    // Compression is a wire optimization. Preserve the admitted JSON if the
    // compressor cannot encode it, matching the public client's fallback.
    match zstd::stream::encode_all(body.as_slice(), 0) {
        Ok(encoded) => call
            .header(reqwest::header::CONTENT_ENCODING, "zstd")
            .body(encoded),
        Err(_) => call.body(body),
    }
}

fn native_http_request(
    http: &reqwest::Client,
    request: &ResponsesRequest,
    body: Vec<u8>,
    token: &str,
    contract: &super::NativeTransportContract,
) -> reqwest::RequestBuilder {
    match contract.route {
        ResponsesRoute::ChatGptPlan => http_request(http, request, body, token),
        ResponsesRoute::GeminiInteractions => http
            .post(&contract.endpoint)
            .header("x-goog-api-key", token)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body),
        ResponsesRoute::DeepSeek => http
            .post(&contract.endpoint)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body),
    }
}

pub(super) fn http_client() -> Result<reqwest::Client, TransportError> {
    http_client_with_limits(
        super::InferenceLimits::chatgpt_default(),
        ResponsesRoute::ChatGptPlan,
    )
}
pub(super) fn http_client_with_limits(
    limits: super::InferenceLimits,
    route: ResponsesRoute,
) -> Result<reqwest::Client, TransportError> {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(limits.connect_timeout_secs))
        .timeout(Duration::from_secs(limits.total_timeout_secs))
        .redirect(reqwest::redirect::Policy::none());
    let builder = match route {
        ResponsesRoute::ChatGptPlan => builder,
        ResponsesRoute::DeepSeek | ResponsesRoute::GeminiInteractions => {
            builder.retry(reqwest::retry::never())
        }
    };
    builder
        .build()
        .map_err(|_| TransportError::Stream("HTTP client initialization failed".into()))
}

fn observed_utc_ms() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64)
}

/// One request-owned receipt publishes diagnostics synchronously on producer
/// drop, independently of provider-output backpressure and without another task.
struct ObservationGuard {
    observation: Option<super::TransportObservation>,
    publisher: Option<tokio::sync::oneshot::Sender<super::TransportObservation>>,
    started: Option<std::time::Instant>,
    body_started: Option<std::time::Instant>,
    queued: Option<std::time::Instant>,
    framer: SseFramer,
}
impl ObservationGuard {
    fn new(
        publisher: Option<tokio::sync::oneshot::Sender<super::TransportObservation>>,
        request: Option<(&ResponsesRequest, &super::NativeTransportContract)>,
    ) -> Self {
        let started = request.map(|_| std::time::Instant::now());
        let mut summary = super::TransportObservation::new(super::ResponseEnd::Abandoned);
        if let Some((request, contract)) = request {
            let (model, truncated) = super::observation::metadata(&request.model, "", "");
            summary.requested_model = Some(model);
            summary.metadata_truncated |= truncated;
            summary.backend = Some(contract.route);
            summary.effective_reasoning = Some(crate::item::Item::effective_effort(
                &request.input,
                request.pinned_effort,
            ));
            summary.max_output_tokens = contract.limits.max_output_tokens;
            summary.started_at_unix_ms = observed_utc_ms();
            summary.dispatch_attempted = Some(false);
        }
        Self {
            observation: Some(summary),
            publisher,
            started,
            body_started: None,
            queued: None,
            framer: SseFramer::new(),
        }
    }
    fn summary(&mut self) -> &mut super::TransportObservation {
        self.observation.as_mut().expect("active observation")
    }
    fn elapsed_ms(&self) -> Option<u64> {
        self.started
            .map(|started| started.elapsed().as_millis().min(u64::MAX as u128) as u64)
    }
    fn headers(&mut self, response: &reqwest::Response, token: &str, account: &str) {
        let headers = super::TransportObservation::response(response, token, account);
        self.summary().http_status = headers.http_status;
        self.summary().server_request_id = headers.server_request_id;
        self.summary().content_type = headers.content_type;
        self.summary().metadata_truncated |= headers.metadata_truncated;
    }
    fn visible_event(&mut self, event: &StreamEvent) {
        use super::sse::OutputChannel;
        let (text, reasoning) = match event {
            StreamEvent::Delta { channel, text, .. } => (
                !text.is_empty() && *channel == OutputChannel::Assistant,
                !text.is_empty() && *channel == OutputChannel::Reasoning,
            ),
            StreamEvent::CompletedItem(done) => {
                let item = &done.item().0;
                let has = |kind| {
                    item["content"].as_array().is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part["type"] == kind
                                && part["text"].as_str().is_some_and(|text| !text.is_empty())
                        })
                    })
                };
                (
                    item["type"] == "message" && item["role"] == "assistant" && has("output_text"),
                    item["type"] == "reasoning" && has("reasoning_text"),
                )
            }
        };
        let elapsed = self.elapsed_ms();
        if text && self.summary().first_text_ms.is_none() {
            self.summary().first_text_ms = elapsed;
        }
        if reasoning && self.summary().first_reasoning_ms.is_none() {
            self.summary().first_reasoning_ms = elapsed;
        }
    }
    fn publish(&mut self) {
        let Some(mut summary) = self.observation.take() else {
            return;
        };
        self.framer.observe_tail(&mut summary);
        if let Some(started) = self.started {
            summary.total_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            summary.completed_at_unix_ms = observed_utc_ms();
            if summary.queue_ms.is_none() {
                summary.queue_ms = self
                    .queued
                    .map(|queued| queued.elapsed().as_millis().min(u64::MAX as u128) as u64);
            }
        }
        if let Some(started) = self.body_started {
            summary.body_elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        }
        if let Some(publisher) = self.publisher.take() {
            let _ = publisher.send(summary);
        }
    }
}
impl Drop for ObservationGuard {
    fn drop(&mut self) {
        self.publish();
    }
}

/// Auth lookup and HTTP begin only after exact prepared-contract comparison and
/// inference admission. Cancellation retains facts; it never proves completion.
pub(super) async fn execute<A: Auth + Clone + 'static>(
    auth: A,
    admission: super::NativeTransportAdmission,
    http: Result<reqwest::Client, TransportError>,
    mut prepared: super::PreparedProviderRequest,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ResponsesTurn, TransportError> {
    if prepared
        .selection()
        .native_admission()
        .map(|a| a.contract())
        != Some(admission.contract())
    {
        return Err(TransportError::PreparedSelectionMismatch);
    }
    let mut observation = ObservationGuard::new(
        prepared.take_observation_publisher(),
        Some((prepared.request(), admission.contract())),
    );
    let http = match http {
        Ok(http) => http,
        Err(error) => {
            observation.summary().end = super::ResponseEnd::AdmissionFailed;
            return Err(error);
        }
    };
    observation.queued = Some(std::time::Instant::now());
    let _permit = match admission.acquire().await {
        Ok(permit) => permit,
        Err(error) => {
            observation.summary().end = super::ResponseEnd::AdmissionFailed;
            return Err(error);
        }
    };
    observation.summary().queue_ms = observation
        .queued
        .map(|queued| queued.elapsed().as_millis().min(u64::MAX as u128) as u64);
    let (request, body, _) = prepared.into_dispatch_parts();
    let token = match tokio::task::spawn_blocking(move || auth.access()).await {
        Ok(Ok(token)) => token,
        Ok(Err(error)) => {
            observation.summary().end = super::ResponseEnd::AuthenticationFailed;
            return Err(error);
        }
        Err(_) => {
            observation.summary().end = super::ResponseEnd::AuthenticationFailed;
            return Err(match admission.contract().route {
                ResponsesRoute::ChatGptPlan => TransportError::Authentication,
                route => TransportError::CredentialUnavailable {
                    route,
                    reason: super::CredentialFailure::WorkerFailed,
                },
            });
        }
    };
    let call = native_http_request(&http, &request, body, &token, admission.contract());
    observation.summary().dispatch_attempted = Some(true);
    let response = match call.send().await {
        Ok(response) => response,
        Err(_) => {
            observation.summary().end = super::ResponseEnd::SendFailed;
            return Err(TransportError::Stream("HTTP request failed".into()));
        }
    };
    observation.headers(&response, &token, "");
    let status = response.status().as_u16();
    if status != 200 {
        observation.summary().end = super::ResponseEnd::HttpRejected;
        if status == 401 {
            return Err(match admission.contract().route {
                ResponsesRoute::ChatGptPlan => TransportError::Authentication,
                route => TransportError::ProviderAuthentication { route },
            });
        }
        let diagnostic = super::http_error::read(response, &token, "").await;
        return Err(TransportError::Http { status, diagnostic });
    }
    read_response_observed(
        response,
        sink,
        &token,
        "",
        &mut observation,
        admission.contract().route,
    )
    .await
}

#[cfg(test)]
async fn read_response(
    response: reqwest::Response,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    token: &str,
    account: &str,
    publisher: Option<tokio::sync::oneshot::Sender<super::TransportObservation>>,
) -> Result<ResponsesTurn, TransportError> {
    let mut observation = ObservationGuard::new(publisher, None);
    read_response_observed(
        response,
        sink,
        token,
        account,
        &mut observation,
        ResponsesRoute::ChatGptPlan,
    )
    .await
}

async fn read_response_observed(
    response: reqwest::Response,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    token: &str,
    account: &str,
    observation: &mut ObservationGuard,
    route: ResponsesRoute,
) -> Result<ResponsesTurn, TransportError> {
    use super::ResponseEnd;
    let started = std::time::Instant::now();
    observation.headers(&response, token, account);
    observation.body_started = Some(started);
    observation.summary().received_bytes = Some(0);
    observation.summary().received_chunks = Some(0);
    observation.summary().framed_events = Some(0);
    let mut stream = response.bytes_stream();
    let mut assembly = ProviderAssembly::new(route);
    let (result, end) = 'body: loop {
        let Some(chunk) = stream.next().await else {
            break (assembly.finish(), ResponseEnd::CleanEof);
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                // Body-read errors contain transport causes, never provider body data.
                let diagnostic: String = format!("{:?}", error.without_url())
                    .chars()
                    .take(512)
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                tracing::warn!(error = %diagnostic, "provider response body read failed");
                break (
                    assembly.finish().map_err(|_| {
                        TransportError::IncompleteResponse(super::StreamInterruption::ReadFailed)
                    }),
                    ResponseEnd::ReadFailed,
                );
            }
        };
        if !chunk.is_empty() && observation.summary().first_byte_ms.is_none() {
            observation.summary().first_byte_ms = Some(started.elapsed().as_millis() as u64);
        }
        *observation.summary().received_bytes.as_mut().unwrap() += chunk.len() as u64;
        *observation.summary().received_chunks.as_mut().unwrap() += 1;
        let (events, framing_error) = observation.framer.push(&chunk);
        for frame in events {
            if observation.summary().first_event_ms.is_none() {
                observation.summary().first_event_ms = observation.elapsed_ms();
            }
            *observation.summary().framed_events.as_mut().unwrap() += 1;
            if let Some(name) = &frame.event_name {
                observation
                    .summary()
                    .sse_event(super::EventKind::from_provider(Some(name)));
            }
            match assembly.accept(&frame.data, token, account, observation.summary()) {
                Ok(events) => {
                    for event in events {
                        observation.visible_event(&event);
                        if let Some(sender) = &sink {
                            let _ = sender.send(event).await;
                        }
                    }
                }
                Err(error) => {
                    let end = if observation.summary().terminal.is_some() {
                        ResponseEnd::ProviderTerminal
                    } else {
                        ResponseEnd::ProtocolError
                    };
                    break 'body (Err(error), end);
                }
            }
            if assembly.is_complete() {
                break 'body (assembly.finish(), ResponseEnd::ProviderTerminal);
            }
        }
        if let Some(error) = framing_error {
            break (Err(error), ResponseEnd::ProtocolError);
        }
    };
    observation.summary().end = end;
    result
}

// Both protocols share framing, observation, admission and cancellation owners.
enum ProviderAssembly {
    Responses(ResponseAssembly),
    Interactions(super::interactions::Assembly),
}
impl ProviderAssembly {
    fn new(route: ResponsesRoute) -> Self {
        match route {
            ResponsesRoute::GeminiInteractions => Self::Interactions(Default::default()),
            _ => Self::Responses(Default::default()),
        }
    }
    fn accept(
        &mut self,
        data: &str,
        token: &str,
        account: &str,
        observation: &mut super::TransportObservation,
    ) -> Result<Vec<StreamEvent>, TransportError> {
        match self {
            Self::Responses(assembly) => assembly
                .accept_observed(data, token, account, Some(observation))
                .map(|event| event.into_iter().collect()),
            Self::Interactions(assembly) => assembly.accept(data, token, account, observation),
        }
    }
    fn is_complete(&self) -> bool {
        match self {
            Self::Responses(a) => a.is_complete(),
            Self::Interactions(a) => a.is_complete(),
        }
    }
    fn finish(self) -> Result<ResponsesTurn, TransportError> {
        match self {
            Self::Responses(a) => a.finish(),
            Self::Interactions(a) => a.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::Item,
        model::Effort,
        transport::{ResponsesClient, auth::ChatGptPlanAuth},
    };

    #[test]
    fn request_is_stateless_and_pins_effort() {
        let request = ResponsesRequest {
            input: vec![Item::user_text("hello")],
            instructions: "fixed".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
            prompt_cache_comparison: None,
        };
        let body = request_body(&request).expect("body");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["prompt_cache_key"], "shared");
        assert_eq!(body["instructions"], "fixed");
        assert_eq!(body["input"][0]["content"], "hello");
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("previous_response_id").is_none());
    }

    #[test]
    fn restricted_tools_keep_manifest_order_and_empty_omits_tool_fields() {
        let mut request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![
                json!({"type":"function","name":"a","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
                json!({"type":"function","name":"b","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
            ]
            .into(),
            tools_allowed: Some(vec!["b".into(), "a".into()]),
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
            prompt_cache_comparison: None,
        };
        let body = request_body(&request).unwrap();
        assert_eq!(body["tools"][0]["tools"].as_array().unwrap().len(), 2);
        assert_eq!(body["tools"][0]["tools"][0]["name"], "a");
        assert_eq!(body["tools"][0]["tools"][1]["name"], "b");
        request.tools_allowed = Some(vec![]);
        assert!(request_body(&request).unwrap().get("tool_choice").is_none());
        assert!(request_body(&request).unwrap().get("tools").is_none());
    }

    #[test]
    fn rejects_non_strict_tool() {
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![json!({"type":"function","name":"bad"})].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
            prompt_cache_comparison: None,
        };
        assert!(request_body(&request).is_err());
    }

    #[test]
    fn accepts_custom_tool_without_function_strict_field() {
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![json!({"type":"custom","name":"run","description":"Freeform script"})]
                .into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
            prompt_cache_comparison: None,
        };
        assert!(request_body(&request).is_ok());
    }

    #[test]
    fn request_gate_checks_nested_strict_contracts_and_preserves_wire_metadata() {
        let valid = json!({"type":"function","name":"lookup","strict":true,"parameters":{
            "type":"object","properties":{"nested":{"type":"object","properties":{
                "value":{"type":["string","null"],"description":"An optional value"}
            },"required":["value"],"additionalProperties":false}},
            "required":["nested"],"additionalProperties":false,"description":"Host contract"
        }});
        let mut request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![valid.clone()].into(),
            tools_allowed: None,
            model: "test-model".into(),
            pinned_effort: Effort::Low,
            session_id: "gate".into(),
            prompt_cache_comparison: None,
        };
        assert_eq!(
            request_body(&request).unwrap()["tools"][0]["tools"][0],
            valid
        );
        // Deserialization retains the same immutable admission result.
        let restored: ResponsesRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        assert_eq!(
            request_body(&restored).unwrap()["tools"][0]["tools"][0],
            valid
        );
        for path in [
            "root-required",
            "nested-required",
            "nested-properties",
            "parameters",
        ] {
            let mut invalid = valid.clone();
            match path {
                "root-required" => {
                    invalid["parameters"]
                        .as_object_mut()
                        .unwrap()
                        .remove("required");
                }
                "nested-required" => {
                    invalid["parameters"]["properties"]["nested"]["required"] = json!([])
                }
                "nested-properties" => {
                    invalid["parameters"]["properties"]["nested"]
                        .as_object_mut()
                        .unwrap()
                        .remove("additionalProperties");
                }
                "parameters" => invalid["parameters"] = json!({"type":"string"}),
                _ => unreachable!(),
            }
            request.tools = vec![invalid].into();
            assert!(
                request_body(&request).is_err(),
                "invalid {path} was serialized"
            );
        }
    }

    #[tokio::test]
    async fn request_gate_refuses_invalid_schema_before_authentication_access() {
        #[derive(Clone)]
        struct NoAuth;
        impl Auth for NoAuth {
            fn admission(
                &self,
            ) -> std::result::Result<
                crate::transport::NativeTransportAdmission,
                crate::transport::TransportError,
            > {
                crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
                    "/offline/synthetic-credential",
                ))
            }
            fn access(&self) -> Result<String, TransportError> {
                panic!("invalid schema must be refused before authentication");
            }
        }
        let request = ResponsesRequest {
            input: vec![], instructions:String::new(), tools:vec![json!({
                "type":"function","name":"status","strict":true,
                "parameters":{"type":"object","properties":{"view":{"type":"string"}},"additionalProperties":false}
            })].into(),tools_allowed:None,model:"test-model".into(),pinned_effort:Effort::Low,session_id:"gate-before-auth".into(),prompt_cache_comparison:None
        };
        let client = ResponsesClient::new(NoAuth);
        let selection = crate::engine::ResponsesTransport::prepare_submission(&client).unwrap();
        let store = crate::store::Store::memory().unwrap();
        let head = crate::model::RequestId("invalid-schema".into());
        store.create_request(&head, None, "/root").unwrap();
        assert!(matches!(
            store
                .prepare_provider_request(
                    &head,
                    &head,
                    request,
                    &[],
                    None,
                    selection,
                    crate::store::ProviderRequestPurpose::Inference,
                )
                .map_err(crate::store::StoreError::from),
            Err(crate::store::StoreError::ProviderAdmission(
                TransportError::Stream(_)
            ))
        ));
        assert_eq!(store.provider_response_state(&head).unwrap(), None);
    }

    #[test]
    fn sse_frame_accepts_split_multiline_event() {
        let mut f = SseFramer::new();
        assert!(f.push(b"data: {\"type\":\r\n").0.is_empty());
        assert!(f.push(b"data: \"x\"}\n").0.is_empty());
        assert_eq!(
            f.push(b"\n").0,
            vec![FramedEvent {
                data: "{\"type\":\n\"x\"}".to_owned(),
                event_name: None
            }]
        );
    }

    #[tokio::test]
    async fn provider_stream_error_retains_redacted_fields_through_http_body_reader() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let token = "mock-active-token";
        let account = "mock-active-account";
        let packet = json!({"type":"error", "code":"invalid_tools", "param":"tools[0].parameters",
            "message":format!("schema refused {token} {account}"), "echoed_request":{"token":token}});
        let body = format!("data: {packet}\n\n");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len(),
            ).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let response = http_client()
            .unwrap()
            .get(format!("http://{address}/responses"))
            .send()
            .await
            .unwrap();
        let failure = read_response(response, None, token, account, None)
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(&failure, TransportError::ProviderStreamFailure {
            event: super::super::ProviderStreamFailureEvent::Error,
            diagnostic: Some(diagnostic),
        } if diagnostic.code.as_deref() == Some("invalid_tools")
            && diagnostic.param.as_deref() == Some("tools[0].parameters")
            && diagnostic.message.as_deref() == Some("schema refused [redacted] [redacted]")));
        assert!(failure.request_failure().is_none());
        let rendered = failure.to_string();
        for private in [token, account, "echoed_request"] {
            assert!(!rendered.contains(private));
        }
    }

    #[tokio::test]
    async fn body_truncation_preserves_terminal_proof_and_distinguishes_clean_eof() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (completed, truncated) in [(false, false), (false, true), (true, true)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let body = if completed {
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"terminal-proof\",\"output\":[]}}\n\n"
            } else {
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"terminal-proof\"}}\n\n"
            };
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                socket.read(&mut request).await.unwrap();
                let length = body.len() + usize::from(truncated) * 100;
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
                ).as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            });
            let response = http_client()
                .unwrap()
                .get(format!("http://{address}/responses"))
                .send()
                .await
                .unwrap();
            let result = read_response(response, None, "", "", None).await;
            server.await.unwrap();
            match (completed, truncated, result) {
                (true, _, Ok(turn)) => assert_eq!(turn.response_id, "terminal-proof"),
                (
                    false,
                    false,
                    Err(TransportError::IncompleteResponse(
                        super::super::StreamInterruption::MissingCompletion,
                    )),
                ) => {}
                (
                    false,
                    true,
                    Err(TransportError::IncompleteResponse(
                        super::super::StreamInterruption::ReadFailed,
                    )),
                ) => {}
                (_, _, result) => panic!("unexpected response disposition: {result:?}"),
            }
        }
    }

    /// Explicit live smoke test; never run in ordinary CI. Only the public
    /// item/usage shape is asserted, never the credential or request headers.
    #[tokio::test]
    #[ignore]
    async fn live_subscription_response() {
        let auth = ChatGptPlanAuth::new(ChatGptPlanAuth::default_path().expect("auth path"));
        let client = ResponsesClient::new(auth);
        let request = ResponsesRequest {
            input: vec![Item::user_text("Reply with exactly: TRANSPORT_OK")],
            instructions: "This is a transport smoke test. Reply briefly.".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "harness-wave0-transport-smoke".into(),
            prompt_cache_comparison: None,
        };
        let selection = crate::engine::ResponsesTransport::prepare_submission(&client).unwrap();
        let store = crate::store::Store::memory().unwrap();
        let head = crate::model::RequestId("live-smoke".into());
        store.create_request(&head, None, "/root").unwrap();
        let prepared = store
            .prepare_provider_request(
                &head,
                &head,
                request,
                &[None],
                None,
                selection,
                crate::store::ProviderRequestPurpose::Inference,
            )
            .unwrap();
        let turn = client.create(prepared).await.expect("live response");
        assert!(
            turn.items
                .iter()
                .any(|item| item.0["phase"] == "final_answer")
        );
        assert!(!turn.response_id.is_empty());
    }
}

#[cfg(test)]
#[path = "client_plan_tests.rs"]
mod plan_tests;

#[cfg(test)]
#[path = "client_terminal_tests.rs"]
mod terminal_tests;
