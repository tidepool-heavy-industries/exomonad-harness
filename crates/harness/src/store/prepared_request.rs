//! Store admission and transport normalization meet once before dispatch.
use super::{Store, StoreError, context, provider_response::IssuedProviderRequest};
use crate::{
    context::{ContextError, ContextReplayRejection, Occurrence},
    item::ItemHash,
    model::RequestId,
    transport::{
        PreparedTransportSelection, ResponsesProtocol, ResponsesRequest, ResponsesRoute,
        TransportError, client,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderRequestPurpose {
    Inference,
    ServerCompaction,
    TextSummary,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PrepareProviderError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

impl From<PrepareProviderError> for StoreError {
    fn from(error: PrepareProviderError) -> Self {
        match error {
            PrepareProviderError::Store(error) => error,
            PrepareProviderError::Transport(error) => Self::ProviderAdmission(error),
        }
    }
}

/// Dispatch authority is created only after checked input and durable issuance.
/// Captured request DTOs and restored JSON cannot construct this value.
pub struct PreparedProviderRequest {
    request: ResponsesRequest,
    body: client::NormalizedProviderBody,
    selection: PreparedTransportSelection,
    issued: IssuedProviderRequest,
    observation_tx: Option<tokio::sync::oneshot::Sender<crate::transport::TransportObservation>>,
    observation_rx: Option<tokio::sync::oneshot::Receiver<crate::transport::TransportObservation>>,
}

impl PreparedProviderRequest {
    /// Transfer this issued request's diagnostic receipt once. A closed receipt
    /// means no observation was published; it does not establish completion.
    /// This channel is independent of bounded provider-output backpressure.
    pub fn take_transport_observation(
        &mut self,
    ) -> Option<tokio::sync::oneshot::Receiver<crate::transport::TransportObservation>> {
        self.observation_rx.take()
    }
    pub(crate) fn take_observation_publisher(
        &mut self,
    ) -> Option<tokio::sync::oneshot::Sender<crate::transport::TransportObservation>> {
        self.observation_tx.take()
    }
    pub fn request(&self) -> &ResponsesRequest {
        &self.request
    }
    pub fn request_id(&self) -> &RequestId {
        &self.issued.request
    }
    pub fn response_contract(&self) -> (ResponsesRoute, ResponsesProtocol) {
        self.selection.response_contract()
    }
    pub fn wire_body(&self) -> &[u8] {
        self.body.bytes()
    }
    pub(crate) fn issuer(&self) -> &IssuedProviderRequest {
        &self.issued
    }
    pub(crate) fn selection(&self) -> &PreparedTransportSelection {
        &self.selection
    }
    pub(crate) fn purpose(&self) -> ProviderRequestPurpose {
        self.issued.purpose()
    }
    pub(crate) fn into_dispatch_parts(
        self,
    ) -> (
        ResponsesRequest,
        Vec<u8>,
        (ResponsesRoute, ResponsesProtocol),
    ) {
        (
            self.request,
            self.body.into_bytes(),
            self.selection.response_contract(),
        )
    }
}

impl Store {
    pub(crate) fn prepare_provider_request(
        &self,
        source: &RequestId,
        local: &RequestId,
        request: ResponsesRequest,
        occurrences: &[Option<Occurrence>],
        instructions: Option<&ItemHash>,
        selection: PreparedTransportSelection,
        purpose: ProviderRequestPurpose,
    ) -> Result<PreparedProviderRequest, super::PrepareProviderError> {
        if request.input.len() != occurrences.len() {
            return Err(StoreError::InvalidProviderEvidence(local.clone()).into());
        }
        let contract = selection.response_contract();
        let canonical = occurrences.iter().flatten().cloned().collect::<Vec<_>>();
        let projection = {
            let c = self.lock()?;
            let projected = context::checked_projection(
                &c,
                source,
                &canonical,
                Some(&request.model),
                None,
                Some(contract),
                selection
                    .native_admission()
                    .map(|admission| admission.contract()),
            )?;
            let expected = projected
                .items()
                .iter()
                .filter_map(|(_, _, item, occurrence)| {
                    occurrence
                        .as_ref()
                        .map(|occurrence| (&occurrence.origin, item))
                })
                .collect::<Vec<_>>();
            let actual = request
                .input
                .iter()
                .zip(occurrences)
                .filter_map(|(item, occurrence)| {
                    occurrence
                        .as_ref()
                        .map(|occurrence| (&occurrence.origin, item))
                })
                .collect::<Vec<_>>();
            if expected != actual {
                return Err(StoreError::Context(ContextError::Replay(
                    ContextReplayRejection::InvalidMembership,
                ))
                .into());
            }
            for (item, occurrence) in request.input.iter().zip(occurrences) {
                if occurrence.is_none()
                    && !item.is_configuration_update()
                    && !(item.0["type"] == "message"
                        && matches!(
                            item.0["role"].as_str(),
                            Some("system" | "developer" | "user" | "assistant")
                        )
                        && !item
                            .0
                            .as_object()
                            .is_some_and(|object| object.contains_key("encrypted_content") || object.contains_key("gemini_step")))
                    && !(purpose == ProviderRequestPurpose::ServerCompaction
                        && item.0["type"] == "compaction_trigger")
                {
                    return Err(StoreError::InvalidProviderEvidence(local.clone()).into());
                }
            }
            projected
        };
        // Editable context notes and protected committed hook annotations have
        // separate issuers. Unretained messages passed host-authored admission
        // above; role spelling and absent provider envelopes prove neither.
        let hook_notes = match contract.0 {
            ResponsesRoute::ChatGptPlan => std::collections::HashSet::new(),
            ResponsesRoute::DeepSeek | ResponsesRoute::GeminiInteractions => self.model_tool_note_origins(occurrences)?,
        };
        let authored = occurrences
            .iter()
            .map(|occurrence| {
                occurrence.as_ref().is_none_or(|occurrence| {
                    occurrence.note || hook_notes.contains(&occurrence.origin)
                })
            })
            .collect::<Vec<_>>();
        let body = client::normalize_for_selection(&request, &selection, &authored)?;
        let hashes = request
            .input
            .iter()
            .zip(occurrences)
            .map(|(item, occurrence)| {
                occurrence
                    .as_ref()
                    .filter(|occurrence| {
                        occurrence.item == *item
                            && occurrence.overlays.is_empty()
                            && !occurrence.note
                    })
                    .map(|occurrence| occurrence.hash.clone())
            })
            .collect::<Vec<_>>();
        let mut sealed = self.seal_replay_request_with_checked_projection(
            source,
            &request,
            &hashes,
            instructions,
            occurrences,
            projection,
        )?;
        sealed.record.purpose = Some(purpose);
        sealed.record.native_transport = selection.native_admission().map(|a| a.contract().clone());
        let issued = self.issue_provider_request(local, sealed, contract)?;
        let (observation_tx, observation_rx) = tokio::sync::oneshot::channel();
        Ok(PreparedProviderRequest {
            request,
            body,
            selection,
            issued,
            observation_tx: Some(observation_tx),
            observation_rx: Some(observation_rx),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{item::Item, model::Effort};
    use serde_json::json;

    fn request() -> ResponsesRequest {
        ResponsesRequest {
            input: vec![Item(
                json!({"type":"message","role":"user","content":"hello"}),
            )],
            instructions: "fixed instructions".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "issued-model".into(),
            pinned_effort: Effort::Low,
            session_id: "session".into(),
            prompt_cache_comparison: None,
        }
    }

    fn store() -> (Store, RequestId) {
        let store = Store::memory().unwrap();
        let head = RequestId("prepared-request".into());
        store.create_request(&head, None, "/root").unwrap();
        (store, head)
    }

    #[test]
    fn frozen_body_and_contract_are_bound_to_durable_issuance() {
        let (store, head) = store();
        let mut dto = request();
        let contract = (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard);
        let expected = client::normalize_for_dispatch(&dto, contract).unwrap();
        let prepared = store
            .prepare_provider_request(
                &head,
                &head,
                dto.clone(),
                &[None],
                None,
                PreparedTransportSelection::component(contract),
                ProviderRequestPurpose::Inference,
            )
            .unwrap();
        dto.model = "later-model".into();
        assert_eq!(prepared.request().model, "issued-model");
        assert_eq!(prepared.request_id(), &head);
        assert_eq!(prepared.response_contract(), contract);
        assert_eq!(prepared.wire_body(), expected.bytes());
        assert_eq!(
            store.provider_response_state(&head).unwrap(),
            Some(super::super::ProviderResponseState::Streaming)
        );
    }

    #[test]
    fn normalization_refusal_prevents_issuance() {
        let (store, head) = store();
        let mut invalid = request();
        invalid.session_id = "invalid\nsession".into();
        assert!(matches!(
            store.prepare_provider_request(
                &head,
                &head,
                invalid,
                &[None],
                None,
                PreparedTransportSelection::component((
                    ResponsesRoute::ChatGptPlan,
                    ResponsesProtocol::Standard
                )),
                ProviderRequestPurpose::Inference
            ),
            Err(PrepareProviderError::Transport(_))
        ));
        assert_eq!(store.provider_response_state(&head).unwrap(), None);
    }

    #[test]
    fn unowned_native_items_cannot_enter_prepared_requests() {
        let (store, head) = store();
        for item in [
            json!({"type":"reasoning","id":"unowned","encrypted_content":"opaque-fixture"}),
            json!({"type":"function_call","call_id":"unowned","name":"effect","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"unowned","output":"result"}),
        ] {
            let mut dto = request();
            dto.input = vec![Item(item)];
            assert!(matches!(
                store.prepare_provider_request(
                    &head,
                    &head,
                    dto,
                    &[None],
                    None,
                    PreparedTransportSelection::component((
                        ResponsesRoute::ChatGptPlan,
                        ResponsesProtocol::Standard
                    )),
                    ProviderRequestPurpose::Inference
                ),
                Err(PrepareProviderError::Store(_))
            ));
            assert_eq!(store.provider_response_state(&head).unwrap(), None);
        }
    }

    #[test]
    fn issuance_failure_never_constructs_dispatch_authority() {
        let (store, head) = store();
        store.lock().unwrap().execute_batch("CREATE TRIGGER refuse_provider_issuance BEFORE INSERT ON provider_requests BEGIN SELECT RAISE(ABORT,'injected issuance failure'); END;").unwrap();
        assert!(matches!(
            store.prepare_provider_request(
                &head,
                &head,
                request(),
                &[None],
                None,
                PreparedTransportSelection::component((
                    ResponsesRoute::ChatGptPlan,
                    ResponsesProtocol::Standard
                )),
                ProviderRequestPurpose::Inference
            ),
            Err(PrepareProviderError::Store(_))
        ));
        assert_eq!(store.provider_response_state(&head).unwrap(), None);
    }
}
