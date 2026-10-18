//! One admitted Gemini API-key source, shared across all clients in the run.
use super::super::{
    Auth, CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
    ResponsesProtocol, ResponsesRoute, TransportError,
};

#[derive(Clone)]
pub struct GeminiAuth {
    admission: NativeTransportAdmission,
}
impl GeminiAuth {
    pub fn from_reference(
        credential: CredentialReference,
        limits: InferenceLimits,
    ) -> Result<Self, TransportError> {
        Self::from_admission(NativeTransportAdmission::rehydrate(
            NativeTransportContract {
                route: ResponsesRoute::GeminiInteractions,
                protocol: ResponsesProtocol::Interactions,
                endpoint: super::super::client::GEMINI_INTERACTIONS_ENDPOINT.into(),
                credential,
                limits,
            },
        )?)
    }
    pub fn from_admission(admission: NativeTransportAdmission) -> Result<Self, TransportError> {
        admission.contract().validate()?;
        if admission.contract().route != ResponsesRoute::GeminiInteractions {
            return Err(TransportError::InvalidAdmission);
        }
        Ok(Self { admission })
    }
}
impl Auth for GeminiAuth {
    fn admission(&self) -> Result<NativeTransportAdmission, TransportError> {
        Ok(self.admission.clone())
    }
    fn access(&self) -> Result<String, TransportError> {
        super::deepseek::read_api_key(
            &self.admission.contract().credential,
            ResponsesRoute::GeminiInteractions,
        )
    }
}
