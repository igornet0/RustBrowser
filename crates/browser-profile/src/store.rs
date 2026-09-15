use crate::bookmarks::BookmarkStore;
use crate::downloads::DownloadManager;
use crate::history::HistoryStore;
use crate::passwords::CredentialStore;
use crate::paths::ProfilePaths;
use crate::permissions::PermissionManager;
use crate::session::SessionStore;
use crate::settings::Settings;
use browser_core::{BrowserError, BrowserResult};
use tracing::info;

/// Open stores for a single profile directory.
pub struct ProfileStore {
    pub paths: ProfilePaths,
    pub history: HistoryStore,
    pub bookmarks: BookmarkStore,
    pub credentials: CredentialStore,
    pub permissions: PermissionManager,
    pub settings: Settings,
    pub session: SessionStore,
    pub downloads: DownloadManager,
}

impl ProfileStore {
    pub fn open(paths: ProfilePaths) -> BrowserResult<Self> {
        paths
            .ensure_dirs()
            .map_err(|e| BrowserError::profile(e.to_string()))?;
        info!(path = %paths.root.display(), "profile loading");

        let history = HistoryStore::open(&paths.history_db)?;
        let bookmarks = BookmarkStore::open(&paths.bookmarks_db)?;
        let credentials = CredentialStore::open(&paths.passwords_db)?;
        let permissions = PermissionManager::open(&paths.permissions_db)?;
        let settings = Settings::load_or_default(&paths.preferences)?;
        let session = SessionStore::new(paths.session.clone());
        let downloads = DownloadManager::open(&paths.downloads_db, paths.downloads_dir.clone())?;

        Ok(Self {
            paths,
            history,
            bookmarks,
            credentials,
            permissions,
            settings,
            session,
            downloads,
        })
    }

    pub fn save_settings(&self) -> BrowserResult<()> {
        self.settings.save(&self.paths.preferences)
    }
}
