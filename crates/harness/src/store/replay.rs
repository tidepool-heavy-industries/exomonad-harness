//! Exact issued windows reference immutable Store bytes, never later history.
use super::{RecordedReplayTurn, Result, Store, StoreError, utc_millis};
use crate::{
    item::{Item, ItemHash},
    model::{Effort, RequestId},
    transport::{ResponsesRequest, ResponsesTurn, Usage},
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

const FORMAT: u32 = 1;

/// Sealed before transport starts; later output cannot change these references.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssuedReplayRequest {
    input: Vec<ItemHash>,
    instructions: ItemHash,
    tools: ItemHash,
    tools_allowed: Option<Vec<String>>,
    model: String,
    pinned_effort: Effort,
    session_id: String,
    #[cfg(test)]
    #[serde(skip)]
    input_reencodings: usize,
}

#[derive(Serialize, Deserialize)]
struct ReplayRecord {
    format: u32,
    request: RequestId,
    issued: IssuedReplayRequest,
    response: ReplayResponse,
}

#[derive(Serialize, Deserialize)]
struct ReplayResponse {
    response_id: String,
    items: Vec<ItemHash>,
    usage: Usage,
}

fn read_item(tx: &Transaction<'_>, hash: &ItemHash) -> Result<Item> {
    let json: String = tx
        .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| StoreError::MissingReplayItem(hash.0.clone()))?;
    Ok(serde_json::from_str(&json)?)
}

impl Store {
    /// Intern an exact attempt, including transient hook or projection Items.
    pub fn seal_replay_request(&self, request: &ResponsesRequest) -> Result<IssuedReplayRequest> {
        self.seal_replay_request_with_hashes(request, &vec![None; request.input.len()], None)
    }

    pub(crate) fn intern_replay_instructions(&self, instructions: &str) -> Result<ItemHash> {
        self.put_item(&Item(serde_json::Value::String(instructions.to_owned())))
    }

