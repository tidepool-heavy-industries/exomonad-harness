//! Actor-authored output in the existing durable journal, independent of provider turns.
use super::{
    Result, Store, StoreError,
    history::{MAX_HISTORY_BYTES, MAX_HISTORY_ITEMS},
};
use crate::model::ConversationIdentity;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(super) const EVENT_KIND: &str = "actor_output";

pub const MAX_ACTOR_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_ACTOR_METADATA_BYTES: usize = 8 * 1024;
const MAX_EMISSION_BYTES: usize = MAX_HISTORY_BYTES - 32 * 1024;

pub(crate) const INDEXES: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS events_actor_output_identity ON events(
 json_extract(payload,'$.origin.run'),json_extract(payload,'$.origin.nativeActor'),
 json_extract(payload,'$.origin.incarnation'),json_extract(payload,'$.id.displaySlot'),
 json_extract(payload,'$.id.pageOrdinal')) WHERE kind='actor_output';
CREATE INDEX IF NOT EXISTS events_actor_output_history ON events(
 json_extract(payload,'$.origin.run'),json_extract(payload,'$.origin.nativeActor'),
 json_extract(payload,'$.origin.incarnation'),id) WHERE kind='actor_output';
";

/// A native principal remains stable before and after its optional conversation attaches.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorOutputOrigin {
    pub run: String,
    pub native_actor: u64,
    pub incarnation: u64,
}

