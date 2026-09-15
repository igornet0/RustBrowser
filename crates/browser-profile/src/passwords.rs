//! Credential vault: metadata in SQLite, secrets in OS keyring (or encrypted file fallback).
//!
//! After migration, SQLite must never contain plaintext passwords.

use crate::migrations;
use browser_core::{BrowserError, BrowserResult};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use url::Url;

pub type CredentialId = i64;

#[derive(Debug, Clone)]
pub struct Credential {
    pub id: CredentialId,
    pub origin: String,
    pub username: String,
    pub password: String,
    pub note: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Pluggable secret backend — production uses OS keyring.
pub trait SecretBackend: Send + Sync {
    fn set_secret(&self, account: &str, secret: &str) -> BrowserResult<()>;
    fn get_secret(&self, account: &str) -> BrowserResult<Option<String>>;
    fn delete_secret(&self, account: &str) -> BrowserResult<()>;
}

/// macOS Keychain / Windows Credential Manager / Linux Secret Service via `keyring`.
pub struct KeyringSecretBackend {
    service: String,
}

impl KeyringSecretBackend {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl SecretBackend for KeyringSecretBackend {
    fn set_secret(&self, account: &str, secret: &str) -> BrowserResult<()> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| BrowserError::Storage(format!("keyring: {e}")))?;
        entry
            .set_password(secret)
            .map_err(|e| BrowserError::Storage(format!("keyring set: {e}")))
    }

    fn get_secret(&self, account: &str) -> BrowserResult<Option<String>> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| BrowserError::Storage(format!("keyring: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(BrowserError::Storage(format!("keyring get: {e}"))),
        }
    }

    fn delete_secret(&self, account: &str) -> BrowserResult<()> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| BrowserError::Storage(format!("keyring: {e}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(BrowserError::Storage(format!("keyring delete: {e}"))),
        }
    }
}

/// AES-GCM encrypted file store — fallback when OS keyring is unavailable (CI/dev).
/// Not equivalent to Keychain; documented as weaker fallback only.
pub struct EncryptedFileSecretBackend {
    path: PathBuf,
    key: [u8; 32],
}

impl EncryptedFileSecretBackend {
    pub fn open(path: &Path) -> BrowserResult<Self> {
        use rand::RngCore;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let key_path = path.with_extension("key");
        let key = if key_path.exists() {
            let bytes = std::fs::read(&key_path)?;
            if bytes.len() != 32 {
                return Err(BrowserError::Storage(
                    "credentials.key must be 32 bytes".into(),
                ));
            }
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            k
        } else {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            std::fs::write(&key_path, k)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
            k
        };

        if !path.exists() {
            let empty: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            Self::write_map(path, &key, &empty)?;
        }

        Ok(Self {
            path: path.to_path_buf(),
            key,
        })
    }

    fn read_map(&self) -> BrowserResult<serde_json::Map<String, serde_json::Value>> {
        Self::read_map_at(&self.path, &self.key)
    }

    fn read_map_at(
        path: &Path,
        key: &[u8; 32],
    ) -> BrowserResult<serde_json::Map<String, serde_json::Value>> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Key, Nonce};

        let raw = std::fs::read(path)?;
        if raw.len() < 12 {
            return Ok(serde_json::Map::new());
        }
        let (nonce_bytes, ct) = raw.split_at(12);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let nonce = Nonce::from_slice(nonce_bytes);
        let plain = cipher
            .decrypt(nonce, ct)
            .map_err(|e| BrowserError::Storage(format!("decrypt credentials: {e}")))?;
        let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&plain)
            .map_err(|e| BrowserError::Storage(e.to_string()))?;
        Ok(map)
    }

    fn write_map(
        path: &Path,
        key: &[u8; 32],
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> BrowserResult<()> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Key, Nonce};
        use rand::RngCore;

        let plain =
            serde_json::to_vec(map).map_err(|e| BrowserError::Storage(e.to_string()))?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(nonce, plain.as_ref())
            .map_err(|e| BrowserError::Storage(format!("encrypt credentials: {e}")))?;
        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        let tmp = path.with_extension("vault.tmp");
        std::fs::write(&tmp, &out)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

impl SecretBackend for EncryptedFileSecretBackend {
    fn set_secret(&self, account: &str, secret: &str) -> BrowserResult<()> {
        let mut map = self.read_map()?;
        map.insert(
            account.to_string(),
            serde_json::Value::String(secret.to_string()),
        );
        Self::write_map(&self.path, &self.key, &map)
    }

