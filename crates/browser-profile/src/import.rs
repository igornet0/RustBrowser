//! Import bookmarks, passwords, and open tabs from common browser export formats.
//!
//! Supported inputs (auto-detected by extension / content):
//! - Netscape Bookmark HTML (`bookmarks.html` from Chrome / Firefox / Safari / Edge)
//! - Password CSV (Chrome / Edge / Firefox / Bitwarden-compatible columns)
//! - Tabs / session JSON (`[{ "url", "title" }]` or `{ "tabs": [...] }`)

use crate::bookmarks::BookmarkStore;
use crate::passwords::{credential_origin, CredentialStore};
use browser_core::{BrowserError, BrowserResult};
use serde::Deserialize;
use std::path::Path;
use tracing::info;
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub bookmarks: usize,
    pub passwords: usize,
    pub tabs: usize,
    pub skipped: usize,
    pub notes: Vec<String>,
}

impl ImportSummary {
    pub fn describe(&self) -> String {
        format!(
            "Imported {} bookmarks, {} passwords, {} tabs ({} skipped)",
            self.bookmarks, self.passwords, self.tabs, self.skipped
        )
    }
}

#[derive(Debug, Clone)]
pub struct ImportedTab {
    pub url: Url,
    pub title: String,
}

/// Import whatever the path points to (file or directory of export files).
pub fn import_path(
    path: &Path,
    bookmarks: &BookmarkStore,
    credentials: &CredentialStore,
) -> BrowserResult<(ImportSummary, Vec<ImportedTab>)> {
    let mut summary = ImportSummary::default();
    let mut tabs = Vec::new();

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let child = entry.path();
            if child.is_file() {
                let (part, more_tabs) = import_file(&child, bookmarks, credentials)?;
                merge_summary(&mut summary, part);
                tabs.extend(more_tabs);
            }
        }
    } else {
        let (part, more_tabs) = import_file(path, bookmarks, credentials)?;
        merge_summary(&mut summary, part);
        tabs.extend(more_tabs);
    }

    info!(
        bookmarks = summary.bookmarks,
        passwords = summary.passwords,
        tabs = summary.tabs,
        "browser data import finished"
    );
    Ok((summary, tabs))
}

fn merge_summary(into: &mut ImportSummary, part: ImportSummary) {
    into.bookmarks += part.bookmarks;
    into.passwords += part.passwords;
    into.tabs += part.tabs;
    into.skipped += part.skipped;
    into.notes.extend(part.notes);
}

fn import_file(
    path: &Path,
    bookmarks: &BookmarkStore,
    credentials: &CredentialStore,
) -> BrowserResult<(ImportSummary, Vec<ImportedTab>)> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let data = std::fs::read_to_string(path)?;

    if name.ends_with(".html") || name.ends_with(".htm") || looks_like_netscape_html(&data) {
        let mut summary = ImportSummary::default();
        summary.bookmarks = import_bookmarks_html(&data, bookmarks)?;
        return Ok((summary, Vec::new()));
    }

    if name.ends_with(".csv") || looks_like_password_csv(&data) {
        let mut summary = ImportSummary::default();
        let (n, skipped) = import_passwords_csv(&data, credentials)?;
        summary.passwords = n;
        summary.skipped = skipped;
        return Ok((summary, Vec::new()));
    }

    if name.ends_with(".json") {
        let mut summary = ImportSummary::default();
        let tabs = import_tabs_json(&data)?;
        summary.tabs = tabs.len();
        return Ok((summary, tabs));
    }

    Err(BrowserError::Storage(format!(
        "unsupported import file: {} (use .html bookmarks, .csv passwords, or .json tabs)",
        path.display()
    )))
}

fn looks_like_netscape_html(data: &str) -> bool {
    let lower = data.to_ascii_lowercase();
    lower.contains("<!doctype netscape")
        || lower.contains("netscape-bookmark-file")
        || (lower.contains("<a href=") && lower.contains("<dl>"))
}

fn looks_like_password_csv(data: &str) -> bool {
    let first = data.lines().next().unwrap_or("").to_ascii_lowercase();
    (first.contains("url") || first.contains("origin") || first.contains("login_uri"))
        && (first.contains("username") || first.contains("login") || first.contains("password"))
}

/// Parse Netscape Bookmark File Format and insert into the store.
pub fn import_bookmarks_html(html: &str, store: &BookmarkStore) -> BrowserResult<usize> {
    let bar_id = store.bookmarks_bar_folder_id().ok();
    let mut count = 0usize;
    let mut on_bar = false;

    for raw in html.lines() {
        let line = raw.trim();
        let lower = line.to_ascii_lowercase();
        if lower.contains("bookmarks bar") || lower.contains("bookmarks toolbar") {
            on_bar = true;
        }
        if !lower.contains("<a ") || !lower.contains("href=") {
            continue;
        }
        let Some(href) = extract_attr(line, "href") else {
            continue;
        };
        let Ok(url) = Url::parse(&href) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https" | "file") {
            continue;
        }
        let title = extract_link_text(line).unwrap_or_else(|| href.clone());
        store.add(&url, &title, bar_id, on_bar)?;
        count += 1;
    }
    Ok(count)
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{name}=");
    let idx = lower.find(&key)?;
    let rest = &tag[idx + key.len()..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(rest[1..end].to_string())
}

