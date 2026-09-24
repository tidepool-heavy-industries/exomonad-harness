//! Mailbox envelopes and their model-facing rendering.
//!
//! Tool results are deliberately not represented here: they remain call outputs
//! on their original call ids. Envelopes are the separate message channel.
use crate::model::AgentPath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnvelopeType {
    NewTask,
    Message,
    FinalAnswer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryClass {
    Steer,
    AtBoundary,
    Hold,
}

/// The conversation role and body used to deliver an envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageChannel {
    Assistant,
    User,
    Developer,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub kind: EnvelopeType,
    pub recipient: AgentPath,
    pub sender: AgentPath,
    pub payload: String,
    pub class: DeliveryClass,
    pub timestamp_ms: i64,
}

impl Envelope {
    /// The default scheduling class for an envelope's sender.
    ///
    /// A later mailbox hook may change this. The `Steer` restriction to
    /// operator/WebSocket delivery and transport downgrades belong to the host.
    pub fn default_class(sender: &AgentPath) -> DeliveryClass {
        if sender.0 == "/operator" {
            DeliveryClass::Steer
        } else {
            DeliveryClass::AtBoundary
        }
    }

    /// Return the role and body for the distinct envelope message channel.
    /// Agent/job messages carry headers; operator input is plain user content;
    /// harness notices are developer content.
    pub fn render(&self) -> (MessageChannel, String) {
        match self.sender.0.as_str() {
            "/operator" => (MessageChannel::User, self.payload.clone()),
            "/harness" => (MessageChannel::Developer, self.payload.clone()),
            _ => (
                MessageChannel::Assistant,
                format!(
                    "Message Type: {}\nTask name: {}\nSender: {}\nPayload:\n{}",
                    self.kind, self.recipient.0, self.sender.0, self.payload
                ),
            ),
        }
    }
}

impl std::fmt::Display for EnvelopeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NewTask => "NEW_TASK",
            Self::Message => "MESSAGE",
            Self::FinalAnswer => "FINAL_ANSWER",
        })
    }
}

/// Coalesce envelopes without changing their arrival order.
pub fn render_coalesced(envelopes: &[Envelope]) -> Vec<(MessageChannel, String)> {
    let mut rendered = Vec::new();
    let mut agents = Vec::<(&str, String)>::new();
    for envelope in envelopes {
        let (channel, body) = envelope.render();
        if channel == MessageChannel::Assistant {
            agents.push((&envelope.sender.0, envelope.payload.clone()));
        } else {
            rendered.push((channel, body));
        }
    }
    if !agents.is_empty() {
        rendered.push((
            MessageChannel::Assistant,
            agents
                .into_iter()
                .map(|(sender, payload)| format!("## from {sender}\n{payload}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(sender: &str, kind: EnvelopeType, payload: &str) -> Envelope {
        Envelope {
            kind,
            recipient: AgentPath("/root/worker".into()),
            sender: AgentPath(sender.into()),
            payload: payload.into(),
            class: DeliveryClass::AtBoundary,
            timestamp_ms: 42,
        }
    }

    #[test]
    fn sender_defaults_and_channels_are_distinct() {
        let operator = envelope("/operator", EnvelopeType::Message, "hello");
        assert_eq!(
            Envelope::default_class(&operator.sender),
            DeliveryClass::Steer
        );
        assert_eq!(operator.render(), (MessageChannel::User, "hello".into()));
        let harness = envelope("/harness", EnvelopeType::Message, "budget");
        assert_eq!(
            harness.render(),
            (MessageChannel::Developer, "budget".into())
        );
        let agent = envelope("/root/worker", EnvelopeType::FinalAnswer, "{}");
        assert_eq!(
            Envelope::default_class(&agent.sender),
            DeliveryClass::AtBoundary
        );
        assert_eq!(
            agent.render().1,
            "Message Type: FINAL_ANSWER\nTask name: /root/worker\nSender: /root/worker\nPayload:\n{}"
        );
    }

    #[test]
    fn serde_uses_contract_envelope_names_and_classes() {
        let value = serde_json::to_value(envelope("/root", EnvelopeType::NewTask, "task")).unwrap();
        assert_eq!(value["kind"], "NEW_TASK");
        assert_eq!(value["class"], "at_boundary");
    }

    #[test]
    fn coalescing_keeps_agent_arrival_order_and_separates_non_agent_messages() {
        let inputs = [
            envelope("/a", EnvelopeType::Message, "one"),
            envelope("/operator", EnvelopeType::Message, "user"),
            envelope("/b", EnvelopeType::Message, "two"),
        ];
        assert_eq!(
            render_coalesced(&inputs),
            vec![
                (MessageChannel::User, "user".into()),
                (
                    MessageChannel::Assistant,
                    "## from /a\none\n## from /b\ntwo".into()
                ),
            ]
        );
    }
}
