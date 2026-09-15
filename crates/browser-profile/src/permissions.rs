//! Persistent permission decisions (profile-scoped).

use crate::migrations;
use browser_core::{BrowserError, BrowserResult};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Camera,
    Microphone,
    Location,
    Notifications,
    Clipboard,
    ScreenCapture,
    Other,
}

impl PermissionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::Location => "location",
            Self::Notifications => "notifications",
            Self::Clipboard => "clipboard",
            Self::ScreenCapture => "screen_capture",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "camera" => Self::Camera,
            "microphone" => Self::Microphone,
            "location" => Self::Location,
            "notifications" => Self::Notifications,
            "clipboard" => Self::Clipboard,
            "screen_capture" => Self::ScreenCapture,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

impl PermissionDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "allow" => Self::Allow,
            "deny" => Self::Deny,
            _ => Self::Ask,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionRecord {
    pub origin: String,
    pub kind: PermissionKind,
    pub decision: PermissionDecision,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct PermissionManager {
    conn: Connection,
}

impl PermissionManager {
    pub fn open(path: &Path) -> BrowserResult<Self> {
        let conn = Connection::open(path).map_err(|e| BrowserError::database(e.to_string()))?;
        migrations::migrate(
            &conn,
            &[
                "CREATE TABLE IF NOT EXISTS permissions (
                    origin TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    expires_at TEXT,
                    PRIMARY KEY (origin, kind)
                );",
            ],
        )?;
        Ok(Self { conn })
    }

    pub fn decision(
        &self,
        origin: &str,
        kind: PermissionKind,
    ) -> BrowserResult<PermissionDecision> {
        let row: Option<(String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT decision, expires_at FROM permissions WHERE origin = ?1 AND kind = ?2",
                params![origin, kind.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| BrowserError::database(e.to_string()))?;

        let Some((decision, expires)) = row else {
            return Ok(PermissionDecision::Ask);
        };
        if let Some(exp) = expires {
            if let Ok(dt) = DateTime::parse_from_rfc3339(&exp) {
                if dt.with_timezone(&Utc) < Utc::now() {
                    return Ok(PermissionDecision::Ask);
                }
            }
        }
        Ok(PermissionDecision::parse(&decision))
    }

    pub fn set(
        &self,
        origin: &str,
        kind: PermissionKind,
        decision: PermissionDecision,
        expires_at: Option<DateTime<Utc>>,
    ) -> BrowserResult<()> {
        let now = Utc::now().to_rfc3339();
        let exp = expires_at.map(|d| d.to_rfc3339());
        self.conn
            .execute(
                "INSERT INTO permissions (origin, kind, decision, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(origin, kind) DO UPDATE SET
                    decision = excluded.decision,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at",
                params![origin, kind.as_str(), decision.as_str(), now, exp],
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        Ok(())
    }

    pub fn list(&self) -> BrowserResult<Vec<PermissionRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT origin, kind, decision, created_at, expires_at FROM permissions
                 ORDER BY origin, kind",
            )
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| BrowserError::database(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let (origin, kind, decision, created, expires) =
                row.map_err(|e| BrowserError::database(e.to_string()))?;
            out.push(PermissionRecord {
                origin,
                kind: PermissionKind::parse(&kind),
                decision: PermissionDecision::parse(&decision),
                created_at: DateTime::parse_from_rfc3339(&created)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                expires_at: expires.and_then(|e| {
                    DateTime::parse_from_rfc3339(&e)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ask_by_default_then_persist() {
        let dir = tempdir().unwrap();
        let mgr = PermissionManager::open(&dir.path().join("p.db")).unwrap();
        assert_eq!(
            mgr.decision("https://example.com", PermissionKind::Microphone)
                .unwrap(),
            PermissionDecision::Ask
        );
        mgr.set(
            "https://example.com",
            PermissionKind::Microphone,
            PermissionDecision::Allow,
            None,
        )
        .unwrap();
        assert_eq!(
            mgr.decision("https://example.com", PermissionKind::Microphone)
                .unwrap(),
            PermissionDecision::Allow
        );
        mgr.set(
            "https://example.com",
            PermissionKind::Microphone,
            PermissionDecision::Deny,
            None,
        )
        .unwrap();
        assert_eq!(
            mgr.decision("https://example.com", PermissionKind::Microphone)
                .unwrap(),
            PermissionDecision::Deny
        );
    }
}
