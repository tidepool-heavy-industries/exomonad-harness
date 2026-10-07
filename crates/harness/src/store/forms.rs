//! Durable presentation of scope-owned human form leases. Decoders remain native.
use super::{
    Result, Store, StoreError,
    actor_output::{ActorOutputExecution, ActorOutputOrigin, validate_origin},
    presentation::*,
};
use crate::model::ConversationIdentity;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorFormOpen {
    pub origin: ActorOutputOrigin,
    pub mount_id: String,
    pub execution: ActorOutputExecution,
    pub conversation: Option<ConversationIdentity>,
    pub form: FormSpec,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActorFormAttempt {
    Submitted { attempt_id: String, draft: Value },
    Dismissed,
    Unavailable { cause: ActorFormUnavailableCause },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorFormUnavailableCause {
    Interrupted,
    Cancelled,
    Closed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorFormState {
    Open,
    Submitted,
    Answered,
    Dismissed,
    Cancelled,
    Interrupted,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredActorForm {
    pub sequence: i64,
    pub revision_sequence: i64,
    pub opening: ActorFormOpen,
    pub state: ActorFormState,
    pub attempt_id: Option<String>,
    pub draft: Option<FormDraft>,
    pub errors: Vec<FormError>,
    pub answer: Option<View>,
}
impl StoredActorForm {
    fn validate(&self) -> Result<()> {
        let submission = self.attempt_id.is_some() && self.draft.is_some();
        let no_submission = self.attempt_id.is_none() && self.draft.is_none();
        let valid = match self.state {
            ActorFormState::Open => self.answer.is_none() && (submission || no_submission),
            ActorFormState::Submitted => {
                submission && self.answer.is_none() && self.errors.is_empty()
            }
            ActorFormState::Answered => {
                submission && self.answer.is_some() && self.errors.is_empty()
            }
            ActorFormState::Dismissed | ActorFormState::Cancelled | ActorFormState::Interrupted => {
                self.answer.is_none() && (submission || no_submission)
            }
        };
        if !valid
            || self.sequence <= 0
            || self.revision_sequence < self.sequence
            || serde_json::to_vec(self)?.len() > super::history::MAX_HISTORY_BYTES - 8192
        {
            return Err(StoreError::InvalidForm);
        }
        self.opening.form.validate()?;
        if let Some(draft) = &self.draft {
            self.opening.form.validate_draft(draft)?;
        }
        if let Some(answer) = &self.answer {
            answer.validate()?;
        }
        Ok(())
    }
}
enum SettleForm<'a> {
    Reject {
        attempt: &'a str,
        errors: &'a Vec<FormError>,
    },
    Commit {
        attempt: &'a str,
        answer: &'a View,
    },
    Close,
}
/// Validated under the Store transaction; this hook must not reenter Store.
pub trait ActorFormAuthority: Send + Sync {
    fn validate_form(&self, opening: &ActorFormOpen) -> std::result::Result<bool, String>;
}
fn key(origin: &ActorOutputOrigin, mount: &str) -> Result<String> {
    validate_origin(origin)?;
    if mount.is_empty() || mount.len() > 256 {
        return Err(StoreError::InvalidForm);
    }
    Ok(serde_json::to_string(&(origin, mount))?)
}
fn load(tx: &Transaction<'_>, identity: &str) -> Result<StoredActorForm> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT presentation FROM actor_forms WHERE identity=?1",
            [identity],
            |r| r.get(0),
        )
        .optional()?;
    let row: StoredActorForm = serde_json::from_str(&raw.ok_or(StoreError::FormUnavailable)?)?;
    row.validate()?;
    if key(&row.opening.origin, &row.opening.mount_id)? != identity {
        return Err(StoreError::InvalidForm);
    }
    Ok(row)
}
fn save(tx: &Transaction<'_>, key: &str, row: &mut StoredActorForm) -> Result<()> {
    row.validate()?;
    tx.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES(NULL,'actor_form_update',?1,?2)",params![serde_json::to_string(&json!({"origin":row.opening.origin,"mountId":row.opening.mount_id,"openingSequence":row.sequence}))?,super::utc_millis()])?;
    row.revision_sequence = tx.last_insert_rowid();
    tx.execute(
        "UPDATE actor_forms SET presentation=?2 WHERE identity=?1",
        params![key, serde_json::to_string(row)?],
    )?;
    Ok(())
}
impl Store {
    /// Exclusive runtime startup calls this once before admitting continuation scopes.
    /// Opening a connection or serving retained history never performs recovery.
    pub fn interrupt_actor_forms_for_restart(&self) -> Result<usize> {
        let count = interrupt_pending(&mut self.lock())?;
        if count > 0 {
            self.signal_actor_form_change();
        }
        Ok(count)
    }
    /// Subscribe before reading a canonical attempt. Versions are readiness hints,
    /// never publication order or durable form state.
    pub fn subscribe_actor_form_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.actor_form_changes.subscribe()
    }
    fn signal_actor_form_change(&self) {
        self.actor_form_changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
    pub fn open_actor_form(
        &self,
        authority: &dyn ActorFormAuthority,
        opening: &ActorFormOpen,
    ) -> Result<StoredActorForm> {
        let identity = key(&opening.origin, &opening.mount_id)?;
        opening.form.validate()?;
        let mut retained = opening.clone();
        retained.form = self.retain_form_media(&opening.form)?;
        let opening = &retained;
        if let Some(ConversationIdentity::Embedded {
            run, incarnation, ..
        }) = &opening.conversation
        {
            if run != &opening.origin.run || incarnation != &opening.origin.incarnation.to_string()
            {
                return Err(StoreError::InvalidForm);
            }
        }
        if let Some(ConversationIdentity::Standalone { store, .. }) = &opening.conversation {
            if store != &self.store_id {
                return Err(StoreError::InvalidForm);
            }
        }
        let mut c = self.lock();
        let tx = c.transaction()?;
        if !authority
            .validate_form(opening)
            .map_err(StoreError::ActorOutputAuthority)?
        {
            return Err(StoreError::ActorOutputRefused);
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM actor_forms WHERE identity=?1)",
            [&identity],
            |r| r.get(0),
        )?;
        if exists {
            let row = load(&tx, &identity)?;
            if row.opening != *opening {
                return Err(StoreError::ConflictingFormOperation);
            }
            return Ok(row);
        }
        tx.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES(NULL,'actor_form_open',?1,?2)",params![serde_json::to_string(opening)?,super::utc_millis()])?;
        let row = StoredActorForm {
            sequence: tx.last_insert_rowid(),
            revision_sequence: tx.last_insert_rowid(),
            opening: opening.clone(),
            state: ActorFormState::Open,
            attempt_id: None,
            draft: None,
            errors: vec![],
            answer: None,
        };
        row.validate()?;
        tx.execute(
            "INSERT INTO actor_forms(identity,opening_sequence,presentation) VALUES(?1,?2,?3)",
            params![identity, row.sequence, serde_json::to_string(&row)?],
        )?;
        tx.commit()?;
        self.signal_actor_form_change();
        Ok(row)
    }
    pub fn submit_actor_form(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        operation: &str,
        draft: &Value,
    ) -> Result<StoredActorForm> {
        let draft: FormDraft =
            serde_json::from_value(draft.clone()).map_err(|_| StoreError::InvalidForm)?;
        self.form_browser_operation(origin, mount, operation, Some(&draft))
    }
    pub fn dismiss_actor_form(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        operation: &str,
    ) -> Result<StoredActorForm> {
        self.form_browser_operation(origin, mount, operation, None)
    }
    fn form_browser_operation(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        operation: &str,
        draft: Option<&FormDraft>,
    ) -> Result<StoredActorForm> {
        let identity = key(origin, mount)?;
        if operation.is_empty() || operation.len() > 256 {
            return Err(StoreError::InvalidForm);
        }
        let payload = serde_json::to_string(&draft)?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        let mut row = load(&tx, &identity)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT payload FROM actor_form_operations WHERE identity=?1 AND operation_id=?2",
                params![identity, operation],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if old != payload {
                return Err(StoreError::ConflictingFormOperation);
            }
            return Ok(row);
        }
        if row.state != ActorFormState::Open
            && !(draft.is_none() && row.state == ActorFormState::Submitted)
        {
            return Err(StoreError::FormUnavailable);
        }
        if let Some(draft) = draft {
            row.opening.form.validate_draft(draft)?;
        }
        if let Some(draft) = draft {
            row.attempt_id = Some(operation.to_owned());
            row.draft = Some(draft.clone());
        }
        row.errors = vec![];
        row.state = if draft.is_some() {
            ActorFormState::Submitted
        } else {
            ActorFormState::Dismissed
        };
        tx.execute(
            "INSERT INTO actor_form_operations(identity,operation_id,payload) VALUES(?1,?2,?3)",
            params![identity, operation, payload],
        )?;
        save(&tx, &identity, &mut row)?;
        tx.commit()?;
        self.signal_actor_form_change();
        Ok(row)
    }
    pub fn actor_form_attempt(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
    ) -> Result<Option<ActorFormAttempt>> {
        let identity = key(origin, mount)?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        let row = load(&tx, &identity)?;
        Ok(match row.state {
            ActorFormState::Open => None,
            ActorFormState::Submitted => Some(ActorFormAttempt::Submitted {
                attempt_id: row.attempt_id.ok_or(StoreError::InvalidForm)?,
                draft: serde_json::to_value(row.draft.ok_or(StoreError::InvalidForm)?)?,
            }),
            ActorFormState::Dismissed => Some(ActorFormAttempt::Dismissed),
            _ => Some(ActorFormAttempt::Unavailable {
                cause: match row.state {
                    ActorFormState::Interrupted => ActorFormUnavailableCause::Interrupted,
                    ActorFormState::Cancelled => ActorFormUnavailableCause::Cancelled,
                    _ => ActorFormUnavailableCause::Closed,
                },
            }),
        })
    }
    pub fn reject_actor_form(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        attempt: &str,
        errors: &Value,
    ) -> Result<bool> {
        let errors = parse_errors(errors)?;
        self.settle_actor_form(
            origin,
            mount,
            SettleForm::Reject {
                attempt,
                errors: &errors,
            },
        )
    }
    pub fn commit_actor_form(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        attempt: &str,
        presentation: &Value,
    ) -> Result<bool> {
        let presentation: View =
            serde_json::from_value(presentation.clone()).map_err(|_| StoreError::InvalidForm)?;
        presentation.validate()?;
        let retained = self.retain_view_media(&presentation)?;
        self.settle_actor_form(
            origin,
            mount,
            SettleForm::Commit {
                attempt,
                answer: &retained,
            },
        )
    }
    pub fn close_actor_form(&self, origin: &ActorOutputOrigin, mount: &str) -> Result<bool> {
        self.settle_actor_form(origin, mount, SettleForm::Close)
    }
    fn settle_actor_form(
        &self,
        origin: &ActorOutputOrigin,
        mount: &str,
        settlement: SettleForm<'_>,
    ) -> Result<bool> {
        let identity = key(origin, mount)?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        let mut row = load(&tx, &identity)?;
        match settlement {
            SettleForm::Commit { attempt, answer } => {
                if row.attempt_id.as_deref() != Some(attempt) {
                    return Ok(false);
                }
                if row.state == ActorFormState::Answered && row.answer.as_ref() == Some(answer) {
                    return Ok(true);
                }
                if row.state != ActorFormState::Submitted {
                    return Ok(false);
                }
                row.state = ActorFormState::Answered;
                row.answer = Some(answer.clone());
                row.errors = vec![];
            }
            SettleForm::Reject { attempt, errors } => {
                if row.attempt_id.as_deref() != Some(attempt) {
                    return Ok(false);
                }
                if row.state == ActorFormState::Open && &row.errors == errors {
                    return Ok(true);
                }
                if row.state != ActorFormState::Submitted {
                    return Ok(false);
                }
                row.state = ActorFormState::Open;
                row.errors = errors.clone();
            }
            SettleForm::Close => {
                if !matches!(row.state, ActorFormState::Open | ActorFormState::Submitted) {
                    return Ok(false);
                }
                row.state = ActorFormState::Cancelled;
            }
        }
        save(&tx, &identity, &mut row)?;
        tx.commit()?;
        self.signal_actor_form_change();
        Ok(true)
    }
    fn retain_form_media(&self, form: &FormSpec) -> Result<FormSpec> {
        fn node(store: &Store, n: &mut FormNode) -> Result<()> {
            match n {
                FormNode::View { presentation } => {
                    *presentation = store.retain_view_media(presentation)?
                }
                FormNode::Group { children } => {
                    for child in children {
                        node(store, child)?;
                    }
                }
                FormNode::Section { child, .. } => node(store, child)?,
                FormNode::Choice { options, .. } | FormNode::Many { options, .. } => {
                    for o in options {
                        o.presentation = store.retain_view_media(&o.presentation)?;
                    }
                }
                FormNode::Alternatives { options, .. } => {
                    for o in options {
                        o.presentation = store.retain_view_media(&o.presentation)?;
                        node(store, &mut o.form)?;
                    }
                }
                _ => (),
            };
            Ok(())
        }
        let mut retained = form.clone();
        node(self, &mut retained.root)?;
        Ok(retained)
    }
    pub fn actor_form(&self, origin: &ActorOutputOrigin, mount: &str) -> Result<StoredActorForm> {
        let identity = key(origin, mount)?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        load(&tx, &identity)
    }
}
fn interrupt_pending(c: &mut rusqlite::Connection) -> Result<usize> {
    let tx = c.transaction()?;
    let rows = {
        let mut q = tx.prepare("SELECT identity,presentation FROM actor_forms WHERE json_extract(presentation,'$.state') IN ('open','submitted')")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let count = rows.len();
    for (identity, raw) in rows {
        let mut row: StoredActorForm = serde_json::from_str(&raw)?;
        if matches!(row.state, ActorFormState::Open | ActorFormState::Submitted) {
            row.state = ActorFormState::Interrupted;
            save(&tx, &identity, &mut row)?;
        }
    }
    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Authority(bool);
    impl ActorFormAuthority for Authority {
        fn validate_form(&self, _: &ActorFormOpen) -> std::result::Result<bool, String> {
            Ok(self.0)
        }
    }
    fn opening(mount: &str) -> ActorFormOpen {
        ActorFormOpen {
            origin: ActorOutputOrigin {
                run: "run".into(),
                native_actor: 7,
                incarnation: 1,
            },
            mount_id: mount.into(),
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            form: serde_json::from_value(
                json!({"version":1,"root":{"kind":"text","id":"f0","label":"Name","initial":null}}),
            )
            .unwrap(),
        }
    }
    #[test]
    fn attempts_retain_drafts_fence_stale_settlement_and_do_not_move_cards() {
        let store = Store::memory().unwrap();
        let a = opening("a");
        let b = opening("b");
        let first = store.open_actor_form(&Authority(true), &a).unwrap();
        let second = store.open_actor_form(&Authority(true), &b).unwrap();
        assert!(first.sequence < second.sequence);
        let draft = json!({"f0":"invalid"});
        store
            .submit_actor_form(&a.origin, "a", "one", &draft)
            .unwrap();
        let errors = json!([{"field":"f0","message":"Please correct"}]);
        assert!(
            store
                .reject_actor_form(&a.origin, "a", "one", &errors)
                .unwrap()
        );
        assert!(
            store
                .reject_actor_form(&a.origin, "a", "one", &errors)
                .unwrap()
        );
        let rejected = store.actor_form(&a.origin, "a").unwrap();
        assert_eq!(rejected.sequence, first.sequence);
        assert_eq!(serde_json::to_value(rejected.draft).unwrap(), draft);
        // Exact transport retries acknowledge the original operation without resubmitting.
        assert_eq!(
            store
                .submit_actor_form(&a.origin, "a", "one", &draft)
                .unwrap()
                .state,
            ActorFormState::Open
        );
        assert!(matches!(
            store.submit_actor_form(&a.origin, "a", "one", &json!({"f0":"different"})),
            Err(StoreError::ConflictingFormOperation)
        ));
        store
            .submit_actor_form(&a.origin, "a", "two", &json!({"f0":"correct"}))
            .unwrap();
        assert!(
            !store
                .commit_actor_form(&a.origin, "a", "one", &json!({"kind":"text","text":"old"}))
                .unwrap()
        );
        store
            .submit_actor_form(&b.origin, "b", "other", &json!({"f0":"second"}))
            .unwrap();
        assert!(
            store
                .commit_actor_form(
                    &b.origin,
                    "b",
                    "other",
                    &json!({"kind":"text","text":"second answer"})
                )
                .unwrap()
        );
        assert!(
            store
                .commit_actor_form(
                    &a.origin,
                    "a",
                    "two",
                    &json!({"kind":"text","text":"first answer"})
                )
                .unwrap()
        );
        assert!(
            store
                .commit_actor_form(
                    &a.origin,
                    "a",
                    "two",
                    &json!({"kind":"text","text":"first answer"})
                )
                .unwrap()
        );
        assert!(!store.close_actor_form(&a.origin, "a").unwrap());
        assert_eq!(
            store.actor_form(&a.origin, "a").unwrap().state,
            ActorFormState::Answered
        );
        let page = store.chat_page(&a.origin, None, 0, 100).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert!(
            matches!(&page.entries[0],super::super::chat::ChatEntry::Form{form,..}if form.sequence==first.sequence&&form.state==ActorFormState::Answered)
        );
    }
    #[test]
    fn cancellation_dismissal_and_transaction_failure_fence_validation() {
        let store = Store::memory().unwrap();
        let a = opening("a");
        assert!(store.open_actor_form(&Authority(false), &a).is_err());
        assert!(store.events(None).unwrap().is_empty());
        store.open_actor_form(&Authority(true), &a).unwrap();
        store
            .submit_actor_form(&a.origin, "a", "one", &json!({"f0":"draft"}))
            .unwrap();
        store.lock().execute_batch("CREATE TRIGGER refuse_form BEFORE INSERT ON events WHEN NEW.kind='actor_form_update' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        assert!(
            store
                .commit_actor_form(
                    &a.origin,
                    "a",
                    "one",
                    &json!({"kind":"text","text":"answer"})
                )
                .is_err()
        );
        assert_eq!(
            store.actor_form(&a.origin, "a").unwrap().state,
            ActorFormState::Submitted
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_form")
            .unwrap();
        store.close_actor_form(&a.origin, "a").unwrap();
        assert!(
            !store
                .reject_actor_form(&a.origin, "a", "one", &json!([]))
                .unwrap()
        );
        assert!(
            !store
                .commit_actor_form(
                    &a.origin,
                    "a",
                    "one",
                    &json!({"kind":"text","text":"answer"})
                )
                .unwrap()
        );
        let b = opening("b");
        store.open_actor_form(&Authority(true), &b).unwrap();
        store
            .submit_actor_form(
                &b.origin,
                "b",
                "pending",
                &json!({"f0":"retained when dismissed"}),
            )
            .unwrap();
        store.dismiss_actor_form(&b.origin, "b", "dismiss").unwrap();
        assert_eq!(
            store.actor_form_attempt(&b.origin, "b").unwrap(),
            Some(ActorFormAttempt::Dismissed)
        );
        assert!(store.actor_form(&b.origin, "b").unwrap().draft.is_some());
        assert!(
            !store
                .commit_actor_form(
                    &b.origin,
                    "b",
                    "pending",
                    &json!({"kind":"text","text":"stale"})
                )
                .unwrap()
        );
        assert!(
            store
                .submit_actor_form(&b.origin, "b", "late", &json!({}))
                .is_err()
        );
    }
    #[test]
    fn reopening_interrupts_pending_but_preserves_submitted_and_final_history() {
        let path =
            std::env::temp_dir().join(format!("harness-form-{}.sqlite", uuid::Uuid::new_v4()));
        let pending = opening("pending");
        let done = opening("done");
        {
            let store = Store::open(&path).unwrap();
            for o in [&pending, &done] {
                store.open_actor_form(&Authority(true), o).unwrap();
                store
                    .submit_actor_form(&o.origin, &o.mount_id, "submit", &json!({"f0":"answer"}))
                    .unwrap();
            }
            store
                .commit_actor_form(
                    &done.origin,
                    "done",
                    "submit",
                    &json!({"kind":"markdown","text":"**accepted**"}),
                )
                .unwrap();
            let reader = Store::open(&path).unwrap();
            assert_eq!(
                reader.actor_form(&pending.origin, "pending").unwrap().state,
                ActorFormState::Submitted
            );
            assert_eq!(
                store.actor_form(&pending.origin, "pending").unwrap().state,
                ActorFormState::Submitted
            );
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.interrupt_actor_forms_for_restart().unwrap(), 1);
            assert_eq!(store.interrupt_actor_forms_for_restart().unwrap(), 0);
            assert_eq!(
                store.actor_form(&pending.origin, "pending").unwrap().state,
                ActorFormState::Interrupted
            );
            assert!(matches!(
                store
                    .actor_form_attempt(&pending.origin, "pending")
                    .unwrap(),
                Some(ActorFormAttempt::Unavailable {
                    cause: ActorFormUnavailableCause::Interrupted
                })
            ));
            let row = store.actor_form(&done.origin, "done").unwrap();
            assert_eq!(row.state, ActorFormState::Answered);
            assert!(row.answer.is_some());
            assert!(row.draft.is_some());
            assert!(!store.close_actor_form(&done.origin, "done").unwrap());
            assert!(
                store
                    .submit_actor_form(&pending.origin, "pending", "late", &json!({}))
                    .is_err()
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
#[cfg(test)]
mod notification_tests {
    use super::*;
    struct Authority;
    impl ActorFormAuthority for Authority {
        fn validate_form(&self, _: &ActorFormOpen) -> std::result::Result<bool, String> {
            Ok(true)
        }
    }
    fn opening() -> ActorFormOpen {
        ActorFormOpen{origin:ActorOutputOrigin{run:"run".into(),native_actor:7,incarnation:1},mount_id:"mount".into(),execution:ActorOutputExecution::ActorProgram,conversation:None,form:serde_json::from_value(json!({"version":1,"root":{"kind":"text","id":"f0","label":"Field","initial":null}})).unwrap()}
    }
    #[tokio::test]
    async fn durable_mutations_wake_subscribed_waiters_and_failed_transactions_do_not() {
        let store = Store::memory().unwrap();
        let o = opening();
        store.open_actor_form(&Authority, &o).unwrap();
        let mut wake = store.subscribe_actor_form_changes();
        assert!(
            store
                .actor_form_attempt(&o.origin, &o.mount_id)
                .unwrap()
                .is_none()
        );
        store
            .submit_actor_form(&o.origin, &o.mount_id, "one", &json!({"f0":"draft"}))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), wake.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            store.actor_form_attempt(&o.origin, &o.mount_id).unwrap(),
            Some(ActorFormAttempt::Submitted { .. })
        ));
        store
            .reject_actor_form(
                &o.origin,
                &o.mount_id,
                "one",
                &json!([{"field":"f0","message":"correct"}]),
            )
            .unwrap();
        wake.changed().await.unwrap();
        assert!(
            store
                .actor_form_attempt(&o.origin, &o.mount_id)
                .unwrap()
                .is_none()
        );
        store.lock().execute_batch("CREATE TRIGGER refuse_form BEFORE INSERT ON events WHEN NEW.kind='actor_form_update' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        assert!(
            store
                .submit_actor_form(&o.origin, &o.mount_id, "two", &json!({"f0":"next"}))
                .is_err()
        );
        assert!(!wake.has_changed().unwrap());
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_form")
            .unwrap();
        store
            .submit_actor_form(&o.origin, &o.mount_id, "two", &json!({"f0":"correct"}))
            .unwrap();
        wake.changed().await.unwrap();
        store
            .dismiss_actor_form(&o.origin, &o.mount_id, "dismiss")
            .unwrap();
        wake.changed().await.unwrap();
        assert_eq!(
            store.actor_form_attempt(&o.origin, &o.mount_id).unwrap(),
            Some(ActorFormAttempt::Dismissed)
        );
        let mut other = o.clone();
        other.mount_id = "other".into();
        store.open_actor_form(&Authority, &other).unwrap();
        wake.changed().await.unwrap();
        store
            .submit_actor_form(
                &other.origin,
                &other.mount_id,
                "submit",
                &json!({"f0":"answer"}),
            )
            .unwrap();
        wake.changed().await.unwrap();
        store
            .commit_actor_form(
                &other.origin,
                &other.mount_id,
                "submit",
                &json!({"kind":"text","text":"accepted"}),
            )
            .unwrap();
        wake.changed().await.unwrap();
        assert!(
            !store
                .close_actor_form(&other.origin, &other.mount_id)
                .unwrap()
        );
        assert!(!wake.has_changed().unwrap());
        let mut cancelled = o.clone();
        cancelled.mount_id = "cancelled".into();
        store.open_actor_form(&Authority, &cancelled).unwrap();
        wake.changed().await.unwrap();
        store
            .close_actor_form(&cancelled.origin, &cancelled.mount_id)
            .unwrap();
        wake.changed().await.unwrap();
        let mut interrupted = o.clone();
        interrupted.mount_id = "interrupted".into();
        store.open_actor_form(&Authority, &interrupted).unwrap();
        wake.changed().await.unwrap();
        store.interrupt_actor_forms_for_restart().unwrap();
        wake.changed().await.unwrap();
        assert!(matches!(
            store
                .actor_form_attempt(&interrupted.origin, &interrupted.mount_id)
                .unwrap(),
            Some(ActorFormAttempt::Unavailable {
                cause: ActorFormUnavailableCause::Interrupted
            })
        ));
    }
    #[test]
    fn decoded_state_invariants_and_open_save_bounds_are_owned_centrally() {
        let store = Store::memory().unwrap();
        let o = opening();
        store.open_actor_form(&Authority, &o).unwrap();
        store
            .submit_actor_form(&o.origin, &o.mount_id, "attempt", &json!({"f0":"draft"}))
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE actor_forms SET presentation=json_set(presentation,'$.state','answered')",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.actor_form(&o.origin, &o.mount_id),
            Err(StoreError::InvalidForm)
        ));
        let mut large = opening();
        large.mount_id = "too-large".into();
        large.execution = ActorOutputExecution::Notebook {
            execution: "x".repeat(super::super::history::MAX_HISTORY_BYTES),
            input_unit_index: 0,
            effect_ordinal: 0,
        };
        assert!(matches!(
            store.open_actor_form(&Authority, &large),
            Err(StoreError::InvalidForm)
        ));
        assert!(matches!(
            store.actor_form(&large.origin, &large.mount_id),
            Err(StoreError::FormUnavailable)
        ));
    }
}
