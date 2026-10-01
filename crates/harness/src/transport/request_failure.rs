use super::{HttpDiagnostic, TransportError};
use serde::{Deserialize, Serialize};

/// A request was refused before a provider response began. Stream failures do
/// not establish this boundary and cannot authorize a new request by themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestFailure {
    Authentication,
    Http {
        status: u16,
        diagnostic: Option<HttpDiagnostic>,
    },
}

impl TransportError {
    pub(crate) fn request_failure(&self) -> Option<RequestFailure> {
        match self {
            Self::Authentication => Some(RequestFailure::Authentication),
            Self::Http { status, diagnostic } => Some(RequestFailure::Http {
                status: *status,
                diagnostic: diagnostic.clone(),
            }),
            _ => None,
        }
    }
}
