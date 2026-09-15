//! Discover installed browsers and import from their on-disk profiles.

use crate::bookmarks::BookmarkStore;
use crate::import::{ImportSummary, ImportedTab};
use crate::passwords::CredentialStore;
use browser_core::{BrowserError, BrowserResult};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Chromium,
    Firefox,
    Safari,
}

#[derive(Debug, Clone)]
pub struct DetectedBrowser {
    pub id: String,
    pub browser_name: String,
    pub profile_name: String,
    pub kind: BrowserKind,
    pub path: PathBuf,
    pub has_bookmarks: bool,
    pub has_tabs: bool,
    /// Password DB exists but values are OS-encrypted — CSV export still needed.
    pub passwords_encrypted: bool,
}

impl DetectedBrowser {
    pub fn label(&self) -> String {
        if self.profile_name.is_empty() || self.profile_name == "Default" {
            self.browser_name.clone()
        } else {
            format!("{} — {}", self.browser_name, self.profile_name)
        }
    }

    pub fn capabilities_label(&self) -> String {
        let mut parts = Vec::new();
        if self.has_bookmarks {
            parts.push("bookmarks");
        }
        if self.has_tabs {
            parts.push("tabs");
        }
        if self.passwords_encrypted {
            parts.push("passwords (CSV export)");
        }
        if parts.is_empty() {
            "no data".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Scan well-known locations for browser profiles on this machine.
pub fn discover_browsers() -> Vec<DetectedBrowser> {
    let mut out = Vec::new();
    let home = directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "macos")]
    {
        let support = home.join("Library/Application Support");
        scan_chromium_family(
            &mut out,
            &support.join("Google/Chrome"),
            "Google Chrome",
        );
        scan_chromium_family(
            &mut out,
            &support.join("Google/Chrome Canary"),
            "Chrome Canary",
        );
        scan_chromium_family(
            &mut out,
            &support.join("Microsoft Edge"),
            "Microsoft Edge",
        );
        scan_chromium_family(
            &mut out,
            &support.join("BraveSoftware/Brave-Browser"),
            "Brave",
        );
        scan_chromium_family(&mut out, &support.join("Chromium"), "Chromium");
        scan_chromium_family(&mut out, &support.join("Vivaldi"), "Vivaldi");
        scan_chromium_family(
            &mut out,
            &support.join("com.operasoftware.Opera"),
            "Opera",
        );
        scan_chromium_family(
            &mut out,
            &support.join("Yandex/YandexBrowser"),
            "Yandex",
        );
        scan_chromium_family(&mut out, &support.join("Arc/User Data"), "Arc");
        scan_firefox(&mut out, &support.join("Firefox/Profiles"));
        scan_safari(&mut out, &home.join("Library/Safari"));
    }

    #[cfg(target_os = "linux")]
    {
        let config = home.join(".config");
        scan_chromium_family(&mut out, &config.join("google-chrome"), "Google Chrome");
        scan_chromium_family(&mut out, &config.join("chromium"), "Chromium");
        scan_chromium_family(&mut out, &config.join("microsoft-edge"), "Microsoft Edge");
        scan_chromium_family(
            &mut out,
            &config.join("BraveSoftware/Brave-Browser"),
            "Brave",
        );
        scan_firefox(&mut out, &home.join(".mozilla/firefox"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            scan_chromium_family(
                &mut out,
                &local.join("Google/Chrome/User Data"),
                "Google Chrome",
            );
            scan_chromium_family(
                &mut out,
                &local.join("Microsoft/Edge/User Data"),
                "Microsoft Edge",
            );
            scan_chromium_family(
                &mut out,
                &local.join("BraveSoftware/Brave-Browser/User Data"),
                "Brave",
            );
        }
        if let Ok(roaming) = std::env::var("APPDATA") {
            scan_firefox(&mut out, &PathBuf::from(roaming).join("Mozilla/Firefox/Profiles"));
        }
    }

    out.sort_by(|a, b| {
        a.browser_name
            .cmp(&b.browser_name)
            .then(a.profile_name.cmp(&b.profile_name))
    });
    info!(count = out.len(), "discovered browser profiles");
    out
}

fn scan_chromium_family(out: &mut Vec<DetectedBrowser>, root: &Path, browser_name: &str) {
    if !root.is_dir() {
        return;
    }
    let candidates = chromium_profile_dirs(root);
    for profile_dir in candidates {
        let bookmarks = profile_dir.join("Bookmarks");
        let login_data = profile_dir.join("Login Data");
        let has_bookmarks = bookmarks.is_file();
        let has_tabs = chromium_session_files(&profile_dir).next().is_some();
        let passwords_encrypted = login_data.is_file();
        if !has_bookmarks && !has_tabs && !passwords_encrypted {
            continue;
        }
        let profile_name = chromium_profile_display_name(&profile_dir);
        let id = format!(
            "chromium:{}:{}",
            browser_name,
            profile_dir.file_name().and_then(|s| s.to_str()).unwrap_or("?")
        );
        out.push(DetectedBrowser {
            id,
            browser_name: browser_name.into(),
            profile_name,
            kind: BrowserKind::Chromium,
            path: profile_dir,
            has_bookmarks,
            has_tabs,
            passwords_encrypted,
        });
    }
}

fn chromium_profile_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let default = root.join("Default");
    if default.is_dir() {
        dirs.push(default);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("Profile ") && entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
    }
    // Some installs put Bookmarks directly under root (rare).
    if dirs.is_empty() && root.join("Bookmarks").is_file() {
        dirs.push(root.to_path_buf());
    }
    dirs
}

fn chromium_profile_display_name(profile_dir: &Path) -> String {
    let prefs = profile_dir.join("Preferences");
    if let Ok(data) = std::fs::read_to_string(&prefs) {
        if let Ok(v) = serde_json::from_str::<Value>(&data) {
            if let Some(name) = v
                .pointer("/profile/name")
                .and_then(|n| n.as_str())
                .filter(|s| !s.is_empty())
            {
                return name.to_string();
            }
        }
    }
    profile_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Default")
        .to_string()
}

fn chromium_session_files(profile_dir: &Path) -> impl Iterator<Item = PathBuf> {
    let sessions = profile_dir.join("Sessions");
    let mut files = Vec::new();
    for name in ["Current Session", "Last Session", "Current Tabs", "Last Tabs"] {
        let p = profile_dir.join(name);
        if p.is_file() {
            files.push(p);
        }
        let p = sessions.join(name);
        if p.is_file() {
            files.push(p);
        }
    }
    if let Ok(entries) = std::fs::read_dir(&sessions) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("Session_") || name.starts_with("Tabs_") {
                files.push(entry.path());
            }
        }
    }
    files.into_iter()
}

