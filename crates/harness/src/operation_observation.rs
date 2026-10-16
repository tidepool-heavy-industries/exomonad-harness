//! Read-only facts from the existing operation owners. No field grants control.

use crate::{embedding::HostIdentity, model::OperationId};
use serde::{Deserialize, Serialize};

/// Scheduler state is not durable result publication or native cleanup evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationExecution {
    Unknown,
    Queued,
    Running,
    Terminal,
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NativeOperationExecution {
    Unknown,
    Pending,
    Terminal,
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationFinalization {
    Unknown,
    Pending,
    Completed,
    Aborted,
    RetirementAborted,
    NotAdmitted,
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOperationObservation {
    pub execution: NativeOperationExecution,
    pub finalization: OperationFinalization,
}

impl Default for NativeOperationObservation {
    fn default() -> Self {
        Self {
            execution: NativeOperationExecution::Unknown,
            finalization: OperationFinalization::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledOperationObservation {
    pub execution: OperationExecution,
    pub native: NativeOperationObservation,
}

/// A small outcome classification; full retained results use the result reader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    NotStarted,
    Success,
    Failure,
    Cancelled,
    CancelledWithReceipt,
    Interrupted,
    CancellationUnconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationSettlement {
    Pending,
    Retained { outcome: OperationOutcome },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationDelivery {
    Pending,
    /// Present in this observer's effective input history; not proof of attention.
    InHistory,
}

/// The Store facts share one SQLite observation. Live scheduler/native samples
/// are independently labelled; this is not an atomic distributed snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationObservation {
    pub operation: OperationId,
    pub observer: HostIdentity,
    pub execution: OperationExecution,
    pub native_execution: NativeOperationExecution,
    pub finalization: OperationFinalization,
    pub settlement: OperationSettlement,
    pub delivery: OperationDelivery,
    pub history_head: crate::model::RequestId,
    pub provenance: OperationObservationProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationObservationProvenance {
    pub store: bool,
    pub scheduler: bool,
    pub native: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum OperationObservationError {
    #[error("operation is not retained by this current conversation")]
    Unauthorized,
    #[error("operation observation is unavailable: {0}")]
    Unavailable(String),
}
