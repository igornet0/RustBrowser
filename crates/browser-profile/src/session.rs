use browser_core::{BrowserError, BrowserResult};
use browser_core::{Tab, TabId};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 1280,
            height: 800,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    pub id: TabId,
    pub title: String,
    pub url: Option<Url>,
}

impl From<&Tab> for SessionTab {
    fn from(tab: &Tab) -> Self {
        Self {
            id: tab.id,
            title: tab.title.clone(),
            url: tab.url.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub tabs: Vec<SessionTab>,
    pub active_tab: TabId,
    pub window: WindowState,
}

/// Crash-safe session persistence with atomic replace writes.
pub struct SessionStore {
    path: PathBuf,
    lock_path: PathBuf,
    last_write: Option<Instant>,
    debounce: Duration,
    pending: Option<SessionState>,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        let lock_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("running.lock");
        Self {
            path,
            lock_path,
            last_write: None,
            debounce: Duration::from_millis(750),
            pending: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// True if a previous run left `running.lock` behind (unclean exit).
    pub fn previous_session_unclean(&self) -> bool {
        self.lock_path.exists()
    }

    pub fn mark_running(&self) -> BrowserResult<()> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&self.lock_path)?;
        let pid = std::process::id();
        writeln!(f, "{pid}")?;
        f.sync_all()?;
        Ok(())
    }

    pub fn mark_clean_shutdown(&self) -> BrowserResult<()> {
        if self.lock_path.exists() {
            fs::remove_file(&self.lock_path)?;
        }
        Ok(())
    }

    /// Queue a checkpoint; flushed by debounce or [`Self::flush`].
    pub fn schedule(&mut self, state: SessionState) {
        self.pending = Some(state);
        if self.last_write.is_none()
            || self
                .last_write
                .is_some_and(|t| t.elapsed() >= self.debounce)
        {
            let _ = self.flush();
        }
    }

    /// Force write any pending state (and optionally the provided snapshot).
    pub fn checkpoint(&mut self, state: SessionState) -> BrowserResult<()> {
        self.pending = Some(state);
        self.flush()
    }

    pub fn flush(&mut self) -> BrowserResult<()> {
        let Some(state) = self.pending.take() else {
            return Ok(());
        };
        self.save(&state)?;
        self.last_write = Some(Instant::now());
        Ok(())
    }

    pub fn save(&self, state: &SessionState) -> BrowserResult<()> {
        atomic_write_json(&self.path, state)
    }

    pub fn load(&self) -> BrowserResult<Option<SessionState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let data = match fs::read_to_string(&self.path) {
            Ok(d) => d,
            Err(e) => {
                warn!(?e, "session read failed");
                return Ok(None);
            }
        };
        match serde_json::from_str(&data) {
            Ok(state) => Ok(Some(state)),
            Err(e) => {
                warn!(?e, "corrupted session.json — ignoring");
                // Keep a backup for debugging; do not crash startup.
                let bak = self.path.with_extension("json.corrupt");
                let _ = fs::rename(&self.path, &bak);
                Ok(None)
            }
        }
    }

    pub fn clear(&self) -> BrowserResult<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

/// Write JSON via tmp + fsync + rename so a crash mid-write cannot corrupt the file.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> BrowserResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let data =
        serde_json::to_string_pretty(value).map_err(|e| BrowserError::Storage(e.to_string()))?;

    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| BrowserError::Storage(e.to_string()))?;
        file.write_all(data.as_bytes())
            .map_err(|e| BrowserError::Storage(e.to_string()))?;
        file.sync_all()
            .map_err(|e| BrowserError::Storage(e.to_string()))?;
    }

    fs::rename(&tmp, path).map_err(|e| BrowserError::Storage(e.to_string()))?;
    info!(path = %path.display(), "session checkpoint");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn session_roundtrip_atomic() {
        let dir = tempdir().unwrap();
        let mut store = SessionStore::new(dir.path().join("session.json"));
        let id = TabId::new();
        let state = SessionState {
            tabs: vec![SessionTab {
                id,
                title: "Example".into(),
                url: Some(Url::parse("https://example.com").unwrap()),
            }],
            active_tab: id,
            window: WindowState::default(),
        };
        store.checkpoint(state.clone()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.active_tab, id);
        assert!(!dir.path().join("session.json.tmp").exists());
    }

    #[test]
    fn corrupted_session_does_not_crash() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "{not-json").unwrap();
        let store = SessionStore::new(path);
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn running_lock_detects_unclean() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("session.json"));
        assert!(!store.previous_session_unclean());
        store.mark_running().unwrap();
        assert!(store.previous_session_unclean());
        store.mark_clean_shutdown().unwrap();
        assert!(!store.previous_session_unclean());
    }
}
