use crate::model::OperationId;
use std::sync::{Arc, OnceLock};

/// Host-issued identity for one native request activation. Keep the same clone
/// in every immutable tool snapshot for that request; a successor request gets
/// a new identity even when its schema is identical. This grants no replay or
/// restart authority and is deliberately not serializable.
#[derive(Clone, Debug, Default)]
pub struct RequestActivation {
    pub(crate) state: Arc<ActivationState>,
}

#[derive(Debug, Default)]
pub(crate) struct ActivationState {
    pub(crate) scheduler: OnceLock<Arc<()>>,
    pub(crate) terminal: OnceLock<OperationId>,
}

impl RequestActivation {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn bind_scheduler(&self, owner: &Arc<()>) -> bool {
        Arc::ptr_eq(self.state.scheduler.get_or_init(|| owner.clone()), owner)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.terminal.get().is_some()
    }
}

impl PartialEq for RequestActivation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}
impl Eq for RequestActivation {}

/// Meaning pinned by the issuing host snapshot, never inferred by Harness
/// from a tool's name or result. Only domain work blocks request settlement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InvocationRole {
    #[default]
    Domain,
    Observation,
    Settlement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationState {
    Queued,
    Running,
    /// Scheduler terminal carrier without proof that native work stopped.
    Unconfirmed,
}

/// Existing scheduler inventory, including calls registered before physical
/// provider admission. Native owners may refine Running using their own exact
/// terminal facts; absence of a native entry does not establish completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInvocation {
    pub operation: OperationId,
    pub name: String,
    pub state: InvocationState,
}

/// Issued only while the exact settlement invocation and its native claim hold
/// the scheduler's admission fence. It stops inference for this activation,
/// after ordinary output/context publication, without closing its actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationTerminal {
    pub(crate) operation: OperationId,
    pub(crate) activation: RequestActivation,
}

impl InvocationTerminal {
    pub(crate) fn matches(&self, operation: &OperationId, activation: &RequestActivation) -> bool {
        &self.operation == operation
            && &self.activation == activation
            && activation.state.terminal.get() == Some(operation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationAdmissionError<E> {
    /// A synchronous callback cannot wait for the scheduler lock. Retry through
    /// the owning host boundary; contention never proves an empty inventory.
    Unavailable,
    ForeignInvocation,
    NotSettlement,
    AlreadyTerminated,
    Claim(E),
}
