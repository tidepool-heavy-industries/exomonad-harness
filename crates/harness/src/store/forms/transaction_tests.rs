//! Real SQLite completion faults at the public form boundary; no crash simulation.
use super::*;
use std::{path::PathBuf, sync::Arc};

struct Authority(bool);
impl ActorFormAuthority for Authority {
    fn validate_form(&self, _: &ActorFormOpen) -> std::result::Result<bool, String> {
        Ok(self.0)
    }
}
fn opening() -> ActorFormOpen {
    ActorFormOpen {
        origin: ActorOutputOrigin {
            run: "run".into(),
            native_actor: 7,
            incarnation: 1,
        },
        mount_id: "form".into(),
        execution: ActorOutputExecution::ActorProgram,
        conversation: None,
        form: serde_json::from_value(
            json!({"version":1,"root":{"kind":"text","id":"f0","label":"Name","initial":null}}),
        )
        .unwrap(),
    }
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("harness-form-completion-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn open(&self) -> Arc<Store> {
        Arc::new(Store::open(self.0.join("store.sqlite")).unwrap())
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[derive(Clone, Copy, Debug)]
enum Fault {
    Commit,
    Rollback,
}
fn deny_completion(store: &Store, fault: Fault) {
    unsafe extern "C" fn deny(
        selected: *mut std::ffi::c_void,
        action: std::ffi::c_int,
        operation: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
    ) -> std::ffi::c_int {
        if action == rusqlite::ffi::SQLITE_TRANSACTION && !operation.is_null() {
            let operation = unsafe { std::ffi::CStr::from_ptr(operation) }.to_bytes();
            let blocked = if selected.is_null() {
                b"ROLLBACK".as_slice()
            } else {
                b"COMMIT".as_slice()
            };
            if operation == blocked {
                return rusqlite::ffi::SQLITE_DENY;
            }
        }
        rusqlite::ffi::SQLITE_OK
    }
    let connection = store.lock().unwrap();
    // The pointer is a tag, never dereferenced; the callback has no borrowed state.
    let selected = match fault {
        Fault::Rollback => std::ptr::null_mut(),
        Fault::Commit => std::ptr::dangling_mut(),
    };
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_set_authorizer(connection.handle(), Some(deny), selected) },
        rusqlite::ffi::SQLITE_OK
    );
}
#[derive(Clone, Copy, Debug)]
enum Action {
    Open,
    RepeatOpen,
    Submit,
    RepeatSubmit,
    Reject,
    RepeatReject,
    Answer,
    RepeatAnswer,
    StaleAnswer,
    Close,
    RepeatClose,
    Dismiss,
    RepeatDismiss,
    Restart,
    RefuseAuthority,
    ConflictOpen,
    ConflictSubmit,
    Unavailable,
    InvalidDraft,
    StatementRefusal,
}
impl Action {
    fn prepare(self, store: &Store, o: &ActorFormOpen) {
        if matches!(self, Self::Open | Self::RefuseAuthority) {
            return;
        }
        store.open_actor_form(&Authority(true), o).unwrap();
        if matches!(
            self,
            Self::RepeatSubmit
                | Self::Reject
                | Self::RepeatReject
                | Self::Answer
                | Self::RepeatAnswer
                | Self::StaleAnswer
                | Self::ConflictSubmit
        ) {
            store
                .submit_actor_form(&o.origin, &o.mount_id, "one", &json!({"f0":"answer"}))
                .unwrap();
        }
        if matches!(self, Self::RepeatReject) {
            store
                .reject_actor_form(
                    &o.origin,
                    &o.mount_id,
                    "one",
                    &json!([{ "field":"f0", "message":"correct" }]),
                )
                .unwrap();
        }
        if matches!(self, Self::RepeatAnswer) {
            store
                .commit_actor_form(
                    &o.origin,
                    &o.mount_id,
                    "one",
                    &json!({"kind":"text","text":"accepted"}),
                )
                .unwrap();
        }
        if matches!(self, Self::RepeatClose | Self::Unavailable) {
            store.close_actor_form(&o.origin, &o.mount_id).unwrap();
        }
        if matches!(self, Self::RepeatDismiss) {
            store
                .dismiss_actor_form(&o.origin, &o.mount_id, "dismiss")
                .unwrap();
        }
        if matches!(self, Self::StatementRefusal) {
            store.lock().unwrap().execute_batch("CREATE TRIGGER refuse_form BEFORE INSERT ON events WHEN NEW.kind='actor_form_update' BEGIN SELECT RAISE(ABORT,'form update refused'); END;").unwrap();
        }
    }
    fn run(self, store: &Store, o: &ActorFormOpen) -> Result<()> {
        match self {
            Self::Open | Self::RepeatOpen => store.open_actor_form(&Authority(true), o).map(|_| ()),
            Self::RefuseAuthority => store.open_actor_form(&Authority(false), o).map(|_| ()),
            Self::ConflictOpen => {
                let mut other = o.clone();
                other.execution = ActorOutputExecution::Notebook {
                    execution: "different".into(),
                    input_unit_index: 0,
                    effect_ordinal: 0,
                };
                store.open_actor_form(&Authority(true), &other).map(|_| ())
            }
            Self::Submit | Self::RepeatSubmit | Self::StatementRefusal | Self::Unavailable => store
                .submit_actor_form(&o.origin, &o.mount_id, "one", &json!({"f0":"answer"}))
                .map(|_| ()),
            Self::ConflictSubmit => store
                .submit_actor_form(&o.origin, &o.mount_id, "one", &json!({"f0":"different"}))
                .map(|_| ()),
            Self::InvalidDraft => store
                .submit_actor_form(&o.origin, &o.mount_id, "bad", &json!({"foreign":"value"}))
                .map(|_| ()),
            Self::Reject | Self::RepeatReject => store
                .reject_actor_form(
                    &o.origin,
                    &o.mount_id,
                    "one",
                    &json!([{ "field":"f0", "message":"correct" }]),
                )
                .map(|_| ()),
            Self::Answer | Self::RepeatAnswer | Self::StaleAnswer => store
                .commit_actor_form(
                    &o.origin,
                    &o.mount_id,
                    if matches!(self, Self::StaleAnswer) {
                        "old"
                    } else {
                        "one"
                    },
                    &json!({"kind":"text","text":"accepted"}),
                )
                .map(|_| ()),
            Self::Close | Self::RepeatClose => {
                store.close_actor_form(&o.origin, &o.mount_id).map(|_| ())
            }
            Self::Dismiss | Self::RepeatDismiss => store
                .dismiss_actor_form(&o.origin, &o.mount_id, "dismiss")
                .map(|_| ()),
            Self::Restart => store.interrupt_actor_forms_for_restart().map(|_| ()),
        }
    }
}
fn snapshot(store: &Store) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let c = store.lock().unwrap();
    let query = |sql| {
        let mut stmt = c.prepare(sql).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    (
        query("SELECT json_array(id,kind,payload) FROM events ORDER BY id"),
        query(
            "SELECT json_array(identity,opening_sequence,presentation) FROM actor_forms ORDER BY identity",
        ),
        query(
            "SELECT json_array(identity,operation_id,payload) FROM actor_form_operations ORDER BY identity,operation_id",
        ),
        query("SELECT json_array(hash,json) FROM items ORDER BY hash"),
    )
}
fn assert_recovery(error: StoreError, action: impl std::fmt::Debug) {
    assert!(
        matches!(error, StoreError::RecoveryRequired(ref cause) if cause.sqlite_error_code() == Some(rusqlite::ErrorCode::AuthorizationForStatementDenied)),
        "{action:?}: {error:?}"
    );
}
#[test]
fn single_row_reads_do_not_require_transaction_completion() {
    let database = Database::new();
    let store = database.open();
    let o = opening();
    let expected = store.open_actor_form(&Authority(true), &o).unwrap();
    let before = snapshot(&store);
    deny_completion(&store, Fault::Rollback);
    assert_eq!(store.actor_form(&o.origin, &o.mount_id).unwrap(), expected);
    store
        .ensure_ready()
        .expect("single-row read must not open a transaction whose rollback can fence admission");
    assert_eq!(
        store.actor_form_attempt(&o.origin, &o.mount_id).unwrap(),
        None
    );
    assert_eq!(snapshot(&store), before);
}
#[test]
fn failed_form_rollback_retains_cause_and_acknowledged_prefix() {
    for action in [
        Action::RefuseAuthority,
        Action::ConflictOpen,
        Action::ConflictSubmit,
        Action::Unavailable,
        Action::InvalidDraft,
        Action::StatementRefusal,
    ] {
        let database = Database::new();
        let store = database.open();
        let sibling = store.clone();
        let o = opening();
        action.prepare(&store, &o);
        let before = snapshot(&store);
        let changes = store.subscribe_actor_form_changes();
        deny_completion(&store, Fault::Rollback);
        assert_recovery(
            action
                .run(&store, &o)
                .expect_err("rollback fault must override ordinary refusal"),
            action,
        );
        assert!(
            !changes.has_changed().unwrap(),
            "no false form notification for {action:?}"
        );
        assert!(matches!(
            sibling.actor_form(&o.origin, &o.mount_id),
            Err(StoreError::AdmissionFenced)
        ));
        drop(sibling);
        drop(store);
        let reopened = database.open();
        assert_eq!(snapshot(&reopened), before, "{action:?}");
    }
}
#[test]
fn failed_form_commit_covers_mutation_idempotency_stale_and_restart_histories() {
    for action in [
        Action::Open,
        Action::RepeatOpen,
        Action::Submit,
        Action::RepeatSubmit,
        Action::Reject,
        Action::RepeatReject,
        Action::Answer,
        Action::RepeatAnswer,
        Action::StaleAnswer,
        Action::Close,
        Action::RepeatClose,
        Action::Dismiss,
        Action::RepeatDismiss,
        Action::Restart,
    ] {
        let database = Database::new();
        let store = database.open();
        let sibling = store.clone();
        let o = opening();
        action.prepare(&store, &o);
        let before = snapshot(&store);
        let changes = store.subscribe_actor_form_changes();
        deny_completion(&store, Fault::Commit);
        assert_recovery(
            action
                .run(&store, &o)
                .expect_err("every accepted transaction must check completion"),
            action,
        );
        assert!(
            !changes.has_changed().unwrap(),
            "no false form notification for {action:?}"
        );
        assert!(matches!(
            sibling.actor_form(&o.origin, &o.mount_id),
            Err(StoreError::AdmissionFenced)
        ));
        drop(sibling);
        drop(store);
        let reopened = database.open();
        assert_eq!(snapshot(&reopened), before, "{action:?}");
    }
}

