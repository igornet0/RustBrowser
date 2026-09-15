use crate::migrations;
use browser_core::{BrowserError, BrowserResult};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{error, info};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DownloadId(pub Uuid);

impl DownloadId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: DownloadId,
    pub url: Url,
    pub filename: String,
    pub destination: PathBuf,
    pub progress: f64,
    pub state: DownloadState,
    pub error: Option<String>,
}

/// Application-level download manager.
///
/// Servo 0.5.0 has no public Content-Disposition download callback.
/// Downloads are started explicitly (toolbar / URL).
pub struct DownloadManager {
    db_path: PathBuf,
    download_dir: PathBuf,
    items: Arc<Mutex<Vec<Download>>>,
}

impl DownloadManager {
    pub fn open(db_path: &Path, download_dir: PathBuf) -> BrowserResult<Self> {
        std::fs::create_dir_all(&download_dir)?;
        let conn =
            Connection::open(db_path).map_err(|e| BrowserError::database(e.to_string()))?;
        migrations::migrate(
            &conn,
            &[
                "CREATE TABLE IF NOT EXISTS downloads (
                    id TEXT PRIMARY KEY,
                    url TEXT NOT NULL,
                    filename TEXT NOT NULL,
                    destination TEXT NOT NULL,
                    progress REAL NOT NULL,
                    state TEXT NOT NULL,
                    error TEXT
                );",
            ],
        )?;

        let mut stmt = conn
            .prepare(
                "SELECT id, url, filename, destination, progress, state, error FROM downloads
                 ORDER BY rowid DESC",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([], map_download)
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| BrowserError::database(e.to_string()))?);
        }

        Ok(Self {
            db_path: db_path.to_path_buf(),
            download_dir,
            items: Arc::new(Mutex::new(items)),
        })
    }

    pub fn list(&self) -> BrowserResult<Vec<Download>> {
        self.items
            .lock()
            .map(|g| g.clone())
            .map_err(|_| BrowserError::database("downloads lock"))
    }

    pub fn start(&self, url: Url) -> BrowserResult<DownloadId> {
        let filename = url
            .path_segments()
            .and_then(|s| s.last())
            .filter(|s| !s.is_empty())
            .unwrap_or("download.bin")
            .to_string();
        let destination = unique_path(&self.download_dir, &filename);
        let id = DownloadId::new();
        let download = Download {
            id,
            url: url.clone(),
            filename,
            destination: destination.clone(),
            progress: 0.0,
            state: DownloadState::Queued,
            error: None,
        };
        persist(&self.db_path, &download)?;
        {
            let mut items = self
                .items
                .lock()
                .map_err(|_| BrowserError::database("downloads lock"))?;
            items.insert(0, download);
        }

        let items = self.items.clone();
        let db_path = self.db_path.clone();
        thread::Builder::new()
            .name("download".into())
            .spawn(move || run_download(id, url, destination, items, db_path))
            .map_err(|e| BrowserError::Other(e.to_string()))?;
        Ok(id)
    }

    pub fn cancel(&self, id: DownloadId) -> BrowserResult<()> {
        let mut items = self
            .items
            .lock()
            .map_err(|_| BrowserError::database("downloads lock"))?;
        if let Some(d) = items.iter_mut().find(|d| d.id == id) {
            if matches!(
                d.state,
                DownloadState::Queued | DownloadState::Downloading
            ) {
                d.state = DownloadState::Cancelled;
                persist(&self.db_path, d)?;
            }
        }
        Ok(())
    }
}

