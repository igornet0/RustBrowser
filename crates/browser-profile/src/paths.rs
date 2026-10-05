use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProfilePaths {
    pub root: PathBuf,
    pub profile_db: PathBuf,
    pub history_db: PathBuf,
    pub bookmarks_db: PathBuf,
    pub preferences: PathBuf,
    /// Generated hosts-format file for Servo `Opts.host_file`.
    pub local_hosts: PathBuf,
    pub session: PathBuf,
    pub downloads_db: PathBuf,
    pub passwords_db: PathBuf,
    pub permissions_db: PathBuf,
    pub storage: PathBuf,
    pub cache: PathBuf,
    pub downloads_dir: PathBuf,
}

impl ProfilePaths {
    pub fn for_directory(dir: impl Into<PathBuf>) -> Self {
        let root = dir.into();
        Self {
            profile_db: root.join("profile.db"),
            history_db: root.join("history.db"),
            bookmarks_db: root.join("bookmarks.db"),
            preferences: root.join("preferences.json"),
            local_hosts: root.join("local_hosts"),
            session: root.join("session.json"),
            downloads_db: root.join("downloads.db"),
            passwords_db: root.join("passwords.db"),
            permissions_db: root.join("permissions.db"),
            storage: root.join("storage"),
            cache: root.join("cache"),
            downloads_dir: root.join("downloads"),
            root,
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.storage)?;
        std::fs::create_dir_all(&self.cache)?;
        std::fs::create_dir_all(&self.downloads_dir)?;
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