    fn get_secret(&self, account: &str) -> BrowserResult<Option<String>> {
        let map = self.read_map()?;
        Ok(map.get(account).and_then(|v| v.as_str().map(str::to_string)))
    }

    fn delete_secret(&self, account: &str) -> BrowserResult<()> {
        let mut map = self.read_map()?;
        map.remove(account);
        Self::write_map(&self.path, &self.key, &map)
    }
}

/// In-memory backend for unit tests.
#[derive(Default)]
pub struct MemorySecretBackend {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl SecretBackend for MemorySecretBackend {
    fn set_secret(&self, account: &str, secret: &str) -> BrowserResult<()> {
        self.map
            .lock()
            .map_err(|e| BrowserError::Storage(e.to_string()))?
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get_secret(&self, account: &str) -> BrowserResult<Option<String>> {
        Ok(self
            .map
            .lock()
            .map_err(|e| BrowserError::Storage(e.to_string()))?
            .get(account)
            .cloned())
    }

    fn delete_secret(&self, account: &str) -> BrowserResult<()> {
        self.map
            .lock()
            .map_err(|e| BrowserError::Storage(e.to_string()))?
            .remove(account);
        Ok(())
    }
}

pub struct CredentialStore {
    conn: Connection,
    secrets: Box<dyn SecretBackend>,
    profile_tag: String,
}

impl CredentialStore {
    pub fn open(path: &Path) -> BrowserResult<Self> {
        let secrets: Box<dyn SecretBackend> = {
            let kr = KeyringSecretBackend::new("rust-browser");
            match kr.set_secret("__rust_browser_probe__", "1") {
                Ok(()) => {
                    let _ = kr.delete_secret("__rust_browser_probe__");
                    info!("credential secrets: OS keyring");
                    Box::new(kr)
                }
                Err(err) => {
                    warn!(?err, "keyring unavailable — using encrypted file fallback");
                    let vault = path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join("credentials.vault");
                    Box::new(EncryptedFileSecretBackend::open(&vault)?)
                }
            }
        };
        Self::open_with_backend(path, secrets, "default")
    }

    pub fn open_with_backend(
        path: &Path,
        secrets: Box<dyn SecretBackend>,
        profile_tag: impl Into<String>,
    ) -> BrowserResult<Self> {
        let conn = Connection::open(path).map_err(|e| BrowserError::database(e.to_string()))?;
        migrations::migrate(
            &conn,
            &[
                // v1 — legacy plaintext (migration source)
                "CREATE TABLE IF NOT EXISTS credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    origin TEXT NOT NULL,
                    username TEXT NOT NULL,
                    password TEXT NOT NULL,
                    note TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_credentials_origin ON credentials(origin);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_credentials_origin_user
                    ON credentials(origin, username);",
                // v2 — secret_ref; password column retained empty until wiped
                "ALTER TABLE credentials ADD COLUMN secret_ref TEXT NOT NULL DEFAULT '';",
            ],
        )?;
        let store = Self {
            conn,
            secrets,
            profile_tag: profile_tag.into(),
        };
        store.migrate_plaintext()?;
        Ok(store)
    }

    fn account_key(&self, origin: &str, username: &str) -> String {
        format!("{}|{origin}|{username}", self.profile_tag)
    }

    /// Move legacy plaintext passwords into the secret backend and wipe the column.
    fn migrate_plaintext(&self) -> BrowserResult<()> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, origin, username, password FROM credentials
                 WHERE password IS NOT NULL AND password != ''",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows: Vec<(i64, String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| BrowserError::database(e.to_string()))?
            .collect::<Result<_, _>>()
            .map_err(|e| BrowserError::database(e.to_string()))?;

        for (id, origin, username, password) in rows {
            let account = self.account_key(&origin, &username);
            self.secrets.set_secret(&account, &password)?;
            self.conn
                .execute(
                    "UPDATE credentials SET password = '', secret_ref = ?1 WHERE id = ?2",
                    params![account, id],
                )
                .map_err(|e| BrowserError::database(e.to_string()))?;
            info!(%origin, %username, "migrated plaintext credential to secure storage");
        }
        Ok(())
    }