fn scan_firefox(out: &mut Vec<DetectedBrowser>, profiles_root: &Path) {
    if !profiles_root.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(profiles_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let places = path.join("places.sqlite");
        if !places.is_file() {
            continue;
        }
        let has_tabs = firefox_session_path(&path).is_some();
        let passwords_encrypted = path.join("logins.json").is_file();
        let profile_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("profile")
            .to_string();
        // Prefer readable name after the dot: abcd.default-release → default-release
        let nice = profile_name
            .split_once('.')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| profile_name.clone());
        out.push(DetectedBrowser {
            id: format!("firefox:{profile_name}"),
            browser_name: "Firefox".into(),
            profile_name: nice,
            kind: BrowserKind::Firefox,
            path,
            has_bookmarks: true,
            has_tabs,
            passwords_encrypted,
        });
    }
}

fn firefox_session_path(profile: &Path) -> Option<PathBuf> {
    let candidates = [
        profile.join("sessionstore-backups/recovery.jsonlz4"),
        profile.join("sessionstore-backups/previous.jsonlz4"),
        profile.join("sessionstore.jsonlz4"),
        profile.join("sessionstore.json"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn scan_safari(out: &mut Vec<DetectedBrowser>, safari_dir: &Path) {
    let bookmarks = safari_dir.join("Bookmarks.plist");
    if !bookmarks.is_file() {
        return;
    }
    out.push(DetectedBrowser {
        id: "safari:default".into(),
        browser_name: "Safari".into(),
        profile_name: "Default".into(),
        kind: BrowserKind::Safari,
        path: safari_dir.to_path_buf(),
        has_bookmarks: true,
        has_tabs: false,
        passwords_encrypted: true, // Keychain
    });
}

/// Import bookmarks (and recoverable tabs) from a detected browser profile.
pub fn import_from_detected(
    source: &DetectedBrowser,
    bookmarks: &BookmarkStore,
    _credentials: &CredentialStore,
) -> BrowserResult<(ImportSummary, Vec<ImportedTab>)> {
    let mut summary = ImportSummary::default();
    let mut tabs = Vec::new();

    match source.kind {
        BrowserKind::Chromium => {
            if source.has_bookmarks {
                match import_chromium_bookmarks(&source.path.join("Bookmarks"), bookmarks) {
                    Ok(n) => summary.bookmarks = n,
                    Err(err) => {
                        warn!(?err, "chromium bookmarks import");
                        summary.notes.push(err.to_string());
                    }
                }
            }
            if source.has_tabs {
                tabs = import_chromium_tabs(&source.path);
                summary.tabs = tabs.len();
            }
            if source.passwords_encrypted {
                summary.notes.push(
                    "Passwords in this browser are encrypted. Export CSV from the browser’s password manager, then import the file below.".into(),
                );
            }
        }
        BrowserKind::Firefox => {
            match import_firefox_bookmarks(&source.path.join("places.sqlite"), bookmarks) {
                Ok(n) => summary.bookmarks = n,
                Err(err) => {
                    warn!(?err, "firefox bookmarks import");
                    summary.notes.push(err.to_string());
                }
            }
            if let Some(session) = firefox_session_path(&source.path) {
                match import_firefox_tabs(&session) {
                    Ok(t) => {
                        summary.tabs = t.len();
                        tabs = t;
                    }
                    Err(err) => summary.notes.push(format!("tabs: {err}")),
                }
            }
            if source.passwords_encrypted {
                summary.notes.push(
                    "Firefox passwords are encrypted. Use Firefox → Logins → Export Passwords (CSV).".into(),
                );
            }
        }
        BrowserKind::Safari => {
            match import_safari_bookmarks(&source.path.join("Bookmarks.plist"), bookmarks) {
                Ok(n) => summary.bookmarks = n,
                Err(err) => {
                    warn!(?err, "safari bookmarks import");
                    summary.notes.push(err.to_string());
                }
            }
            summary.notes.push(
                "Safari passwords live in the macOS Keychain — export is not available automatically.".into(),
            );
        }
    }

    info!(
        browser = %source.browser_name,
        profile = %source.profile_name,
        bookmarks = summary.bookmarks,
        tabs = summary.tabs,
        "imported from detected browser"
    );
    Ok((summary, tabs))
}

// --- Chromium Bookmarks JSON -------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChromeBookmarksFile {
    roots: ChromeRoots,
}

#[derive(Debug, Deserialize)]
struct ChromeRoots {
    #[serde(default)]
    bookmark_bar: Option<ChromeNode>,
    #[serde(default)]
    other: Option<ChromeNode>,
    #[serde(default)]
    synced: Option<ChromeNode>,
}

#[derive(Debug, Deserialize)]
struct ChromeNode {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    children: Vec<ChromeNode>,
}

fn import_chromium_bookmarks(path: &Path, store: &BookmarkStore) -> BrowserResult<usize> {
    let data = std::fs::read_to_string(path)?;
    let file: ChromeBookmarksFile = serde_json::from_str(&data)
        .map_err(|e| BrowserError::Storage(format!("Chrome Bookmarks JSON: {e}")))?;
    let bar_id = store.bookmarks_bar_folder_id().ok();
    let mut count = 0usize;
    if let Some(bar) = file.roots.bookmark_bar {
        count += walk_chrome_node(&bar, store, bar_id, true)?;
    }
    if let Some(other) = file.roots.other {
        count += walk_chrome_node(&other, store, bar_id, false)?;
    }
    if let Some(synced) = file.roots.synced {
        count += walk_chrome_node(&synced, store, bar_id, false)?;
    }
    Ok(count)
}

fn walk_chrome_node(
    node: &ChromeNode,
    store: &BookmarkStore,
    bar_id: Option<i64>,
    on_bar: bool,
) -> BrowserResult<usize> {
    let mut count = 0usize;
    if node.kind == "url" || (!node.url.is_empty() && node.children.is_empty()) {
        if let Ok(url) = Url::parse(&node.url) {
            if matches!(url.scheme(), "http" | "https") {
                let title = if node.name.is_empty() {
                    node.url.clone()
                } else {
                    node.name.clone()
                };
                store.add(&url, &title, bar_id, on_bar)?;
                count += 1;
            }
        }
    }
    for child in &node.children {
        // Children of bookmark_bar stay on the bar; nested folders under other are not.
        count += walk_chrome_node(child, store, bar_id, on_bar)?;
    }
    Ok(count)
}

fn import_chromium_tabs(profile_dir: &Path) -> Vec<ImportedTab> {
    let mut urls = BTreeSet::new();
    for file in chromium_session_files(profile_dir) {
        if let Ok(bytes) = std::fs::read(&file) {
            for url in extract_http_urls_from_bytes(&bytes) {
                urls.insert(url);
            }
        }
    }
    urls.into_iter()
        .filter_map(|u| {
            let url = Url::parse(&u).ok()?;
            let title = url.host_str().unwrap_or("Tab").to_string();
            Some(ImportedTab { url, title })
        })
        .take(50)
        .collect()
}

fn extract_http_urls_from_bytes(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 < data.len() {
        if data[i..].starts_with(b"https://") || data[i..].starts_with(b"http://") {
            let start = i;
            i += if data[i..].starts_with(b"https://") {
                8
            } else {
                7
            };
            while i < data.len() {
                let b = data[i];
                if b.is_ascii_graphic() || b == b'%' {
                    i += 1;
                } else {
                    break;
                }
            }
            if let Ok(s) = std::str::from_utf8(&data[start..i]) {
                // Trim trailing punctuation common in binary scrapes.
                let cleaned = s.trim_end_matches(|c: char| {
                    matches!(c, '"' | '\'' | ')' | ']' | '>' | ',' | ';' | '\\')
                });
                if cleaned.len() > 12 {
                    out.push(cleaned.to_string());
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

// --- Firefox places.sqlite ---------------------------------------------------

fn import_firefox_bookmarks(places: &Path, store: &BookmarkStore) -> BrowserResult<usize> {
    // Copy DB — Firefox may lock the live file.
    let tmp = places.with_extension("import-copy.sqlite");
    std::fs::copy(places, &tmp)?;
    let conn = rusqlite::Connection::open(&tmp)
        .map_err(|e| BrowserError::database(e.to_string()))?;
    let bar_id = store.bookmarks_bar_folder_id().ok();
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(b.title, p.title, p.url), p.url
             FROM moz_bookmarks b
             JOIN moz_places p ON b.fk = p.id
             WHERE b.type = 1 AND p.url LIKE 'http%'
             ORDER BY b.id",
        )
        .map_err(|e| BrowserError::database(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| BrowserError::database(e.to_string()))?;
    let mut count = 0usize;
    for row in rows {
        let (title, url_str) = row.map_err(|e| BrowserError::database(e.to_string()))?;
        if let Ok(url) = Url::parse(&url_str) {
            store.add(&url, &title, bar_id, false)?;
            count += 1;
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(count)
}

fn import_firefox_tabs(path: &Path) -> BrowserResult<Vec<ImportedTab>> {
    let data = std::fs::read(path)?;
    let json = if path.extension().and_then(|e| e.to_str()) == Some("jsonlz4")
        || data.starts_with(b"mozLz40\0")
    {
        decompress_moz_lz4(&data)?
    } else {
        String::from_utf8(data)
            .map_err(|e| BrowserError::Storage(format!("firefox session utf8: {e}")))?
    };
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| BrowserError::Storage(format!("firefox session json: {e}")))?;
    let mut tabs = Vec::new();
    if let Some(windows) = v.get("windows").and_then(|w| w.as_array()) {
        for window in windows {
            if let Some(tab_list) = window.get("tabs").and_then(|t| t.as_array()) {
                for tab in tab_list {
                    if let Some(entries) = tab.get("entries").and_then(|e| e.as_array()) {
                        let idx = tab
                            .get("index")
                            .and_then(|i| i.as_u64())
                            .unwrap_or(1)
                            .saturating_sub(1) as usize;
                        let entry = entries.get(idx).or_else(|| entries.last());
                        if let Some(entry) = entry {
                            let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("");
                            let title = entry
                                .get("title")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Ok(parsed) = Url::parse(url) {
                                if matches!(parsed.scheme(), "http" | "https") {
                                    let title = if title.is_empty() {
                                        parsed.host_str().unwrap_or("Tab").to_string()
                                    } else {
                                        title
                                    };
                                    tabs.push(ImportedTab {
                                        url: parsed,
                                        title,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(tabs)
}

fn decompress_moz_lz4(data: &[u8]) -> BrowserResult<String> {
    const MAGIC: &[u8] = b"mozLz40\0";
    if data.len() < MAGIC.len() + 4 || !data.starts_with(MAGIC) {
        return Err(BrowserError::Storage("not a mozLz4 file".into()));
    }
    let compressed = &data[MAGIC.len()..];
    // First 4 bytes = uncompressed size (little-endian), rest = lz4 block.
    if compressed.len() < 4 {
        return Err(BrowserError::Storage("truncated mozLz4".into()));
    }
    let body = &compressed[4..];
    let decompressed = lz4_flex::decompress_size_prepended(body).or_else(|_| {
        // Some files store size separately; try raw block with size from header.
        let size = u32::from_le_bytes([compressed[0], compressed[1], compressed[2], compressed[3]])
            as usize;
        lz4_flex::decompress(body, size)
    }).map_err(|e| BrowserError::Storage(format!("lz4 decompress: {e}")))?;
    String::from_utf8(decompressed)
        .map_err(|e| BrowserError::Storage(format!("mozLz4 utf8: {e}")))
}

// --- Safari Bookmarks.plist --------------------------------------------------

fn import_safari_bookmarks(path: &Path, store: &BookmarkStore) -> BrowserResult<usize> {
    let file = std::fs::File::open(path)?;
    let value: plist::Value = plist::from_reader(file)
        .map_err(|e| BrowserError::Storage(format!("Safari Bookmarks.plist: {e}")))?;
    let bar_id = store.bookmarks_bar_folder_id().ok();
    let mut count = 0usize;
    walk_safari_plist(&value, store, bar_id, &mut count)?;
    Ok(count)
}

fn walk_safari_plist(
    value: &plist::Value,
    store: &BookmarkStore,
    bar_id: Option<i64>,
    count: &mut usize,
) -> BrowserResult<()> {
    let Some(dict) = value.as_dictionary() else {
        if let Some(arr) = value.as_array() {
            for item in arr {
                walk_safari_plist(item, store, bar_id, count)?;
            }
        }
        return Ok(());
    };

    let web_url = dict.get("URLString").and_then(|v| v.as_string());

    if let Some(url_str) = web_url {
        if let Ok(url) = Url::parse(url_str) {
            if matches!(url.scheme(), "http" | "https") {
                let title = dict
                    .get("URIDictionary")
                    .and_then(|v| v.as_dictionary())
                    .and_then(|d| d.get("title"))
                    .and_then(|v| v.as_string())
                    .or_else(|| dict.get("Title").and_then(|v| v.as_string()))
                    .unwrap_or(url_str)
                    .to_string();
                store.add(&url, &title, bar_id, false)?;
                *count += 1;
            }
        }
    }

    if let Some(children) = dict.get("Children").and_then(|v| v.as_array()) {
        for child in children {
            walk_safari_plist(child, store, bar_id, count)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extract_urls_from_session_blob() {
        let blob = b"xxxxhttps://example.com/path\0yyyhttp://servo.org\0";
        let urls = extract_http_urls_from_bytes(blob);
        assert!(urls.iter().any(|u| u.contains("example.com")));
        assert!(urls.iter().any(|u| u.contains("servo.org")));
    }

    #[test]
    fn chrome_bookmarks_json() {
        let dir = tempdir().unwrap();
        let store = BookmarkStore::open(&dir.path().join("b.db")).unwrap();
        let path = dir.path().join("Bookmarks");
        std::fs::write(
            &path,
            r#"{
              "roots": {
                "bookmark_bar": {
                  "children": [
                    {"type":"url","name":"Ex","url":"https://example.com/"},
                    {"type":"folder","name":"F","children":[
                      {"type":"url","name":"Rust","url":"https://rust-lang.org/"}
                    ]}
                  ],
                  "type":"folder"
                },
                "other": {"children":[],"type":"folder"},
                "synced": {"children":[],"type":"folder"}
              }
            }"#,
        )
        .unwrap();
        assert_eq!(import_chromium_bookmarks(&path, &store).unwrap(), 2);
    }
}
