use super::{HttpDiagnostic, TransportError};
use serde::{Deserialize, Serialize};

/// A request was refused before a provider response began. Stream failures do
/// not establish this boundary and cannot authorize a new request by themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestFailure {
    Authentication,
    CredentialUnavailable {
        route: super::ResponsesRoute,
        reason: super::CredentialFailure,
    },
    ProviderAuthentication {
        route: super::ResponsesRoute,
    },
    Http {
        #[schemars(schema_with = "http_status_schema")]
        status: u16,
        diagnostic: Option<HttpDiagnostic>,
    },
}

fn http_status_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": u16::MIN, "maximum": u16::MAX})
}

/// Exhaustive transport-owned classification; Engine combines these facts with
/// cleanup and persistence evidence instead of interpreting rendered errors.
pub(crate) enum TransportFailureKind {
    Rejected(RequestFailure),
    Interrupted(super::StreamInterruption),
    ProviderFailed {
        event: super::ProviderStreamFailureEvent,
        diagnostic: Option<HttpDiagnostic>,
    },
    ProviderIncomplete {
        response_id: String,
        reason: Option<super::IncompleteReason>,
    },
    Other,
}

impl TransportFailureKind {
    /// The stopped attempt's retained disposition is not a recovery grant.
    /// Store separately checks escaped items, claims, cleanup and lineage.
    pub(crate) fn provider_response_state(&self) -> crate::store::ProviderResponseState {
        use crate::store::ProviderResponseState;
        match self {
            Self::Rejected(_) => ProviderResponseState::Rejected,
            Self::Interrupted(_) => ProviderResponseState::Interrupted,
            Self::ProviderFailed { .. } | Self::ProviderIncomplete { .. } | Self::Other => {
                ProviderResponseState::Failed
            }
        }
    }
}

impl TransportError {
    pub(crate) fn failure_kind(&self) -> TransportFailureKind {
        match self {
            Self::ResponseIncomplete {
                response_id,
                reason,
            } => TransportFailureKind::ProviderIncomplete {
                response_id: response_id.clone(),
                reason: *reason,
            },
            Self::CredentialUnavailable { route, reason } => {
                TransportFailureKind::Rejected(RequestFailure::CredentialUnavailable {
                    route: *route,
                    reason: *reason,
                })
            }
            Self::ProviderAuthentication { route } => {
                TransportFailureKind::Rejected(RequestFailure::ProviderAuthentication {
                    route: *route,
                })
            }
            Self::Authentication => TransportFailureKind::Rejected(RequestFailure::Authentication),
            Self::Http { status, diagnostic } => {
                TransportFailureKind::Rejected(RequestFailure::Http {
                    status: *status,
                    diagnostic: diagnostic.clone(),
                })
            }
            Self::IncompleteResponse(cause) => TransportFailureKind::Interrupted(*cause),
            Self::ProviderStreamFailure { event, diagnostic } => {
                TransportFailureKind::ProviderFailed {
                    event: *event,
                    diagnostic: diagnostic.clone(),
                }
            }
            Self::InvalidAdmission
            | Self::UnsupportedSemantics(_)
            | Self::PreparedSelectionMismatch
            | Self::ReplayContractMismatch
            | Self::UnprovenResponseContract
            | Self::MissingOutputIndex
            | Self::ConflictingOutputIndex { .. }
            | Self::IncompleteOutputIndices { .. }
            | Self::Stream(_)
            | Self::ReplayRequestReuse { .. } => TransportFailureKind::Other,
        }
    }

    pub(crate) fn request_failure(&self) -> Option<RequestFailure> {
        match self.failure_kind() {
            TransportFailureKind::Rejected(failure) => Some(failure),
            TransportFailureKind::Interrupted(_)
            | TransportFailureKind::ProviderFailed { .. }
            | TransportFailureKind::ProviderIncomplete { .. }
            | TransportFailureKind::Other => None,
        }
    }
}