    pub fn upsert(
        &self,
        origin: &str,
        username: &str,
        password: &str,
        note: &str,
    ) -> BrowserResult<CredentialId> {
        let now = Utc::now().to_rfc3339();
        let account = self.account_key(origin, username);
        self.secrets.set_secret(&account, password)?;
        self.conn
            .execute(
                "INSERT INTO credentials (origin, username, password, note, created_at, updated_at, secret_ref)
                 VALUES (?1, ?2, '', ?3, ?4, ?4, ?5)
                 ON CONFLICT(origin, username) DO UPDATE SET
                    password = '',
                    note = excluded.note,
                    updated_at = excluded.updated_at,
                    secret_ref = excluded.secret_ref",
                params![origin, username, note, now, account],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        self.conn
            .query_row(
                "SELECT id FROM credentials WHERE origin = ?1 AND username = ?2",
                params![origin, username],
                |r| r.get(0),
            )
            .map_err(|e| BrowserError::database(e.to_string()))
    }

    pub fn list(&self) -> BrowserResult<Vec<Credential>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, origin, username, note, created_at, updated_at, secret_ref
                 FROM credentials ORDER BY origin, username",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| BrowserError::database(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (id, origin, username, note, created, updated, secret_ref) =
                row.map_err(|e| BrowserError::database(e.to_string()))?;
            let account = if secret_ref.is_empty() {
                self.account_key(&origin, &username)
            } else {
                secret_ref
            };
            let password = self.secrets.get_secret(&account)?.unwrap_or_default();
            out.push(Credential {
                id,
                origin,
                username,
                password,
                note,
                created_at: parse_ts(&created),
                updated_at: parse_ts(&updated),
            });
        }
        Ok(out)
    }

    pub fn get(&self, origin: &str) -> BrowserResult<Option<Credential>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM credentials WHERE origin = ?1 LIMIT 1",
                params![origin],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| BrowserError::database(e.to_string()))?;
        if id.is_none() {
            return Ok(None);
        }
        Ok(self.list()?.into_iter().find(|c| c.origin == origin))
    }

    pub fn remove(&self, id: CredentialId) -> BrowserResult<()> {
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT origin, username, secret_ref FROM credentials WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| BrowserError::database(e.to_string()))?;
        if let Some((origin, username, secret_ref)) = row {
            let account = if secret_ref.is_empty() {
                self.account_key(&origin, &username)
            } else {
                secret_ref
            };
            let _ = self.secrets.delete_secret(&account);
        }
        self.conn
            .execute("DELETE FROM credentials WHERE id = ?1", [id])
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn count(&self) -> BrowserResult<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM credentials", [], |r| r.get(0))
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(n as usize)
    }

    /// Test helper: assert no non-empty plaintext passwords remain.
    pub fn plaintext_password_count(&self) -> BrowserResult<usize> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM credentials WHERE password IS NOT NULL AND password != ''",
                [],
                |r| r.get(0),
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(n as usize)
    }
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Normalize a login URL to an origin string for storage.
pub fn credential_origin(url_or_host: &str) -> String {
    if let Ok(url) = Url::parse(url_or_host) {
        if let Some(host) = url.host_str() {
            let scheme = url.scheme();
            return format!("{scheme}://{host}");
        }
    }
    url_or_host.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_and_list_with_memory_backend() {
        let dir = tempdir().unwrap();
        let store = CredentialStore::open_with_backend(
            &dir.path().join("c.db"),
            Box::new(MemorySecretBackend::default()),
            "test",
        )
        .unwrap();
        store
            .upsert("https://example.com", "user", "secret", "imported")
            .unwrap();
        store
            .upsert("https://example.com", "user", "secret2", "updated")
            .unwrap();
        let items = store.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].password, "secret2");
        assert_eq!(store.plaintext_password_count().unwrap(), 0);
    }

    #[test]
    fn migrates_legacy_plaintext() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("c.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
                 INSERT INTO schema_migrations VALUES (1, datetime('now'));
                 CREATE TABLE credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    origin TEXT NOT NULL,
                    username TEXT NOT NULL,
                    password TEXT NOT NULL,
                    note TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 CREATE UNIQUE INDEX idx_credentials_origin_user ON credentials(origin, username);
                 INSERT INTO credentials (origin, username, password, note, created_at, updated_at)
                 VALUES ('https://legacy.test', 'u', 'pwned', '', datetime('now'), datetime('now'));",
            )
            .unwrap();
        }
        let store = CredentialStore::open_with_backend(
            &path,
            Box::new(MemorySecretBackend::default()),
            "mig",
        )
        .unwrap();
        let items = store.list().unwrap();
        assert_eq!(items[0].password, "pwned");
        assert_eq!(store.plaintext_password_count().unwrap(), 0);
    }
}
