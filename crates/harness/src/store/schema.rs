pub const VERSION: u32 = 1;
pub const SQL: &str = include_str!("schema.sql");

pub fn initialize(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SQL)?;
    let version: Option<u32> = conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |r| r.get(0)).optional()?;
    match version {
        Some(v) if v == VERSION => Ok(()),
        Some(v) => Err(rusqlite::Error::InvalidParameterName(format!("unsupported schema version {v}"))),
        None => { conn.execute("INSERT INTO schema_version(version) VALUES (?1)", [VERSION])?; Ok(()) }
    }
}

use rusqlite::OptionalExtension;
