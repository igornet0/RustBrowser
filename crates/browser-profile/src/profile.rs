use crate::paths::ProfilePaths;
use crate::store::ProfileStore;
use browser_core::{BrowserError, BrowserResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProfileId(pub Uuid);

impl ProfileId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfile {
    pub id: ProfileId,
    pub name: String,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfilesIndex {
    active: ProfileId,
    profiles: Vec<BrowserProfile>,
}

/// Manages profile directories under a root (e.g. `~/Library/.../profiles` or `./data/profiles`).
pub struct ProfileManager {
    root: PathBuf,
    index_path: PathBuf,
    index: ProfilesIndex,
}

impl ProfileManager {
    pub fn open(root: impl Into<PathBuf>) -> BrowserResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| BrowserError::profile(e.to_string()))?;
        let index_path = root.join("profiles.json");
        let index = if index_path.exists() {
            let data = fs::read_to_string(&index_path)?;
            serde_json::from_str(&data).map_err(|e| BrowserError::profile(e.to_string()))?
        } else {
            let id = ProfileId::new();
            let directory = root.join("Default");
            fs::create_dir_all(&directory).map_err(|e| BrowserError::profile(e.to_string()))?;
            let index = ProfilesIndex {
                active: id,
                profiles: vec![BrowserProfile {
                    id,
                    name: "Default".into(),
                    directory,
                }],
            };
            let mgr = Self {
                root: root.clone(),
                index_path: index_path.clone(),
                index,
            };
            mgr.save_index()?;
            return Ok(mgr);
        };
        Ok(Self {
            root,
            index_path,
            index,
        })
    }

    pub fn default_root() -> PathBuf {
        if let Some(proj) = directories::ProjectDirs::from("org", "RustBrowser", "RustBrowser") {
            return proj.data_dir().join("profiles");
        }
        PathBuf::from("data/profiles")
    }

    fn save_index(&self) -> BrowserResult<()> {
        let data = serde_json::to_string_pretty(&self.index)
            .map_err(|e| BrowserError::profile(e.to_string()))?;
        fs::write(&self.index_path, data)?;
        Ok(())
    }

    pub fn list(&self) -> &[BrowserProfile] {
        &self.index.profiles
    }

    pub fn active(&self) -> BrowserResult<&BrowserProfile> {
        self.index
            .profiles
            .iter()
            .find(|p| p.id == self.index.active)
            .ok_or_else(|| BrowserError::profile("active profile missing"))
    }

    pub fn create(&mut self, name: impl Into<String>) -> BrowserResult<ProfileId> {
        let name = name.into();
        let id = ProfileId::new();
        let dir_name = sanitize_name(&name);
        let directory = unique_dir(&self.root, &dir_name);
        fs::create_dir_all(&directory).map_err(|e| BrowserError::profile(e.to_string()))?;
        info!(%id, %name, "profile creation");
        self.index.profiles.push(BrowserProfile {
            id,
            name,
            directory,
        });
        self.save_index()?;
        Ok(id)
    }

    pub fn delete(&mut self, id: ProfileId) -> BrowserResult<()> {
        if self.index.profiles.len() == 1 {
            return Err(BrowserError::profile("cannot delete the last profile"));
        }
        let Some(idx) = self.index.profiles.iter().position(|p| p.id == id) else {
            return Err(BrowserError::profile(format!("profile {id} not found")));
        };
        let removed = self.index.profiles.remove(idx);
        if removed.directory.exists() {
            fs::remove_dir_all(&removed.directory)
                .map_err(|e| BrowserError::profile(e.to_string()))?;
        }
        if self.index.active == id {
            self.index.active = self.index.profiles[0].id;
        }
        self.save_index()?;
        Ok(())
    }

    pub fn rename(&mut self, id: ProfileId, name: impl Into<String>) -> BrowserResult<()> {
        let name = name.into();
        let profile = self
            .index
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| BrowserError::profile(format!("profile {id} not found")))?;
        profile.name = name;
        self.save_index()?;
        Ok(())
    }

    pub fn switch(&mut self, id: ProfileId) -> BrowserResult<()> {
        if !self.index.profiles.iter().any(|p| p.id == id) {
            return Err(BrowserError::profile(format!("profile {id} not found")));
        }
        self.index.active = id;
        self.save_index()?;
        Ok(())
    }

    pub fn open_active_store(&self) -> BrowserResult<ProfileStore> {
        let profile = self.active()?;
        ProfileStore::open(ProfilePaths::for_directory(&profile.directory))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.is_empty() {
        "Profile".into()
    } else {
        s
    }
}

fn unique_dir(root: &Path, base: &str) -> PathBuf {
    let candidate = root.join(base);
    if !candidate.exists() {
        return candidate;
    }
    for i in 2..10_000 {
        let p = root.join(format!("{base}_{i}"));
        if !p.exists() {
            return p;
        }
    }
    root.join(format!("{base}_{}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_switch_rename() {
        let dir = tempdir().unwrap();
        let mut mgr = ProfileManager::open(dir.path()).unwrap();
        assert_eq!(mgr.list().len(), 1);
        let work = mgr.create("Work").unwrap();
        mgr.switch(work).unwrap();
        assert_eq!(mgr.active().unwrap().name, "Work");
        mgr.rename(work, "Office").unwrap();
        assert_eq!(mgr.active().unwrap().name, "Office");
        let _ = mgr.open_active_store().unwrap();
    }
}
