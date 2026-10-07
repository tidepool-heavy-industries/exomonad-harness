pub const VERSION: u32 = 13;
pub const SQL: &str = include_str!("schema.sql");
const MODEL_REQUEST_INDEXES: &str = "
    CREATE INDEX IF NOT EXISTS events_model_turn_recent ON events(id DESC,request_id) WHERE kind='model_turn';
    DROP INDEX IF EXISTS events_model_settled_recent;
    CREATE INDEX IF NOT EXISTS events_model_outcomes_recent ON events(id DESC,request_id) WHERE kind IN ('model_turn','request_failed','model_interrupted');
";

pub fn initialize(conn: &mut rusqlite::Connection) -> super::Result<()> {
    let has_version: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
        [],
        |r| r.get(0),
    )?;
    if !has_version {
        let tx = conn.transaction()?;
        tx.execute_batch(SQL)?;
        tx.execute_batch(MODEL_REQUEST_INDEXES)?;
        tx.execute_batch(super::actor_output::INDEXES)?;
        tx.execute_batch(super::validation::INDEXES)?;
        tx.execute("INSERT INTO schema_version(version) VALUES (?1)", [VERSION])?;
        super::schema_migration::ensure_store_id(&tx)?;
        tx.commit()?;
        return Ok(());
    }
    let versions: Vec<u32> = {
        let mut stmt = conn.prepare("SELECT version FROM schema_version")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    if versions.len() != 1 {
        return Err(rusqlite::Error::InvalidParameterName(
            "schema_version must contain exactly one row".into(),
        )
        .into());
    }
    let version = versions[0];
    if version > VERSION {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "database schema {version} is newer than supported {VERSION}"
        ))
        .into());
    }
    if version == VERSION {
        super::schema_migration::validate_checkpoint_metadata(conn)?;
        conn.execute_batch(MODEL_REQUEST_INDEXES)?;
        conn.execute_batch(super::actor_output::INDEXES)?;
        conn.execute_batch(super::validation::INDEXES)?;
        return Ok(());
    }
    let tx = conn.transaction()?;
    // v1 stored second-resolution Unix timestamps. Convert existing records once;
    // writers explicitly provide millisecond timestamps after this migration.
    for table in if version == 1 {
        vec!["requests", "events", "envelopes", "decisions"]
    } else {
        vec![]
    } {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |r| r.get(0),
        )?;
        if exists {
            tx.execute(
                &format!("UPDATE {table} SET created_at=created_at*1000"),
                [],
            )?;
        }
    }
    let columns = {
        let mut stmt = tx.prepare("PRAGMA table_info(requests)")?;
        stmt.query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for column in ["input_tokens", "output_tokens", "cost_micros"] {
        if !columns.iter().any(|c| c == column) {
            tx.execute_batch(&format!(
                "ALTER TABLE requests ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0;"
            ))?;
        }
    }
    for column in ["embedded_run", "embedded_incarnation", "round_phase"] {
        if !columns.iter().any(|existing| existing == column) {
            tx.execute_batch(&format!("ALTER TABLE requests ADD COLUMN {column} TEXT;"))?;
        }
    }
    let legacy_claims: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='claims')",
        [],
        |row| row.get(0),
    )?;
    if legacy_claims && version < 5 {
        tx.execute_batch("ALTER TABLE claims RENAME TO legacy_claims; DROP INDEX IF EXISTS claims_request; DROP INDEX IF EXISTS claims_operation;")?;
    }
    let legacy_inputs: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='embedded_inputs')",
        [],
        |r| r.get(0),
    )?;
    if legacy_inputs && version < 7 {
        tx.execute_batch("ALTER TABLE embedded_inputs RENAME TO legacy_embedded_inputs;")?;
    }
    tx.execute_batch(SQL)?;
    let item_columns = {
        let mut statement = tx.prepare("PRAGMA table_info(request_items)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for column in [
        "source_request",
        "source_position",
        "context_sources",
        "context_note",
        "context_overlays",
    ] {
        if !item_columns.iter().any(|existing| existing == column) {
            let kind = if column == "context_note" {
                "INTEGER NOT NULL DEFAULT 0"
            } else if column == "source_position" {
                "INTEGER"
            } else {
                "TEXT"
            };
            tx.execute_batch(&format!(
                "ALTER TABLE request_items ADD COLUMN {column} {kind};"
            ))?;
        }
    }
    let claim_columns = {
        let mut statement = tx.prepare("PRAGMA table_info(claims)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };
    if !claim_columns.iter().any(|column| column == "terminal_json") {
        tx.execute_batch("ALTER TABLE claims ADD COLUMN terminal_json TEXT;")?;
    }
    tx.execute_batch(MODEL_REQUEST_INDEXES)?;
    tx.execute_batch(super::actor_output::INDEXES)?;
    tx.execute_batch(super::validation::INDEXES)?;
    if legacy_inputs && version < 7 {
        let old_count: i64 =
            tx.query_row("SELECT COUNT(*) FROM legacy_embedded_inputs", [], |r| {
                r.get(0)
            })?;
        let copied=tx.execute("INSERT INTO embedded_inputs(run_id,agent_path,incarnation,operation_id,envelope_id,item_hash) SELECT b.run_id,ei.agent_path,b.incarnation,ei.operation_id,ei.envelope_id,ei.item_hash FROM legacy_embedded_inputs ei JOIN embedded_bindings b ON b.agent_path=ei.agent_path",[])?;
        if copied as i64 != old_count {
            return Err(super::StoreError::InvalidEmbeddedBinding);
        }
        tx.execute_batch("DROP TABLE legacy_embedded_inputs;")?;
    }
    let store_id = super::schema_migration::ensure_store_id(&tx)?;
    if legacy_claims && version < 5 {
        super::schema_migration::migrate_claims(&tx, &store_id)?;
        tx.execute_batch("DROP TABLE legacy_claims")?;
    }
    super::schema_migration::migrate_checkpoint_metadata(&tx)?;
    tx.execute("UPDATE schema_version SET version=?1", [VERSION])?;
    Ok(tx.commit()?)
}
