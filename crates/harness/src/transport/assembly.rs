//! Private native parsing outcomes. Reader interruption and declared provider
//! stops are separate facts; neither grants recovery or replay permission.
use super::{
    HttpDiagnostic, IncompleteReason, ProviderStreamFailureEvent, ResponseTerminal, TransportError,
    TransportObservation, UnsupportedSemantics,
};

#[derive(Debug)]
pub(super) enum ProtocolFault {
    Invalid(&'static str),
    MissingOutputIndex,
    ConflictingOutputIndex { index: u64 },
    IncompleteOutputIndices { index: u64 },
    Unsupported(UnsupportedSemantics),
}
impl ProtocolFault {
    pub(super) fn invalid_input() -> Self {
        Self::Unsupported(UnsupportedSemantics::InputItem)
    }
    pub(super) fn into_error(self) -> TransportError {
        match self {
            Self::Invalid(message) => TransportError::Stream(message.into()),
            Self::MissingOutputIndex => TransportError::MissingOutputIndex,
            Self::ConflictingOutputIndex { index } => {
                TransportError::ConflictingOutputIndex { index }
            }
            Self::IncompleteOutputIndices { index } => {
                TransportError::IncompleteOutputIndices { index }
            }
            Self::Unsupported(semantics) => TransportError::UnsupportedSemantics(semantics),
        }
    }
}

#[derive(Debug)]
pub(super) enum DeclaredStop {
    Incomplete {
        response_id: String,
        reason: Option<IncompleteReason>,
    },
    Failed {
        event: ProviderStreamFailureEvent,
        diagnostic: Option<HttpDiagnostic>,
    },
}
impl DeclaredStop {
    /// Issue the public failure and diagnostic from the same native fact. The
    /// Engine/Store separately determine disposition, cleanup and recovery.
    pub(super) fn into_error(
        self,
        observation: Option<&mut TransportObservation>,
    ) -> TransportError {
        let (terminal, error) = match self {
            Self::Incomplete {
                response_id,
                reason,
            } => (
                ResponseTerminal::Incomplete {
                    response_id: response_id.clone(),
                    reason,
                },
                TransportError::ResponseIncomplete {
                    response_id,
                    reason,
                },
            ),
            Self::Failed { event, diagnostic } => (
                match event {
                    ProviderStreamFailureEvent::ResponseFailed => ResponseTerminal::ResponseFailed,
                    ProviderStreamFailureEvent::InteractionFailed => {
                        ResponseTerminal::InteractionFailed
                    }
                    ProviderStreamFailureEvent::InteractionCancelled => {
                        ResponseTerminal::InteractionCancelled
                    }
                    ProviderStreamFailureEvent::Error => ResponseTerminal::Error,
                },
                TransportError::ProviderStreamFailure { event, diagnostic },
            ),
        };
        if let Some(observation) = observation {
            observation.terminal = Some(terminal);
        }
        error
    }
}

#[derive(Debug)]
pub(super) enum AssemblyAdvance<Events> {
    Progress(Events),
    /// The native terminal was recognized. Local item validation is still
    /// required before the reader may issue a successful ResponsesTurn.
    Complete(Events),
    Stopped(DeclaredStop),
}
impl<Events> AssemblyAdvance<Events> {
    pub(super) fn map_events<Mapped>(
        self,
        map: impl FnOnce(Events) -> Mapped,
    ) -> AssemblyAdvance<Mapped> {
        match self {
            Self::Progress(events) => AssemblyAdvance::Progress(map(events)),
            Self::Complete(events) => AssemblyAdvance::Complete(map(events)),
            Self::Stopped(stop) => AssemblyAdvance::Stopped(stop),
        }
    }

    /// Compatibility view for the public Responses parser; native readers
    /// instead consume the closed outcome and validate completion immediately.
    pub(super) fn into_events(
        self,
        observation: Option<&mut TransportObservation>,
    ) -> Result<Events, TransportError> {
        match self {
            Self::Progress(events) | Self::Complete(events) => Ok(events),
            Self::Stopped(stop) => Err(stop.into_error(observation)),
        }
    }
}
