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
    /// Convert a completed assistant final-answer message into a parent
    /// mailbox envelope. Other assistant items stay in their normal channel.
    pub fn from_final_answer_item(
        recipient: AgentPath,
        sender: AgentPath,
        item: &serde_json::Value,
        timestamp_ms: i64,
    ) -> Option<Self> {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("message")
            || item.get("role").and_then(serde_json::Value::as_str) != Some("assistant")
            || item.get("phase").and_then(serde_json::Value::as_str) != Some("final_answer")
        {
            return None;
        }
        let payload = item
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|part| {
                (part.get("type")?.as_str()? == "output_text")
                    .then(|| part.get("text")?.as_str())
                    .flatten()
            })
            .collect::<String>();
        Some(Self::final_answer(recipient, sender, payload, timestamp_ms))
    }

    pub fn final_answer(
        recipient: AgentPath,
        sender: AgentPath,
        payload: String,
        timestamp_ms: i64,
    ) -> Self {
        Self {
            kind: EnvelopeType::FinalAnswer,
            recipient,
            sender,
            payload,
            class: DeliveryClass::AtBoundary,
            timestamp_ms,
        }
    }

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

/// Render envelopes in arrival order, coalescing adjacent agent envelopes from
/// the same sender. Non-agent messages are never moved across agent messages:
/// mailbox order is observable conversation order.
pub fn render_coalesced(envelopes: &[Envelope]) -> Vec<(MessageChannel, String)> {
    let mut rendered: Vec<(MessageChannel, String)> = Vec::new();
    for envelope in envelopes {
        let (channel, body) = envelope.render();
        if channel == MessageChannel::Assistant
            && rendered.last().is_some_and(|(prior_channel, prior_body)| {
                *prior_channel == MessageChannel::Assistant
                    && prior_body.starts_with(&format!("## from {}\n", envelope.sender.0))
            })
        {
            let (_, prior_body) = rendered.last_mut().expect("checked above");
            prior_body.push('\n');
            prior_body.push_str(&body);
        } else if channel == MessageChannel::Assistant {
            rendered.push((channel, format!("## from {}\n{}", envelope.sender.0, body)));
        } else {
            rendered.push((channel, body));
        }
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
    fn final_answer_item_becomes_parent_envelope_not_tool_output() {
        let item = serde_json::json!({
            "type":"message",
            "role":"assistant",
            "phase":"final_answer",
            "content":[{"type":"output_text","text":"{\"ok\":"},{"type":"output_text","text":"true}"}]
        });
        let envelope = Envelope::from_final_answer_item(
            AgentPath("/root".into()),
            AgentPath("/root/worker".into()),
            &item,
            55,
        )
        .unwrap();
        assert_eq!(envelope.kind, EnvelopeType::FinalAnswer);
        assert_eq!(envelope.payload, "{\"ok\":true}");
        assert_eq!(envelope.class, DeliveryClass::AtBoundary);
        assert!(
            Envelope::from_final_answer_item(
                AgentPath("/root".into()),
                AgentPath("/root/worker".into()),
                &serde_json::json!({"type":"function_call","name":"finalize"}),
                55
            )
            .is_none()
        );
    }

    #[test]
    fn serde_uses_contract_envelope_names_and_classes() {
        let value = serde_json::to_value(envelope("/root", EnvelopeType::NewTask, "task")).unwrap();
        assert_eq!(value["kind"], "NEW_TASK");
        assert_eq!(value["class"], "at_boundary");
    }

    #[test]
    fn rendering_preserves_arrival_order_across_channels() {
        let inputs = [
            envelope("/a", EnvelopeType::Message, "one"),
            envelope("/operator", EnvelopeType::Message, "user"),
            envelope("/b", EnvelopeType::Message, "two"),
        ];
        assert_eq!(
            render_coalesced(&inputs),
            vec![
                (
                    MessageChannel::Assistant,
                    "## from /a\nMessage Type: MESSAGE\nTask name: /root/worker\nSender: /a\nPayload:\none".into()
                ),
                (MessageChannel::User, "user".into()),
                (
                    MessageChannel::Assistant,
                    "## from /b\nMessage Type: MESSAGE\nTask name: /root/worker\nSender: /b\nPayload:\ntwo".into()
                ),
            ]
        );
    }

    #[test]
    fn adjacent_final_answers_from_one_sender_coalesce_without_changing_payloads() {
        let inputs = [
            envelope("/root/worker", EnvelopeType::FinalAnswer, r#"{"ok":true}"#),
            envelope("/root/worker", EnvelopeType::FinalAnswer, r#"{"ok":false}"#),
        ];
        assert_eq!(
            render_coalesced(&inputs),
            vec![(
                MessageChannel::Assistant,
                concat!(
                    "## from /root/worker\n",
                    "Message Type: FINAL_ANSWER\nTask name: /root/worker\n",
                    "Sender: /root/worker\nPayload:\n{\"ok\":true}\n",
                    "Message Type: FINAL_ANSWER\nTask name: /root/worker\n",
                    "Sender: /root/worker\nPayload:\n{\"ok\":false}"
                )
                .into()
            )]
        );
    }
}