/// The actor resource owner issues both counters. Selection keys may legally recur.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorOutputId {
    pub display_slot: u64,
    pub page_ordinal: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActorOutputExecution {
    Notebook {
        execution: String,
        input_unit_index: u64,
        effect_ordinal: u64,
    },
    ActorProgram,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorDisplayPage {
    pub text: String,
    pub expansions: Vec<(u64, String)>,
    pub unavailable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorOutputEmission {
    pub origin: ActorOutputOrigin,
    pub id: ActorOutputId,
    pub execution: ActorOutputExecution,
    pub conversation: Option<ConversationIdentity>,
    pub page: ActorDisplayPage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorOutputReference {
    pub origin: ActorOutputOrigin,
    pub sequence: i64,
}

/// Only Store can construct a committed row; projection never admits output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredActorOutput {
    reference: ActorOutputReference,
    emission: ActorOutputEmission,
    created_at: i64,
}
impl StoredActorOutput {
    pub fn reference(&self) -> &ActorOutputReference {
        &self.reference
    }
    pub fn emission(&self) -> &ActorOutputEmission {
        &self.emission
    }
}

pub enum ActorOutputCommit {
    Appended(StoredActorOutput),
    Existing(StoredActorOutput),
}
impl ActorOutputCommit {
    pub fn output(&self) -> &StoredActorOutput {
        match self {
            Self::Appended(row) | Self::Existing(row) => row,
        }
    }
}

/// The host closes over its live exact actor lease and existing conversation binding.
/// Called under the Store transaction; validation must not reenter Store.
pub trait ActorOutputAuthority: Send + Sync {
    fn validate_output(&self, emission: &ActorOutputEmission) -> std::result::Result<bool, String>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorOutputHistoryPage {
    pub origin: ActorOutputOrigin,
    pub outputs: Vec<StoredActorOutput>,
    pub next_after: Option<i64>,
}

fn validate_origin(origin: &ActorOutputOrigin) -> Result<()> {
    if origin.run.is_empty()
        || origin.run.len() > 1024
        || origin.native_actor > i64::MAX as u64
        || origin.incarnation > i64::MAX as u64
    {
        return Err(StoreError::InvalidActorOutput);
    }
    Ok(())
}

fn validate(emission: &ActorOutputEmission) -> Result<()> {
    validate_origin(&emission.origin)?;
    let page = &emission.page;
    let mut keys = std::collections::HashSet::new();
    let valid_execution = match &emission.execution {
        ActorOutputExecution::ActorProgram => true,
        ActorOutputExecution::Notebook { execution, .. } => {
            !execution.is_empty() && execution.len() <= 256
        }
    };
    let metadata = serde_json::json!({
        "identity": [emission.origin.native_actor, emission.origin.incarnation, emission.id.display_slot],
        "expansions": page.expansions, "unavailable": page.unavailable,
    });
    if emission.origin.run.is_empty()
        || emission.origin.run.len() > 1024
        || emission.id.display_slot == 0
        || !valid_execution
        || [
            emission.origin.native_actor,
            emission.origin.incarnation,
            emission.id.display_slot,
            emission.id.page_ordinal,
        ]
        .iter()
        .any(|value| *value > i64::MAX as u64)
        || page.text.len() > MAX_ACTOR_TEXT_BYTES
        || page
            .expansions
            .iter()
            .any(|(key, _)| *key == 0 || *key > i64::MAX as u64 || !keys.insert(*key))
        || serde_json::to_vec(&metadata)?.len() > MAX_ACTOR_METADATA_BYTES
        || serde_json::to_vec(emission)?.len() > MAX_EMISSION_BYTES
    {
        return Err(StoreError::InvalidActorOutput);
    }
    if let Some(ConversationIdentity::Embedded {
        run, incarnation, ..
    }) = &emission.conversation
    {
        if run != &emission.origin.run || incarnation != &emission.origin.incarnation.to_string() {
            return Err(StoreError::InvalidActorOutput);
        }
    }
    Ok(())
}

fn row(sequence: i64, payload: String, created_at: i64) -> Result<StoredActorOutput> {
    let emission: ActorOutputEmission = serde_json::from_str(&payload)?;
    validate(&emission)?;
    Ok(StoredActorOutput {
        reference: ActorOutputReference {
            origin: emission.origin.clone(),
            sequence,
        },
        emission,
        created_at,
    })
}

impl Store {
    /// Commit once to the existing events journal. Exact retries return the original row.
    pub fn append_actor_output(
        &self,
        authority: &dyn ActorOutputAuthority,
        emission: &ActorOutputEmission,
    ) -> Result<ActorOutputCommit> {
        validate(emission)?;
        if let Some(ConversationIdentity::Standalone { store, .. }) = &emission.conversation {
            if store != &self.store_id {
                return Err(StoreError::InvalidActorOutput);
            }
        }
        let payload = serde_json::to_string(emission)?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if !authority
            .validate_output(emission)
            .map_err(StoreError::ActorOutputAuthority)?
        {
            return Err(StoreError::ActorOutputRefused);
        }
        let existing: Option<(i64, String, i64)> = transaction
            .query_row(
                "SELECT id,payload,created_at FROM events WHERE kind='actor_output'
             AND json_extract(payload,'$.origin.run')=?1
             AND json_extract(payload,'$.origin.nativeActor')=?2
             AND json_extract(payload,'$.origin.incarnation')=?3
             AND json_extract(payload,'$.id.displaySlot')=?4
             AND json_extract(payload,'$.id.pageOrdinal')=?5",
                params![
                    emission.origin.run,
                    emission.origin.native_actor,
                    emission.origin.incarnation,
                    emission.id.display_slot,
                    emission.id.page_ordinal
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((sequence, retained, created_at)) = existing {
            let stored = row(sequence, retained, created_at)?;
            if stored.emission != *emission {
                return Err(StoreError::ConflictingActorOutput);
            }
            transaction.commit()?;
            return Ok(ActorOutputCommit::Existing(stored));
        }
        let created_at = super::utc_millis();
        transaction.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES (NULL,'actor_output',?1,?2)", params![payload, created_at])?;
        let sequence = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(ActorOutputCommit::Appended(StoredActorOutput {
            reference: ActorOutputReference {
                origin: emission.origin.clone(),
                sequence,
            },
            emission: emission.clone(),
            created_at,
        }))
    }

    /// Read exact actor history in journal order with the existing history response bounds.
    pub fn actor_output_page(
        &self,
        origin: &ActorOutputOrigin,
        after: i64,
        limit: usize,
    ) -> Result<ActorOutputHistoryPage> {
        validate_origin(origin)?;
        if after < 0 || limit == 0 || limit > MAX_HISTORY_ITEMS {
            return Err(StoreError::InvalidHistoryOffset);
        }
        let connection = self.lock();
        let mut query = connection.prepare(
            "SELECT id,payload,created_at FROM events WHERE kind='actor_output' AND id>?1
             AND json_extract(payload,'$.origin.run')=?2
             AND json_extract(payload,'$.origin.nativeActor')=?3
             AND json_extract(payload,'$.origin.incarnation')=?4 ORDER BY id LIMIT ?5",
        )?;
        let rows = query.query_map(
            params![
                after,
                origin.run,
                origin.native_actor,
                origin.incarnation,
                limit + 1
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )?;
        let mut page = ActorOutputHistoryPage {
            origin: origin.clone(),
            outputs: Vec::new(),
            next_after: None,
        };
        for candidate in rows {
            let (sequence, payload, created_at) = candidate?;
            let output = row(sequence, payload, created_at)?;
            if page.outputs.len() == limit {
                page.next_after = page.outputs.last().map(|output| output.reference.sequence);
                break;
            }
            page.outputs.push(output);
            page.next_after = page.outputs.last().map(|output| output.reference.sequence);
            if serde_json::to_vec(&page)?.len() > MAX_HISTORY_BYTES {
                page.outputs.pop();
                page.next_after = page.outputs.last().map(|output| output.reference.sequence);
                if page.outputs.is_empty() {
                    return Err(StoreError::InvalidActorOutput);
                }
                break;
            }
            page.next_after = None;
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("harness-actor-output-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Authority(bool);
    impl ActorOutputAuthority for Authority {
        fn validate_output(&self, _: &ActorOutputEmission) -> std::result::Result<bool, String> {
            Ok(self.0)
        }
    }
    fn emission(page_ordinal: u64) -> ActorOutputEmission {
        ActorOutputEmission {
            origin: ActorOutputOrigin {
                run: "run".into(),
                native_actor: 7,
                incarnation: 3,
            },
            id: ActorOutputId {
                display_slot: 1,
                page_ordinal,
            },
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            page: ActorDisplayPage {
                text: "hello".into(),
                expansions: vec![(1, "detail".into())],
                unavailable: false,
            },
        }
    }
    #[test]
    fn actor_output_survives_without_conversation_and_replays_once() {
        let directory = TestDirectory::new();
        let path = directory.path().join("store.sqlite");
        let first = emission(0);
        let reference = {
            let store = Store::open(&path).unwrap();
            let committed = store.append_actor_output(&Authority(true), &first).unwrap();
            assert!(matches!(committed, ActorOutputCommit::Appended(_)));
            let repeated = store.append_actor_output(&Authority(true), &first).unwrap();
            assert!(matches!(repeated, ActorOutputCommit::Existing(_)));
            assert_eq!(committed.output(), repeated.output());
            let expanded = emission(1); // same selected key, independently identified page
            store
                .append_actor_output(&Authority(true), &expanded)
                .unwrap();
            committed.output().reference().clone()
        };
        let recovered = Store::open(path).unwrap();
        let history = recovered.actor_output_page(&first.origin, 0, 100).unwrap();
        assert_eq!(history.outputs.len(), 2);
        assert_eq!(history.outputs[0].reference(), &reference);
        assert!(history.outputs[0].emission().conversation.is_none());
        assert!(
            recovered
                .events(None)
                .unwrap()
                .iter()
                .all(|event| event.request.is_none())
        );
        let mut altered = first.clone();
        altered.page.text = "conflicting".into();
        assert!(matches!(
            recovered.append_actor_output(&Authority(true), &altered),
            Err(StoreError::ConflictingActorOutput)
        ));
        assert!(matches!(
            recovered.append_actor_output(&Authority(false), &first),
            Err(StoreError::ActorOutputRefused)
        ));
        let mut foreign = first.origin.clone();
        foreign.incarnation += 1;
        assert!(
            recovered
                .actor_output_page(&foreign, 0, 100)
                .unwrap()
                .outputs
                .is_empty()
        );
    }
    #[test]
    fn actor_output_bounds_encoded_metadata_and_history_bytes() {
        let store = Store::memory().unwrap();
        assert!(matches!(
            store.record_event(
                None,
                EVENT_KIND,
                &serde_json::to_value(emission(0)).unwrap()
            ),
            Err(StoreError::ActorOutputNeedsAuthority)
        ));
        let mut invalid_origin = emission(0).origin;
        invalid_origin.run = "x".repeat(MAX_HISTORY_BYTES + 1);
        assert!(matches!(
            store.actor_output_page(&invalid_origin, 0, 100),
            Err(StoreError::InvalidActorOutput)
        ));
        let mut invalid = emission(0);
        invalid.page.expansions[0].1 = "\0".repeat(1400);
        assert!(matches!(
            store.append_actor_output(&Authority(true), &invalid),
            Err(StoreError::InvalidActorOutput)
        ));
        assert!(store.events(None).unwrap().is_empty());
        for ordinal in 0..8 {
            let mut large = emission(ordinal);
            large.page.text = "\0".repeat(MAX_ACTOR_TEXT_BYTES);
            store.append_actor_output(&Authority(true), &large).unwrap();
        }
        let mut after = 0;
        let mut count = 0;
        loop {
            let history = store
                .actor_output_page(&emission(0).origin, after, 100)
                .unwrap();
            assert!(serde_json::to_vec(&history).unwrap().len() <= MAX_HISTORY_BYTES);
            assert!(!history.outputs.is_empty());
            count += history.outputs.len();
            match history.next_after {
                Some(next) => {
                    assert!(next > after);
                    after = next;
                }
                None => break,
            }
        }
        assert_eq!(count, 8);
    }
    #[test]
    fn schema_eleven_actor_output_migration_preserves_existing_journal() {
        let directory = TestDirectory::new();
        let path = directory.path().join("store.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store
                .record_event(None, "retained", &serde_json::json!({"value":7}))
                .unwrap();
        }
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("UPDATE schema_version SET version=11; DROP INDEX events_actor_output_identity; DROP INDEX events_actor_output_history;").unwrap();
        }
        let migrated = Store::open(&path).unwrap();
        assert_eq!(migrated.events(None).unwrap()[0].kind, "retained");
        migrated
            .append_actor_output(&Authority(true), &emission(0))
            .unwrap();
        assert_eq!(migrated.events(None).unwrap().len(), 2);
        let version: u32 = migrated
            .lock()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::super::schema::VERSION);
    }
}