#[test]
fn failed_commit_retains_first_cause_when_rollback_also_fails() {
    let database = Database::new();
    let store = database.open();
    let sibling = store.clone();
    let o = opening();
    store.open_actor_form(&Authority(true), &o).unwrap();
    store.lock().unwrap().execute_batch("CREATE TABLE form_commit_parent(id INTEGER PRIMARY KEY); CREATE TABLE form_commit_child(id INTEGER REFERENCES form_commit_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER form_commit_fault AFTER INSERT ON events WHEN NEW.kind='actor_form_update' BEGIN INSERT INTO form_commit_child VALUES(1); END;").unwrap();
    let before = snapshot(&store);
    let changes = store.subscribe_actor_form_changes();
    deny_completion(&store, Fault::Rollback);
    let error = store
        .submit_actor_form(&o.origin, &o.mount_id, "one", &json!({"f0":"answer"}))
        .expect_err("deferred foreign key fails COMMIT before denied rollback cleanup");
    assert!(
        matches!(error, StoreError::RecoveryRequired(ref cause) if cause.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)),
        "first COMMIT cause must survive the later authorization failure: {error:?}"
    );
    assert!(!changes.has_changed().unwrap());
    assert!(matches!(
        sibling.ensure_ready(),
        Err(StoreError::AdmissionFenced)
    ));
    drop(sibling);
    drop(store);
    let reopened = database.open();
    assert_eq!(snapshot(&reopened), before);
    let children: i64 = reopened
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM form_commit_child", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(children, 0);
}