fn run_download(
    id: DownloadId,
    url: Url,
    destination: PathBuf,
    items: Arc<Mutex<Vec<Download>>>,
    db_path: PathBuf,
) {
    let set = |state: DownloadState, progress: f64, error: Option<String>| -> bool {
        let Ok(mut list) = items.lock() else {
            return true;
        };
        let Some(d) = list.iter_mut().find(|d| d.id == id) else {
            return true;
        };
        if d.state == DownloadState::Cancelled {
            return true;
        }
        d.state = state;
        d.progress = progress;
        d.error = error.clone();
        let _ = persist(&db_path, d);
        false
    };

    if set(DownloadState::Downloading, 0.05, None) {
        return;
    }

    let client = match reqwest::blocking::Client::builder().use_rustls_tls().build() {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "download errors");
            set(DownloadState::Failed, 0.0, Some(e.to_string()));
            return;
        }
    };

    let response = match client.get(url).send() {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "download errors");
            set(DownloadState::Failed, 0.0, Some(e.to_string()));
            return;
        }
    };

    if !response.status().is_success() {
        let msg = format!("HTTP {}", response.status());
        error!(error = %msg, "download errors");
        set(DownloadState::Failed, 0.0, Some(msg));
        return;
    }

    let bytes = match response.bytes() {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "download errors");
            set(DownloadState::Failed, 0.0, Some(e.to_string()));
            return;
        }
    };

    if let Ok(list) = items.lock() {
        if list
            .iter()
            .find(|d| d.id == id)
            .is_some_and(|d| d.state == DownloadState::Cancelled)
        {
            return;
        }
    }

    if let Err(e) = std::fs::write(&destination, &bytes) {
        error!(error = %e, "download errors");
        set(DownloadState::Failed, 0.0, Some(e.to_string()));
        return;
    }

    info!(file = %destination.display(), "download completed");
    set(DownloadState::Completed, 1.0, None);
}

fn persist(db_path: &Path, download: &Download) -> BrowserResult<()> {
    let conn = Connection::open(db_path).map_err(|e| BrowserError::database(e.to_string()))?;
    conn.execute(
        "INSERT INTO downloads (id, url, filename, destination, progress, state, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            progress = excluded.progress,
            state = excluded.state,
            error = excluded.error",
        params![
            download.id.0.to_string(),
            download.url.as_str(),
            download.filename,
            download.destination.to_string_lossy(),
            download.progress,
            state_str(download.state),
            download.error,
        ],
    )
    .map_err(|e| BrowserError::database(e.to_string()))?;
    Ok(())
}

fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin");
    for i in 1..10_000 {
        let p = dir.join(format!("{stem}-{i}.{ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}.{ext}", Uuid::new_v4()))
}

fn state_str(state: DownloadState) -> &'static str {
    match state {
        DownloadState::Queued => "queued",
        DownloadState::Downloading => "downloading",
        DownloadState::Completed => "completed",
        DownloadState::Failed => "failed",
        DownloadState::Cancelled => "cancelled",
    }
}

fn parse_state(s: &str) -> DownloadState {
    match s {
        "downloading" => DownloadState::Downloading,
        "completed" => DownloadState::Completed,
        "failed" => DownloadState::Failed,
        "cancelled" => DownloadState::Cancelled,
        _ => DownloadState::Queued,
    }
}

fn map_download(row: &rusqlite::Row<'_>) -> rusqlite::Result<Download> {
    let id: String = row.get(0)?;
    let url: String = row.get(1)?;
    let state: String = row.get(5)?;
    Ok(Download {
        id: DownloadId(Uuid::parse_str(&id).unwrap_or_default()),
        url: Url::parse(&url).unwrap_or_else(|_| Url::parse("about:blank").unwrap()),
        filename: row.get(2)?,
        destination: PathBuf::from(row.get::<_, String>(3)?),
        progress: row.get(4)?,
        state: parse_state(&state),
        error: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_queued_entry() {
        let dir = tempdir().unwrap();
        let mgr =
            DownloadManager::open(&dir.path().join("d.db"), dir.path().join("files")).unwrap();
        let id = mgr
            .start(Url::parse("https://example.com/file.txt").unwrap())
            .unwrap();
        let list = mgr.list().unwrap();
        assert!(list.iter().any(|d| d.id == id));
    }
}
