use crate::migrations;
use browser_core::{BrowserError, BrowserResult};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use tracing::error;
use url::Url;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: i64,
    pub url: Url,
    pub title: String,
    pub visited_at: DateTime<Utc>,
    pub visit_count: i64,
}

pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    pub fn open(path: &Path) -> BrowserResult<Self> {
        let conn = Connection::open(path).map_err(|e| BrowserError::database(e.to_string()))?;
        migrations::migrate(
            &conn,
            &[
                "CREATE TABLE IF NOT EXISTS history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    url TEXT NOT NULL UNIQUE,
                    title TEXT NOT NULL DEFAULT '',
                    visited_at TEXT NOT NULL,
                    visit_count INTEGER NOT NULL DEFAULT 1
                );
                CREATE INDEX IF NOT EXISTS idx_history_visited ON history(visited_at DESC);
                CREATE INDEX IF NOT EXISTS idx_history_url ON history(url);",
            ],
        )?;
        Ok(Self { conn })
    }

    pub fn record_visit(&self, url: &Url, title: &str) -> BrowserResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO history (url, title, visited_at, visit_count)
                 VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(url) DO UPDATE SET
                    title = excluded.title,
                    visited_at = excluded.visited_at,
                    visit_count = history.visit_count + 1",
                params![url.as_str(), title, now],
            )
            .map_err(|e| {
                error!(error = %e, "database errors");
                BrowserError::database(e.to_string())
            })?;
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> BrowserResult<Vec<HistoryEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, url, title, visited_at, visit_count
                 FROM history ORDER BY visited_at DESC LIMIT ?1",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([limit as i64], map_row)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        collect_rows(rows)
    }

    pub fn search(&self, query: &str, limit: usize) -> BrowserResult<Vec<HistoryEntry>> {
        let like = format!("%{query}%");
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, url, title, visited_at, visit_count
                 FROM history
                 WHERE url LIKE ?1 OR title LIKE ?1
                 ORDER BY visited_at DESC LIMIT ?2",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map(params![like, limit as i64], map_row)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        collect_rows(rows)
    }

    pub fn delete(&self, id: i64) -> BrowserResult<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", [id])
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn clear_all(&self) -> BrowserResult<()> {
        self.conn
            .execute("DELETE FROM history", [])
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn clear_since(&self, since: DateTime<Utc>) -> BrowserResult<()> {
        self.conn
            .execute(
                "DELETE FROM history WHERE visited_at >= ?1",
                [since.to_rfc3339()],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let url_str: String = row.get(1)?;
    let visited: String = row.get(3)?;
    Ok(HistoryEntry {
        id: row.get(0)?,
        url: Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap()),
        title: row.get(2)?,
        visited_at: DateTime::parse_from_rfc3339(&visited)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        visit_count: row.get(4)?,
    })
}

fn collect_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry>>,
) -> BrowserResult<Vec<HistoryEntry>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| BrowserError::database(e.to_string()))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn record_and_search() {
        let dir = tempdir().unwrap();
        let store = HistoryStore::open(&dir.path().join("h.db")).unwrap();
        let url = Url::parse("https://example.com").unwrap();
        store.record_visit(&url, "Example").unwrap();
        store.record_visit(&url, "Example").unwrap();
        let recent = store.list_recent(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].visit_count, 2);
        let found = store.search("example", 10).unwrap();
        assert_eq!(found.len(), 1);
        store.delete(found[0].id).unwrap();
        assert!(store.list_recent(10).unwrap().is_empty());
    }
}