#[test]
fn ordinary_form_refusals_preserve_error_and_ready_connection() {
    for action in [
        Action::RefuseAuthority,
        Action::ConflictOpen,
        Action::ConflictSubmit,
        Action::Unavailable,
        Action::InvalidDraft,
        Action::StatementRefusal,
    ] {
        let database = Database::new();
        let store = database.open();
        let o = opening();
        action.prepare(&store, &o);
        let before = snapshot(&store);
        let changes = store.subscribe_actor_form_changes();
        let error = action.run(&store, &o).expect_err("ordinary refusal");
        assert!(
            match action {
                Action::RefuseAuthority => matches!(error, StoreError::ActorOutputRefused),
                Action::ConflictOpen | Action::ConflictSubmit =>
                    matches!(error, StoreError::ConflictingFormOperation),
                Action::Unavailable => matches!(error, StoreError::FormUnavailable),
                Action::InvalidDraft => matches!(error, StoreError::InvalidForm),
                Action::StatementRefusal =>
                    matches!(error, StoreError::Sql(ref cause) if cause.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)),
                _ => unreachable!(),
            },
            "{action:?}: {error:?}"
        );
        store.ensure_ready().unwrap();
        assert!(!changes.has_changed().unwrap());
        assert_eq!(snapshot(&store), before, "{action:?}");
    }
}

