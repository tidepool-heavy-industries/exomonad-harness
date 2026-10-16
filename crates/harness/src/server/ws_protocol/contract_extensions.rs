//! Browser observations and commands for the native form and recovery owners.
use super::contract::{ActorOutputOrigin, ActorOutputProjection, JsonValue};
use super::{CommandControl, HostActorIdentity, WireI64, WireU64};
use crate::{embedding as native, model::ConversationIdentity, store::presentation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InferenceBlock {
    pub request: String,
    pub generation: WireU64,
    pub reason: native::InferenceBlockReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_refusal: Option<native::ContinuationRefusal>,
}
impl From<&native::InferenceBlock> for InferenceBlock {
    fn from(value: &native::InferenceBlock) -> Self {
        Self {
            request: value.request.0.clone(),
            generation: value.generation.into(),
            reason: value.reason.clone(),
            continuation_refusal: value.continuation_refusal,
        }
    }
}
impl From<InferenceBlock> for native::InferenceBlock {
    fn from(value: InferenceBlock) -> Self {
        Self {
            request: crate::model::RequestId(value.request),
            generation: value.generation.get(),
            reason: value.reason,
            continuation_refusal: value.continuation_refusal,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HostInferenceState {
    AutoRetry {
        block: InferenceBlock,
        #[schemars(schema_with = "retry_count_schema")]
        attempt: u8,
        #[schemars(schema_with = "retry_count_schema")]
        maximum: u8,
        #[schemars(schema_with = "retry_delay_schema")]
        delay_ms: u32,
    },
    Idle,
    Running,
    Blocked {
        block: InferenceBlock,
    },
    Recovering {
        operation_id: native::ClientOperationId,
        block: InferenceBlock,
    },
}
fn retry_count_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"integer", "minimum":1, "maximum":2})
}
fn retry_delay_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"integer", "enum":[1000,3000]})
}
impl From<&native::HostInferenceState> for HostInferenceState {
    fn from(value: &native::HostInferenceState) -> Self {
        match value {
            native::HostInferenceState::AutoRetry {
                block,
                attempt,
                maximum,
                delay_ms,
            } => Self::AutoRetry {
                block: block.into(),
                attempt: *attempt,
                maximum: *maximum,
                delay_ms: *delay_ms,
            },
            native::HostInferenceState::Idle => Self::Idle,
            native::HostInferenceState::Running => Self::Running,
            native::HostInferenceState::Blocked { block } => Self::Blocked {
                block: block.into(),
            },
            native::HostInferenceState::Recovering {
                operation_id,
                block,
            } => Self::Recovering {
                operation_id: *operation_id,
                block: block.into(),
            },
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostCommand {
    SelectAccount {
        target: HostActorIdentity,
        account_id: crate::transport::auth::chatgpt::SavedAccountId,
    },
    Input {
        target: HostActorIdentity,
        text: String,
    },
    Interrupt {
        target: HostActorIdentity,
        expected_round: native::EmbeddedRoundId,
    },
    Retire {
        target: HostActorIdentity,
    },
    Recover {
        target: HostActorIdentity,
        expected_block: InferenceBlock,
        mode: native::RecoveryMode,
    },
}
impl From<&crate::server::HostCommand> for HostCommand {
    fn from(value: &crate::server::HostCommand) -> Self {
        use crate::server::HostCommand as N;
        match value {
            N::SelectAccount { target, account_id } => Self::SelectAccount {
                target: target.clone(),
                account_id: account_id.clone(),
            },
            N::Input { target, text } => Self::Input {
                target: target.clone(),
                text: text.clone(),
            },
            N::Interrupt {
                target,
                expected_round,
            } => Self::Interrupt {
                target: target.clone(),
                expected_round: *expected_round,
            },
            N::Retire { target } => Self::Retire {
                target: target.clone(),
            },
            N::Recover {
                target,
                expected_block,
                mode,
            } => Self::Recover {
                target: target.clone(),
                expected_block: expected_block.into(),
                mode: *mode,
            },
        }
    }
}
impl From<HostCommand> for crate::server::HostCommand {
    fn from(value: HostCommand) -> Self {
        match value {
            HostCommand::SelectAccount { target, account_id } => {
                Self::SelectAccount { target, account_id }
            }
            HostCommand::Input { target, text } => Self::Input { target, text },
            HostCommand::Interrupt {
                target,
                expected_round,
            } => Self::Interrupt {
                target,
                expected_round,
            },
            HostCommand::Retire { target } => Self::Retire { target },
            HostCommand::Recover {
                target,
                expected_block,
                mode,
            } => Self::Recover {
                target,
                expected_block: expected_block.into(),
                mode,
            },
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryProjectionReceipt {
    pub omitted: Vec<String>,
    pub unknown_response_completeness: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommandReceipt {
    pub command_id: String,
    #[serde(flatten)]
    pub outcome: CommandReceiptOutcome,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InputContinuation {
    Queued,
    Granted {
        block: InferenceBlock,
    },
    Refused {
        block: InferenceBlock,
        reason: native::ContinuationRefusal,
    },
}
impl From<&native::InputContinuation> for InputContinuation {
    fn from(value: &native::InputContinuation) -> Self {
        match value {
            native::InputContinuation::Queued => Self::Queued,
            native::InputContinuation::Granted { block } => Self::Granted {
                block: block.into(),
            },
            native::InputContinuation::Refused { block, reason } => Self::Refused {
                block: block.into(),
                reason: *reason,
            },
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommandReceiptOutcome {
    AccountSelected {
        target: HostActorIdentity,
        account_id: crate::transport::auth::chatgpt::SavedAccountId,
    },
    RecoveryAuthorized {
        target: HostActorIdentity,
        block: InferenceBlock,
        mode: native::RecoveryMode,
        projection: RecoveryProjectionReceipt,
    },
    ContinuationRefused {
        target: HostActorIdentity,
        block: InferenceBlock,
        reason: native::ContinuationRefusal,
    },
    Admitted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuation: Option<InputContinuation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<HostActorIdentity>,
        envelope_id: WireI64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake_error: Option<String>,
    },
    ControlRequested {
        target: HostActorIdentity,
        control: CommandControl,
    },
    Unconfirmed {
        target: HostActorIdentity,
        reason: String,
    },
    Refused {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<HostActorIdentity>,
        reason: String,
    },
}
impl From<&super::CommandReceipt> for CommandReceipt {
    fn from(value: &super::CommandReceipt) -> Self {
        use super::CommandReceiptOutcome as N;
        let outcome = match &value.outcome {
            N::AccountSelected { target, account_id } => CommandReceiptOutcome::AccountSelected {
                target: target.clone(),
                account_id: account_id.clone(),
            },
            N::RecoveryAuthorized {
                target,
                block,
                mode,
                projection,
            } => CommandReceiptOutcome::RecoveryAuthorized {
                target: target.clone(),
                block: block.into(),
                mode: *mode,
                projection: RecoveryProjectionReceipt {
                    omitted: projection
                        .omitted
                        .iter()
                        .map(|reference| reference.as_str().to_owned())
                        .collect(),
                    unknown_response_completeness: projection
                        .unknown_response_completeness
                        .iter()
                        .map(|request| request.0.clone())
                        .collect(),
                },
            },
            N::ContinuationRefused {
                target,
                block,
                reason,
            } => CommandReceiptOutcome::ContinuationRefused {
                target: target.clone(),
                block: block.into(),
                reason: *reason,
            },
            N::Admitted {
                continuation,
                target,
                envelope_id,
                wake_error,
            } => CommandReceiptOutcome::Admitted {
                continuation: continuation.as_ref().map(Into::into),
                target: target.clone(),
                envelope_id: *envelope_id,
                wake_error: wake_error.clone(),
            },
            N::ControlRequested { target, control } => CommandReceiptOutcome::ControlRequested {
                target: target.clone(),
                control: *control,
            },
            N::Unconfirmed { target, reason } => CommandReceiptOutcome::Unconfirmed {
                target: target.clone(),
                reason: reason.clone(),
            },
            N::Refused { target, reason } => CommandReceiptOutcome::Refused {
                target: target.clone(),
                reason: reason.clone(),
            },
        };
        Self {
            command_id: value.command_id.clone(),
            outcome,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ActorOutputExecution {
    Notebook {
        execution: String,
        input_unit_index: WireU64,
        effect_ordinal: WireU64,
    },
    ActorProgram,
}
impl From<&crate::store::actor_output::ActorOutputExecution> for ActorOutputExecution {
    fn from(value: &crate::store::actor_output::ActorOutputExecution) -> Self {
        use crate::store::actor_output::ActorOutputExecution as N;
        match value {
            N::ActorProgram => Self::ActorProgram,
            N::Notebook {
                execution,
                input_unit_index,
                effect_ordinal,
            } => Self::Notebook {
                execution: execution.clone(),
                input_unit_index: (*input_unit_index).into(),
                effect_ordinal: (*effect_ordinal).into(),
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorFormOpen {
    pub origin: ActorOutputOrigin,
    pub mount_id: String,
    pub execution: ActorOutputExecution,
    pub conversation: Option<ConversationIdentity>,
    pub form: presentation::FormSpec,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredActorForm {
    pub sequence: WireI64,
    pub revision_sequence: WireI64,
    pub opening: ActorFormOpen,
    pub state: crate::store::forms::ActorFormState,
    pub attempt_id: Option<String>,
    pub draft: Option<presentation::FormDraft>,
    pub errors: Vec<presentation::FormError>,
    pub answer: Option<presentation::View>,
}
impl From<&crate::store::forms::StoredActorForm> for StoredActorForm {
    fn from(value: &crate::store::forms::StoredActorForm) -> Self {
        Self {
            sequence: value.sequence.into(),
            revision_sequence: value.revision_sequence.into(),
            opening: ActorFormOpen {
                origin: (&value.opening.origin).into(),
                mount_id: value.opening.mount_id.clone(),
                execution: (&value.opening.execution).into(),
                conversation: value.opening.conversation.clone(),
                form: value.opening.form.clone(),
            },
            state: value.state.clone(),
            attempt_id: value.attempt_id.clone(),
            draft: value.draft.clone(),
            errors: value.errors.clone(),
            answer: value.answer.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingActorForm {
    pub mount_id: String,
    /// Opening journal position, usable as the exact chat locator.
    pub sequence: WireI64,
    pub revision_sequence: WireI64,
    pub execution: ActorOutputExecution,
    pub conversation: Option<ConversationIdentity>,
    pub state: crate::store::forms::PendingActorFormState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingActorFormsPage {
    pub origin: ActorOutputOrigin,
    pub revision_sequence: WireI64,
    pub forms: Vec<PendingActorForm>,
    pub next_after: Option<WireI64>,
}
impl From<&crate::store::forms::PendingActorFormsPage> for PendingActorFormsPage {
    fn from(page: &crate::store::forms::PendingActorFormsPage) -> Self {
        Self {
            origin: (&page.origin).into(),
            revision_sequence: page.revision_sequence.into(),
            forms: page
                .forms
                .iter()
                .map(|form| PendingActorForm {
                    mount_id: form.mount_id.clone(),
                    sequence: form.sequence.into(),
                    revision_sequence: form.revision_sequence.into(),
                    execution: (&form.execution).into(),
                    conversation: form.conversation.clone(),
                    state: form.state,
                })
                .collect(),
            next_after: page.next_after.map(Into::into),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorFormSubmit {
    pub origin: ActorOutputOrigin,
    pub mount_id: String,
    pub operation_id: String,
    pub draft: presentation::FormDraft,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorFormDismiss {
    pub origin: ActorOutputOrigin,
    pub mount_id: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatOperation {
    pub origin: ConversationIdentity,
    pub request: String,
    pub call: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatInvocation {
    pub request: String,
    pub position: WireI64,
    pub hash: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatInputOperation {
    pub envelope_id: WireI64,
    pub operation_id: String,
    pub target: HostActorIdentity,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatTool {
    pub operation: ChatOperation,
    pub invocation: ChatInvocation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ChatEntry {
    Message {
        sequence: WireI64,
        request_id: String,
        position: WireI64,
        hash: String,
        item: JsonValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_item: Option<JsonValue>,
        tool: Option<ChatTool>,
        input_operation: Option<ChatInputOperation>,
        provider_item_id: Option<String>,
    },
    Output {
        sequence: WireI64,
        output: ActorOutputProjection,
    },
    Form {
        sequence: WireI64,
        form: StoredActorForm,
    },
    Oversized {
        sequence: WireI64,
        request_id: String,
        position: WireI64,
        hash: String,
        byte_len: WireU64,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatPage {
    pub origin: ActorOutputOrigin,
    pub cutover_sequence: WireI64,
    pub legacy_history: bool,
    pub entries: Vec<ChatEntry>,
    pub next_after: Option<WireI64>,
    pub previous_before: Option<WireI64>,
}
impl From<&crate::store::chat::ChatPage> for ChatPage {
    fn from(value: &crate::store::chat::ChatPage) -> Self {
        use crate::store::chat::ChatEntry as N;
        let entries = value
            .entries
            .iter()
            .map(|entry| match entry {
                N::Message {
                    sequence,
                    request_id,
                    position,
                    hash,
                    item,
                    model_item,
                    tool,
                    input_operation,
                    provider_item_id,
                } => ChatEntry::Message {
                    sequence: (*sequence).into(),
                    request_id: request_id.clone(),
                    position: (*position).into(),
                    hash: hash.clone(),
                    item: item.clone().into(),
                    model_item: model_item.as_ref().map(|item| item.clone().into()),
                    provider_item_id: provider_item_id.clone(),
                    input_operation: input_operation.as_ref().map(|input| ChatInputOperation {
                        envelope_id: input.envelope_id.into(),
                        operation_id: input.operation_id.clone(),
                        target: input.target.clone(),
                    }),
                    tool: tool.as_ref().map(|tool| ChatTool {
                        operation: ChatOperation {
                            origin: tool.operation.origin.clone(),
                            request: tool.operation.request.0.clone(),
                            call: tool.operation.call.0.clone(),
                        },
                        invocation: ChatInvocation {
                            request: tool.invocation.request.clone(),
                            position: tool.invocation.position.into(),
                            hash: tool.invocation.hash.clone(),
                        },
                    }),
                },
                N::Output { sequence, output } => ChatEntry::Output {
                    sequence: (*sequence).into(),
                    output: output.into(),
                },
                N::Form { sequence, form } => ChatEntry::Form {
                    sequence: (*sequence).into(),
                    form: form.into(),
                },
                N::Oversized {
                    sequence,
                    request_id,
                    position,
                    hash,
                    byte_len,
                } => ChatEntry::Oversized {
                    sequence: (*sequence).into(),
                    request_id: request_id.clone(),
                    position: (*position).into(),
                    hash: hash.clone(),
                    byte_len: (*byte_len as u64).into(),
                },
            })
            .collect();
        Self {
            origin: (&value.origin).into(),
            cutover_sequence: value.cutover_sequence.into(),
            legacy_history: value.legacy_history,
            entries,
            next_after: value.next_after.map(Into::into),
            previous_before: value.previous_before.map(Into::into),
        }
    }
}

pub(super) fn extension_samples() -> serde_json::Value {
    let form = StoredActorForm {
        sequence: 9_007_199_254_740_993i64.into(),
        revision_sequence: i64::MAX.into(),
        opening: ActorFormOpen {
            origin: ActorOutputOrigin {
                run: "run".into(),
                native_actor: u64::MAX.into(),
                incarnation: 9_007_199_254_740_995u64.into(),
            },
            mount_id: "mount".into(),
            execution: ActorOutputExecution::Notebook {
                execution: "execution".into(),
                input_unit_index: u64::MAX.into(),
                effect_ordinal: 9_007_199_254_740_997u64.into(),
            },
            conversation: None,
            form: presentation::FormSpec {
                version: 1,
                root: presentation::FormNode::Text {
                    id: "name".into(),
                    label: "Name".into(),
                    initial: None,
                },
            },
        },
        state: crate::store::forms::ActorFormState::Open,
        attempt_id: None,
        draft: None,
        errors: vec![],
        answer: None,
    };
    let page = ChatPage {
        origin: form.opening.origin.clone(),
        cutover_sequence: 0i64.into(),
        legacy_history: false,
        entries: vec![
            ChatEntry::Form {
                sequence: form.sequence,
                form: form.clone(),
            },
            ChatEntry::Message {
                model_item: None,
                sequence: i64::MAX.into(),
                request_id: "request".into(),
                position: i64::MAX.into(),
                hash: "a".repeat(64),
                item: serde_json::json!([null, "opaque"]).into(),
                tool: None,
                provider_item_id: None,
                input_operation: Some(ChatInputOperation {
                    envelope_id: i64::MAX.into(),
                    operation_id: uuid::Uuid::nil().to_string(),
                    target: HostActorIdentity {
                        run: "run".into(),
                        actor: crate::model::AgentPath("/root".into()),
                        incarnation: "incarnation".into(),
                    },
                }),
            },
        ],
        next_after: Some(i64::MAX.into()),
        previous_before: Some(form.sequence),
    };
    serde_json::json!({ "pendingActorForms": [PendingActorFormsPage {
        origin: form.opening.origin.clone(), revision_sequence: form.revision_sequence,
        forms: vec![PendingActorForm {
            mount_id: form.opening.mount_id.clone(), sequence: form.sequence,
            revision_sequence: form.revision_sequence, execution: form.opening.execution.clone(),
            conversation: form.opening.conversation.clone(), state: crate::store::forms::PendingActorFormState::Open,
        }], next_after: None,
    }], "actorForm": [form.clone()], "chat": [page], "actorFormSubmit": [ActorFormSubmit { origin: form.opening.origin.clone(), mount_id: form.opening.mount_id.clone(), operation_id: "operation".into(), draft: Default::default() }], "actorFormDismiss": [ActorFormDismiss { origin: form.opening.origin, mount_id: form.opening.mount_id, operation_id: "operation".into() }] })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_generation_and_form_revisions_are_exact_decimal_observations() {
        let native = native::InferenceBlock {
            continuation_refusal: None,
            request: crate::model::RequestId("request".into()),
            generation: u64::MAX,
            reason: native::InferenceBlockReason::ContinuityUnproven,
        };
        let block = InferenceBlock::from(&native);
        let encoded = serde_json::to_value(&block).unwrap();
        assert_eq!(encoded["generation"], u64::MAX.to_string());
        assert_eq!(
            native::InferenceBlock::from(
                serde_json::from_value::<InferenceBlock>(encoded.clone()).unwrap()
            ),
            native
        );
        let mut lossy = encoded;
        lossy["generation"] = serde_json::json!(9_007_199_254_740_993u64);
        assert!(serde_json::from_value::<InferenceBlock>(lossy).is_err());
        let samples = extension_samples();
        let form = &samples["actorForm"][0];
        assert_eq!(form["sequence"], "9007199254740993");
        assert_eq!(form["revisionSequence"], i64::MAX.to_string());
        assert_eq!(
            form["opening"]["origin"]["nativeActor"],
            u64::MAX.to_string()
        );
        assert_eq!(
            form["opening"]["execution"]["inputUnitIndex"],
            u64::MAX.to_string()
        );
        let mut lossy = form.clone();
        lossy["revisionSequence"] = serde_json::json!(i64::MAX);
        assert!(serde_json::from_value::<StoredActorForm>(lossy).is_err());
        assert_eq!(
            samples["chat"][0]["entries"][1]["inputOperation"]["envelopeId"],
            i64::MAX.to_string()
        );
        let mut lossy_page = samples["chat"][0].clone();
        lossy_page["entries"][1]["inputOperation"]["envelopeId"] = serde_json::json!(i64::MAX);
        assert!(serde_json::from_value::<ChatPage>(lossy_page).is_err());
        let page: ChatPage = serde_json::from_value(samples["chat"][0].clone()).unwrap();
        assert_eq!(serde_json::to_value(page).unwrap(), samples["chat"][0]);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientCommand {
    Submit {
        command: String,
    },
    Host {
        operation_id: native::ClientOperationId,
        command: HostCommand,
    },
}
impl From<ClientCommand> for crate::server::ClientCommand {
    fn from(value: ClientCommand) -> Self {
        match value {
            ClientCommand::Submit { command } => Self::Submit { command },
            ClientCommand::Host {
                operation_id,
                command,
            } => Self::Host {
                operation_id,
                command: command.into(),
            },
        }
    }
}

impl ChatEntry {
    fn sequence(&self) -> WireI64 {
        match self {
            Self::Message { sequence, .. }
            | Self::Output { sequence, .. }
            | Self::Form { sequence, .. }
            | Self::Oversized { sequence, .. } => *sequence,
        }
    }
}
impl ChatPage {
    /// Decimal metadata changes the encoded size. Retain the query's near edge
    /// and issue the cursor from the actual retained entries after projection.
    pub fn bounded(mut self, backward: bool) -> crate::store::Result<Self> {
        while serde_json::to_vec(&self)?.len() > crate::store::history::MAX_HISTORY_BYTES {
            if self.entries.len() <= 1 {
                return Err(crate::store::StoreError::InvalidActorOutput);
            }
            if backward {
                self.entries.remove(0);
                self.previous_before = self.entries.first().map(ChatEntry::sequence);
            } else {
                self.entries.pop();
                self.next_after = self.entries.last().map(ChatEntry::sequence);
            }
        }
        Ok(self)
    }
}

#[cfg(test)]
mod chat_bounds_tests {
    use super::*;
    #[test]
    fn encoded_page_bound_retains_the_query_edge_and_issues_its_exact_cursor() {
        let mut page: ChatPage =
            serde_json::from_value(extension_samples()["chat"][0].clone()).unwrap();
        let message = |sequence: i64| ChatEntry::Message {
            model_item: None,
            sequence: sequence.into(),
            request_id: "request".into(),
            position: i64::MAX.into(),
            hash: "a".repeat(64),
            item: serde_json::json!("x".repeat(crate::store::history::MAX_HISTORY_BYTES / 2))
                .into(),
            tool: None,
            input_operation: None,
            provider_item_id: None,
        };
        page.entries = vec![
            message(9_007_199_254_740_993),
            message(9_007_199_254_740_994),
        ];
        let forward = page.clone().bounded(false).unwrap();
        assert_eq!(forward.entries.len(), 1);
        assert_eq!(forward.next_after.unwrap().get(), 9_007_199_254_740_993);
        assert!(
            serde_json::to_vec(&forward).unwrap().len() <= crate::store::history::MAX_HISTORY_BYTES
        );
        let backward = page.bounded(true).unwrap();
        assert_eq!(backward.entries.len(), 1);
        assert_eq!(
            backward.previous_before.unwrap().get(),
            9_007_199_254_740_994
        );
        assert!(
            serde_json::to_vec(&backward).unwrap().len()
                <= crate::store::history::MAX_HISTORY_BYTES
        );
    }
}
