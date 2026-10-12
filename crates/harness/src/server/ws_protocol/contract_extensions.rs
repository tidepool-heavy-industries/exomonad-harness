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
}
impl From<&native::InferenceBlock> for InferenceBlock {
    fn from(value: &native::InferenceBlock) -> Self {
        Self {
            request: value.request.0.clone(),
            generation: value.generation.into(),
            reason: value.reason.clone(),
        }
    }
}
impl From<InferenceBlock> for native::InferenceBlock {
    fn from(value: InferenceBlock) -> Self {
        Self {
            request: crate::model::RequestId(value.request),
            generation: value.generation.get(),
            reason: value.reason,
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
impl From<&native::HostInferenceState> for HostInferenceState {
    fn from(value: &native::HostInferenceState) -> Self {
        match value {
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
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommandReceiptOutcome {
    RecoveryAuthorized {
        target: HostActorIdentity,
        block: InferenceBlock,
        mode: native::RecoveryMode,
        projection: RecoveryProjectionReceipt,
    },
    Admitted {
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
            N::Admitted {
                target,
                envelope_id,
                wake_error,
            } => CommandReceiptOutcome::Admitted {
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
        tool: Option<ChatTool>,
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
                    tool,
                } => ChatEntry::Message {
                    sequence: (*sequence).into(),
                    request_id: request_id.clone(),
                    position: (*position).into(),
                    hash: hash.clone(),
                    item: item.clone().into(),
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
                sequence: i64::MAX.into(),
                request_id: "request".into(),
                position: i64::MAX.into(),
                hash: "a".repeat(64),
                item: serde_json::json!([null, "opaque"]).into(),
                tool: None,
            },
        ],
        next_after: Some(i64::MAX.into()),
        previous_before: Some(form.sequence),
    };
    serde_json::json!({ "actorForm": [form.clone()], "chat": [page], "actorFormSubmit": [ActorFormSubmit { origin: form.opening.origin.clone(), mount_id: form.opening.mount_id.clone(), operation_id: "operation".into(), draft: Default::default() }], "actorFormDismiss": [ActorFormDismiss { origin: form.opening.origin, mount_id: form.opening.mount_id, operation_id: "operation".into() }] })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_generation_and_form_revisions_are_exact_decimal_observations() {
        let native = native::InferenceBlock {
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