#[test]
fn multi_query_read_completion_checks_success_and_early_refusal() {
    use crate::model::{AgentPath, CallId, OperationId, RequestId};
    for deny_rollback in [false, true] {
        for refusal in [false, true] {
            let database = Database::new();
            let store = database.open();
            let sibling = store.clone();
            let head = RequestId("head".into());
            store.create_request(&head, None, "/root").unwrap();
            let operation = OperationId {
                origin: store.standalone_identity(AgentPath("/root".into())),
                request: head.clone(),
                call: CallId("absent".into()),
            };
            let before = snapshot(&store);
            if deny_rollback {
                deny_completion(&store, Fault::Rollback);
            }
            let result = if refusal {
                store.begin_context(&operation, &head).map(|_| ())
            } else {
                store.read_context(&head).map(|_| ())
            };
            match (deny_rollback, refusal) {
                (true, _) => {
                    assert_recovery(
                        result.expect_err("read cleanup fault"),
                        (deny_rollback, refusal),
                    );
                    assert!(matches!(
                        sibling.ensure_ready(),
                        Err(StoreError::AdmissionFenced)
                    ));
                }
                (false, true) => {
                    assert!(matches!(
                        result,
                        Err(StoreError::CheckpointBoundaryNotPending(_))
                    ));
                    sibling.ensure_ready().unwrap();
                }
                (false, false) => {
                    result.unwrap();
                    sibling.ensure_ready().unwrap();
                }
            }
            drop(sibling);
            drop(store);
            let reopened = database.open();
            assert_eq!(snapshot(&reopened), before);
        }
    }
}

#[test]
fn replay_read_completion_checks_success_and_decode_refusal() {
    use crate::{
        model::{Effort, RequestId},
        transport::{ResponsesRequest, ResponsesTurn},
    };
    for deny_rollback in [false, true] {
        for refusal in [false, true] {
            let database = Database::new();
            let store = database.open();
            let sibling = store.clone();
            let head = RequestId("head".into());
            store.create_request(&head, None, "/root").unwrap();
            store
                .record_replay_turn(
                    &head,
                    &ResponsesRequest {
                        input: vec![],
                        instructions: "instructions".into(),
                        tools: vec![].into(),
                        tools_allowed: None,
                        model: "offline".into(),
                        pinned_effort: Effort::Low,
                        session_id: "session".into(),
                    },
                    &ResponsesTurn {
                        response_id: "response".into(),
                        items: vec![],
                        usage: Default::default(),
                    },
                )
                .unwrap();
            if refusal {
                // Keep valid SQLite/JSON storage while refusing the typed decoder.
                store
                    .lock()
                    .unwrap()
                    .execute("UPDATE items SET json='null'", [])
                    .unwrap();
            }
            let before = snapshot(&store);
            if deny_rollback {
                deny_completion(&store, Fault::Rollback);
            }
            let result = store.replay_turns(&head);
            match (deny_rollback, refusal) {
                (true, _) => {
                    assert_recovery(
                        result.expect_err("replay cleanup fault"),
                        (deny_rollback, refusal),
                    );
                    assert!(matches!(
                        sibling.ensure_ready(),
                        Err(StoreError::AdmissionFenced)
                    ));
                }
                (false, true) => {
                    assert!(matches!(result, Err(StoreError::Json(_))));
                    sibling.ensure_ready().unwrap();
                }
                (false, false) => {
                    assert_eq!(result.unwrap().len(), 1);
                    sibling.ensure_ready().unwrap();
                }
            }
            drop(sibling);
            drop(store);
            let reopened = database.open();
            assert_eq!(snapshot(&reopened), before);
        }
    }
}
