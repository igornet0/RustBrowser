//! SQL schema bootstrap (simple migrations table).

use browser_core::{BrowserError, BrowserResult};
use rusqlite::Connection;

pub fn migrate(conn: &Connection, statements: &[&str]) -> BrowserResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| BrowserError::database(e.to_string()))?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| BrowserError::database(e.to_string()))?;

    for (idx, sql) in statements.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .map_err(|e| BrowserError::database(format!("migration {version}: {e}")))?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, datetime('now'))",
            [version],
        )
        .map_err(|e| BrowserError::database(e.to_string()))?;
    }
    Ok(())
}
