use crate::migrations;
use browser_core::{BrowserError, BrowserResult};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use url::Url;

pub type BookmarkId = i64;
pub type FolderId = i64;

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub id: BookmarkId,
    pub url: Url,
    pub title: String,
    pub folder_id: Option<FolderId>,
    pub created_at: DateTime<Utc>,
    pub on_bookmarks_bar: bool,
}

#[derive(Debug, Clone)]
pub struct BookmarkFolder {
    pub id: FolderId,
    pub name: String,
    pub parent_id: Option<FolderId>,
}

pub struct BookmarkStore {
    conn: Connection,
}

impl BookmarkStore {
    pub fn open(path: &Path) -> BrowserResult<Self> {
        let conn = Connection::open(path).map_err(|e| BrowserError::database(e.to_string()))?;
        migrations::migrate(
            &conn,
            &[
                "CREATE TABLE IF NOT EXISTS folders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS bookmarks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    url TEXT NOT NULL,
                    title TEXT NOT NULL,
                    folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
                    created_at TEXT NOT NULL,
                    on_bookmarks_bar INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_bookmarks_title ON bookmarks(title);
                CREATE INDEX IF NOT EXISTS idx_bookmarks_url ON bookmarks(url);",
            ],
        )?;
        let store = Self { conn };
        store.ensure_default_folder()?;
        Ok(store)
    }

    fn ensure_default_folder(&self) -> BrowserResult<()> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE name = 'Bookmarks Bar' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| BrowserError::database(e.to_string()))?;
        if exists.is_none() {
            self.conn
                .execute(
                    "INSERT INTO folders (name, parent_id) VALUES ('Bookmarks Bar', NULL)",
                    [],
                )
                .map_err(|e| BrowserError::database(e.to_string()))?;
        }
        Ok(())
    }

    pub fn bookmarks_bar_folder_id(&self) -> BrowserResult<FolderId> {
        self.conn
            .query_row(
                "SELECT id FROM folders WHERE name = 'Bookmarks Bar' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| BrowserError::database(e.to_string()))
    }

    pub fn add(
        &self,
        url: &Url,
        title: &str,
        folder_id: Option<FolderId>,
        on_bar: bool,
    ) -> BrowserResult<BookmarkId> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO bookmarks (url, title, folder_id, created_at, on_bookmarks_bar)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![url.as_str(), title, folder_id, now, on_bar as i64],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn remove(&self, id: BookmarkId) -> BrowserResult<()> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE id = ?1", [id])
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn edit(
        &self,
        id: BookmarkId,
        url: &Url,
        title: &str,
        folder_id: Option<FolderId>,
    ) -> BrowserResult<()> {
        self.conn
            .execute(
                "UPDATE bookmarks SET url = ?1, title = ?2, folder_id = ?3 WHERE id = ?4",
                params![url.as_str(), title, folder_id, id],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn create_folder(&self, name: &str, parent_id: Option<FolderId>) -> BrowserResult<FolderId> {
        self.conn
            .execute(
                "INSERT INTO folders (name, parent_id) VALUES (?1, ?2)",
                params![name, parent_id],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_folders(&self) -> BrowserResult<Vec<BookmarkFolder>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, parent_id FROM folders ORDER BY name")
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(BookmarkFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent_id: row.get(2)?,
                })
            })
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| BrowserError::database(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn bookmarks_bar(&self) -> BrowserResult<Vec<Bookmark>> {
        self.list_where("on_bookmarks_bar = 1")
    }

    pub fn search(&self, query: &str) -> BrowserResult<Vec<Bookmark>> {
        let like = format!("%{query}%");
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, url, title, folder_id, created_at, on_bookmarks_bar
                 FROM bookmarks
                 WHERE title LIKE ?1 OR url LIKE ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([like], map_bookmark)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| BrowserError::database(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn list_all(&self) -> BrowserResult<Vec<Bookmark>> {
        self.list_where("1=1")
    }

    fn list_where(&self, where_clause: &str) -> BrowserResult<Vec<Bookmark>> {
        let sql = format!(
            "SELECT id, url, title, folder_id, created_at, on_bookmarks_bar
             FROM bookmarks WHERE {where_clause} ORDER BY created_at DESC"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([], map_bookmark)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| BrowserError::database(e.to_string()))?);
        }
        Ok(out)
    }
}

fn map_bookmark(row: &rusqlite::Row<'_>) -> rusqlite::Result<Bookmark> {
    let url_str: String = row.get(1)?;
    let created: String = row.get(4)?;
    Ok(Bookmark {
        id: row.get(0)?,
        url: Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap()),
        title: row.get(2)?,
        folder_id: row.get(3)?,
        created_at: DateTime::parse_from_rfc3339(&created)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        on_bookmarks_bar: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_search_remove() {
        let dir = tempdir().unwrap();
        let store = BookmarkStore::open(&dir.path().join("b.db")).unwrap();
        let url = Url::parse("https://example.com").unwrap();
        let id = store.add(&url, "Example", None, true).unwrap();
        assert_eq!(store.bookmarks_bar().unwrap().len(), 1);
        assert_eq!(store.search("Exam").unwrap().len(), 1);
        store
            .edit(id, &Url::parse("https://example.org").unwrap(), "Org", None)
            .unwrap();
        store.remove(id).unwrap();
        assert!(store.list_all().unwrap().is_empty());
    }
}