    pub(crate) fn seal_replay_request_with_hashes(
        &self,
        request: &ResponsesRequest,
        hashes: &[Option<ItemHash>],
        instructions: Option<&ItemHash>,
    ) -> Result<IssuedReplayRequest> {
        assert_eq!(
            hashes.len(),
            request.input.len(),
            "Engine input provenance must align"
        );
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        #[cfg(test)]
        let mut input_reencodings = 0;
        let input = request
            .input
            .iter()
            .zip(hashes)
            .map(|(item, hash)| {
                hash.clone().map_or_else(
                    || {
                        #[cfg(test)]
                        {
                            input_reencodings += 1;
                        }
                        Self::put_item_tx(&tx, item)
                    },
                    Ok,
                )
            })
            .collect::<Result<_>>()?;
        let instructions = match instructions {
            Some(hash) => hash.clone(),
            None => Self::put_item_tx(
                &tx,
                &Item(serde_json::Value::String(request.instructions.clone())),
            )?,
        };
        let (tools, json) = request.tools.encoded();
        let present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE hash=?1)",
            [&tools.0],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(
                "INSERT INTO items(hash,json) VALUES (?1,?2)",
                params![tools.0, json],
            )?;
        }
        let issued = IssuedReplayRequest {
            input,
            instructions,
            tools: tools.clone(),
            tools_allowed: request.tools_allowed.clone(),
            model: request.model.clone(),
            pinned_effort: request.pinned_effort,
            session_id: request.session_id.clone(),
            #[cfg(test)]
            input_reencodings,
        };
        tx.commit()?;
        Ok(issued)
    }

    /// Completion and references commit atomically; no request payload is copied.
    pub fn record_issued_replay_turn(
        &self,
        request: &RequestId,
        issued: IssuedReplayRequest,
        response: &ResponsesTurn,
    ) -> Result<i64> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let items = response
            .items
            .iter()
            .map(|item| Self::put_item_tx(&tx, item))
            .collect::<Result<_>>()?;
        let record = ReplayRecord {
            format: FORMAT,
            request: request.clone(),
            issued,
            response: ReplayResponse {
                response_id: response.response_id.clone(),
                items,
                usage: response.usage.clone(),
            },
        };
        let payload = serde_json::to_string(&record)?;
        tx.execute(
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'model_turn',?2,?3)",
            params![request.0, payload, utc_millis()],
        )?;
        let sequence = tx.last_insert_rowid();
        tx.commit()?;
        Ok(sequence)
    }

    pub(super) fn decode_replay_record(
        &self,
        event: i64,
        payload: &str,
    ) -> Result<RecordedReplayTurn> {
        let value: serde_json::Value = serde_json::from_str(payload)?;
        if value.get("format").and_then(serde_json::Value::as_u64) != Some(u64::from(FORMAT)) {
            return Err(StoreError::UnsupportedReplayFormat { event });
        }
        let record: ReplayRecord = serde_json::from_value(value)?;
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let instructions = read_item(&tx, &record.issued.instructions)?;
        let tools = read_item(&tx, &record.issued.tools)?;
        let request = ResponsesRequest {
            input: record
                .issued
                .input
                .iter()
                .map(|hash| read_item(&tx, hash))
                .collect::<Result<_>>()?,
            instructions: serde_json::from_value(instructions.0)?,
            tools: serde_json::from_value(tools.0)?,
            tools_allowed: record.issued.tools_allowed,
            model: record.issued.model,
            pinned_effort: record.issued.pinned_effort,
            session_id: record.issued.session_id,
        };
        let response = ResponsesTurn {
            response_id: record.response.response_id,
            items: record
                .response
                .items
                .iter()
                .map(|hash| read_item(&tx, hash))
                .collect::<Result<_>>()?,
            usage: record.response.usage,
        };
        tx.commit()?;
        Ok(RecordedReplayTurn {
            request: record.request,
            model_request: request,
            model_response: response,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Effort, transport::client::request_body};
    use serde_json::json;

    fn request(input: Vec<Item>) -> ResponsesRequest {
        ResponsesRequest {
            input,
            instructions: "exact immutable instructions".into(),
            tools: vec![json!({"type":"function","name":"a","strict":true,"future":{"opaque":7}})]
                .into(),
            tools_allowed: Some(vec!["a".into()]),
            model: "offline".into(),
            pinned_effort: Effort::Medium,
            session_id: "session".into(),
        }
    }

    fn response(items: Vec<Item>) -> ResponsesTurn {
        ResponsesTurn {
            response_id: "response".into(),
            items,
            usage: Usage::default(),
        }
    }

    #[test]
    fn exact_issued_items_survive_late_output_and_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-issued-replay-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let root = RequestId("root".into());
        let original = Item(json!({"type":"message","role":"user","content":"original"}));
        let projected = Item(
            json!({"type":"function_call_output","call_id":"same-id","output":"projected","unknown":{"keep":[1,2]}}),
        );
        let injected =
            Item(json!({"type":"message","role":"user","content":"ephemeral hook input"}));
        let input = request(vec![original.clone(), projected, injected]);
        let done = response(vec![Item(
            json!({"type":"custom_tool_call","name":"raw","call_id":"same-id","input":"☃\n\\\"","future_field":42}),
        )]);
        {
            let store = Store::open(&path).unwrap();
            store.create_request(&root, None, "/root").unwrap();
            store.append_items(&root, &[original]).unwrap();
            let sealed = store.seal_replay_request(&input).unwrap();
            store.append_items(&root,&[Item(json!({"type":"custom_tool_call_output","call_id":"same-id","output":"late actual output"}))]).unwrap();
            store
                .record_issued_replay_turn(&root, sealed, &done)
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let replay = store.replay_turns(&root).unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(
            request_body(&replay[0].model_request).unwrap(),
            request_body(&input).unwrap()
        );
        assert_eq!(replay[0].model_response.items, done.items);
        assert_eq!(store.items(&root).unwrap().len(), 2);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_replay_format_is_refused_without_changing_event_bytes() {
        let store = Store::memory().unwrap();
        let root = RequestId("root".into());
        store.create_request(&root, None, "/root").unwrap();
        let old = RecordedReplayTurn {
            request: root.clone(),
            model_request: request(vec![]),
            model_response: response(vec![]),
        };
        let sequence = store
            .record_event(
                Some(&root),
                "model_turn",
                &serde_json::to_value(old).unwrap(),
            )
            .unwrap();
        let before = store.events(Some(&root)).unwrap()[0].payload.clone();
        assert!(
            matches!(store.replay_turns(&root),Err(StoreError::UnsupportedReplayFormat { event }) if event==sequence)
        );
        assert_eq!(store.events(Some(&root)).unwrap()[0].payload, before);
    }

    #[test]
    fn replay_completion_failure_rolls_back_response_references() {
        let store = Store::memory().unwrap();
        let root = RequestId("root".into());
        store.create_request(&root, None, "/root").unwrap();
        let sealed = store.seal_replay_request(&request(vec![])).unwrap();
        store.lock().execute_batch("CREATE TRIGGER reject_replay BEFORE INSERT ON events WHEN NEW.kind='model_turn' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        let output = Item(json!({"type":"message","role":"assistant","content":"new output"}));
        let hash = ItemHash(
            blake3::hash(&serde_json::to_vec(&output).unwrap())
                .to_hex()
                .to_string(),
        );
        assert!(
            store
                .record_issued_replay_turn(&root, sealed, &response(vec![output]))
                .is_err()
        );
        assert!(store.get_item(&hash).unwrap().is_none());
        assert!(store.events(Some(&root)).unwrap().is_empty());
    }

    #[test]
    fn replay_reference_bytes_at_two_history_sizes() {
        for count in [100, 1000] {
            let store = Store::memory().unwrap();
            let root = RequestId("root".into());
            store.create_request(&root, None, "/root").unwrap();
            let items = (0..count).map(|index| Item(json!({"type":"message","role":"user","content":format!("{index}:{}","x".repeat(4096))}))).collect::<Vec<_>>();
            store.append_items(&root, &items).unwrap();
            let hashes = store
                .items_with_hashes(&root)
                .unwrap()
                .into_iter()
                .map(|(hash, _)| Some(hash))
                .collect::<Vec<_>>();
            let issued = request(items);
            let instructions = store
                .intern_replay_instructions(&issued.instructions)
                .unwrap();
            let mut input_reencodings = 0;
            for _ in 0..4 {
                let sealed = store
                    .seal_replay_request_with_hashes(&issued, &hashes, Some(&instructions))
                    .unwrap();
                input_reencodings += sealed.input_reencodings;
                store
                    .record_issued_replay_turn(&root, sealed, &response(vec![]))
                    .unwrap();
            }
            let history_bytes = serde_json::to_vec(&issued.input).unwrap().len();
            let previous_event_bytes = serde_json::to_vec(&RecordedReplayTurn {
                request: root.clone(),
                model_request: issued.clone(),
                model_response: response(vec![]),
            })
            .unwrap()
            .len()
                * 4;
            let events = store.events(Some(&root)).unwrap();
            let replay_bytes = events
                .iter()
                .map(|event| event.payload.len())
                .sum::<usize>();
            assert!(replay_bytes < history_bytes / 8);
            assert_eq!(store.replay_turns(&root).unwrap().len(), 4);
            let stored_items: usize = store
                .lock()
                .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stored_items, count + 2);
            assert_eq!(input_reencodings, 0);
            eprintln!(
                "harness-request-cost {}",
                json!({"history_items":count,"history_bytes":history_bytes,"rounds":4,"previous_full_payload_bytes":previous_event_bytes,"replay_event_bytes":replay_bytes,"stored_items":stored_items,"immutable_schema_bytes":issued.tools.encoded().1.len(),"unchanged_history_reencodings":input_reencodings})
            );
        }
    }
}
