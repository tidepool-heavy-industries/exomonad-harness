pub const VERSION: u32 = 5;
pub const SQL: &str = include_str!("schema.sql");

pub fn initialize(conn: &mut rusqlite::Connection) -> super::Result<()> {
    let has_version: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
        [],
        |r| r.get(0),
    )?;
    if !has_version {
        let tx = conn.transaction()?;
        tx.execute_batch(SQL)?;
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
    let legacy_claims: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='claims')",
        [],
        |row| row.get(0),
    )?;
    if legacy_claims {
        tx.execute_batch("ALTER TABLE claims RENAME TO legacy_claims; DROP INDEX IF EXISTS claims_request; DROP INDEX IF EXISTS claims_operation;")?;
    }
    tx.execute_batch(SQL)?;
    let store_id = super::schema_migration::ensure_store_id(&tx)?;
    if legacy_claims {
        super::schema_migration::migrate_claims(&tx, &store_id)?;
        tx.execute_batch("DROP TABLE legacy_claims")?;
    }
    tx.execute("UPDATE schema_version SET version=?1", [VERSION])?;
    Ok(tx.commit()?)
}