fn extract_link_text(tag: &str) -> Option<String> {
    let start = tag.find('>')? + 1;
    let end = tag[start..].find("</").map(|i| start + i)?;
    let text = tag[start..end].trim();
    if text.is_empty() {
        None
    } else {
        Some(html_unescape(text))
    }
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Chrome/Edge CSV: `name,url,username,password`
/// Firefox: `url,username,password,httpRealm,formActionOrigin,guid,timeCreated,...`
/// Bitwarden: `login_uri,login_username,login_password,...`
pub fn import_passwords_csv(
    csv: &str,
    store: &CredentialStore,
) -> BrowserResult<(usize, usize)> {
    let mut lines = csv.lines();
    let header = lines
        .next()
        .ok_or_else(|| BrowserError::Storage("empty password CSV".into()))?;
    let cols: Vec<String> = split_csv_line(header)
        .into_iter()
        .map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
        .collect();

    let url_i = find_col(
        &cols,
        &["url", "origin", "login_uri", "hostname", "login uri"],
    );
    let user_i = find_col(
        &cols,
        &["username", "login_username", "login", "user", "login username"],
    );
    let pass_i = find_col(&cols, &["password", "login_password", "login password"]);
    let name_i = find_col(&cols, &["name", "title", "label"]);

    let (Some(url_i), Some(user_i), Some(pass_i)) = (url_i, user_i, pass_i) else {
        return Err(BrowserError::Storage(
            "CSV must include url/username/password columns (Chrome, Firefox, or Bitwarden export)"
                .into(),
        ));
    };

    let mut imported = 0usize;
    let mut skipped = 0usize;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_csv_line(line);
        let url = fields.get(url_i).map(|s| s.as_str()).unwrap_or("");
        let user = fields.get(user_i).map(|s| s.as_str()).unwrap_or("");
        let pass = fields.get(pass_i).map(|s| s.as_str()).unwrap_or("");
        if url.is_empty() || pass.is_empty() {
            skipped += 1;
            continue;
        }
        let note = name_i
            .and_then(|i| fields.get(i))
            .map(|s| s.as_str())
            .unwrap_or("imported");
        let origin = credential_origin(url);
        store.upsert(&origin, user, pass, note)?;
        imported += 1;
    }
    Ok((imported, skipped))
}

fn find_col(cols: &[String], names: &[&str]) -> Option<usize> {
    cols.iter().position(|c| names.iter().any(|n| c == n))
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

#[derive(Debug, Deserialize)]
struct TabsFile {
    #[serde(default)]
    tabs: Vec<TabJson>,
}

#[derive(Debug, Deserialize)]
struct TabJson {
    url: String,
    #[serde(default)]
    title: String,
}

pub fn import_tabs_json(data: &str) -> BrowserResult<Vec<ImportedTab>> {
    // Try `{ "tabs": [...] }` then bare array.
    if let Ok(file) = serde_json::from_str::<TabsFile>(data) {
        if !file.tabs.is_empty() {
            return Ok(parse_tab_list(file.tabs));
        }
    }
    let list: Vec<TabJson> = serde_json::from_str(data)
        .map_err(|e| BrowserError::Storage(format!("invalid tabs JSON: {e}")))?;
    Ok(parse_tab_list(list))
}

fn parse_tab_list(list: Vec<TabJson>) -> Vec<ImportedTab> {
    list.into_iter()
        .filter_map(|t| {
            let url = Url::parse(&t.url).ok()?;
            let title = if t.title.is_empty() {
                url.host_str().unwrap_or("Tab").to_string()
            } else {
                t.title
            };
            Some(ImportedTab { url, title })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passwords::CredentialStore;
    use tempfile::tempdir;

    #[test]
    fn import_netscape_bookmarks() {
        let dir = tempdir().unwrap();
        let store = BookmarkStore::open(&dir.path().join("b.db")).unwrap();
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><H3>Bookmarks Bar</H3>
    <DL><p>
        <DT><A HREF="https://example.com/">Example</A>
        <DT><A HREF="https://rust-lang.org/">Rust</A>
    </DL><p>
</DL>"#;
        assert_eq!(import_bookmarks_html(html, &store).unwrap(), 2);
        assert_eq!(store.list_all().unwrap().len(), 2);
    }

    #[test]
    fn import_chrome_password_csv() {
        let dir = tempdir().unwrap();
        let store = CredentialStore::open(&dir.path().join("c.db")).unwrap();
        let csv = "name,url,username,password\nExample,https://example.com/login,alice,s3cret\n";
        let (n, skipped) = import_passwords_csv(csv, &store).unwrap();
        assert_eq!(n, 1);
        assert_eq!(skipped, 0);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn import_tabs() {
        let json = r#"[{"url":"https://example.com","title":"Ex"},{"url":"https://servo.org","title":"Servo"}]"#;
        let tabs = import_tabs_json(json).unwrap();
        assert_eq!(tabs.len(), 2);
    }
}
