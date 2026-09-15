//! Persistent browser profile data: history, bookmarks, settings, session, downloads.

mod bookmarks;
mod detect;
mod downloads;
mod history;
mod import;
mod migrations;
mod passwords;
mod paths;
mod permissions;
mod profile;
mod session;
mod settings;
mod store;

pub use bookmarks::{Bookmark, BookmarkFolder, BookmarkId, BookmarkStore};
pub use detect::{discover_browsers, import_from_detected, BrowserKind, DetectedBrowser};
pub use downloads::{Download, DownloadId, DownloadManager, DownloadState};
pub use history::{HistoryEntry, HistoryStore};
pub use import::{import_path, ImportSummary, ImportedTab};
pub use passwords::{
    credential_origin, Credential, CredentialId, CredentialStore, EncryptedFileSecretBackend,
    KeyringSecretBackend, MemorySecretBackend, SecretBackend,
};
pub use paths::ProfilePaths;
pub use permissions::{PermissionDecision, PermissionKind, PermissionManager, PermissionRecord};
pub use profile::{BrowserProfile, ProfileId, ProfileManager};
pub use session::{atomic_write_json, SessionState, SessionStore, SessionTab, WindowState};
pub use settings::{ColorScheme, NetworkMode, Settings, StartupBehavior, Theme, UiLanguage};
pub use store::ProfileStore;
