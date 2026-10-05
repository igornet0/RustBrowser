use browser_core::{BrowserError, BrowserResult};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;

/// Domain → IP (optional port) override used for local / LAN development hosts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalHostEntry {
    pub domain: String,
    pub ip: String,
    /// When set, navigation to this domain without an explicit port uses this port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl LocalHostEntry {
    pub fn new(domain: impl Into<String>, ip: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            ip: ip.into(),
            port: None,
        }
    }

    /// Parse `domain` + target (`192.168.0.1` or `192.168.0.1:3000` / `[::1]:3000`).
    pub fn from_target(domain: impl Into<String>, target: &str) -> Result<Self, String> {
        let (ip, port) = parse_ip_and_port(target)?;
        Self {
            domain: domain.into(),
            ip,
            port,
        }
        .validated()
    }

    /// Normalize and validate domain + IP (+ optional port).
    pub fn validated(&self) -> Result<Self, String> {
        let domain = normalize_domain(&self.domain)?;
        // Allow callers that already split ip/port, or a combined `ip` field.
        let (ip, parsed_port) = if self.port.is_some() {
            (normalize_ip(&self.ip)?, None)
        } else {
            parse_ip_and_port(&self.ip)?
        };
        let port = match self.port.or(parsed_port) {
            Some(0) => return Err("port must be 1–65535".into()),
            other => other,
        };
        Ok(Self { domain, ip, port })
    }

    pub fn target_display(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{}", self.ip, port),
            None => self.ip.clone(),
        }
    }
}

fn normalize_domain(raw: &str) -> Result<String, String> {
    let d = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return Err("domain is empty".into());
    }
    if d.len() > 253 {
        return Err("domain is too long".into());
    }
    if d.contains('/') || d.contains(':') || d.contains(' ') {
        return Err("domain must be a hostname without scheme or path".into());
    }
    if IpAddr::from_str(&d).is_ok() {
        return Err("domain must not be an IP address".into());
    }
    for label in d.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("invalid domain label".into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("domain labels cannot start or end with '-'".into());
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("domain contains invalid characters".into());
        }
    }
    Ok(d)
}

fn normalize_ip(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("IP is empty".into());
    }
    IpAddr::from_str(trimmed)
        .map(|ip| ip.to_string())
        .map_err(|_| format!("invalid IP address: {trimmed}"))
}

/// Accept `192.168.0.1`, `192.168.0.1:3000`, or `[::1]:3000`.
fn parse_ip_and_port(raw: &str) -> Result<(String, Option<u16>), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("IP is empty".into());
    }
    if let Ok(ip) = IpAddr::from_str(trimmed) {
        return Ok((ip.to_string(), None));
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        let Some((addr, port_part)) = rest.split_once("]:") else {
            return Err(format!("invalid IP address: {trimmed}"));
        };
        let ip = IpAddr::from_str(addr).map_err(|_| format!("invalid IP address: {addr}"))?;
        let port: u16 = port_part
            .parse()
            .map_err(|_| format!("invalid port: {port_part}"))?;
        if port == 0 {
            return Err("port must be 1–65535".into());
        }
        return Ok((ip.to_string(), Some(port)));
    }
    if let Some((host, port_s)) = trimmed.rsplit_once(':') {
        // Bare IPv6 has multiple colons and no brackets — reject ambiguous forms.
        if !host.contains(':') {
            let ip = IpAddr::from_str(host).map_err(|_| format!("invalid IP address: {host}"))?;
            let port: u16 = port_s
                .parse()
                .map_err(|_| format!("invalid port: {port_s}"))?;
            if port == 0 {
                return Err("port must be 1–65535".into());
            }
            return Ok((ip.to_string(), Some(port)));
        }
    }
    Err(format!("invalid IP or IP:port: {trimmed}"))
}

/// If `url` host matches a local mapping with a port and the URL has no explicit
/// port, set that port. When the user typed a bare host (no scheme), prefer `http`
/// for non-443 ports so local HTTP servers work without a TLS cert.
pub fn apply_local_host_overrides(
    url: &mut Url,
    entries: &[LocalHostEntry],
    bare_host_input: bool,
) {
    let Some(host) = url.host_str().map(|h| h.to_string()) else {
        return;
    };
    let Some(entry) = entries
        .iter()
        .find(|e| e.domain.eq_ignore_ascii_case(&host))
    else {
        return;
    };
    let Ok(entry) = entry.validated() else {
        return;
    };
    let Some(port) = entry.port else {
        return;
    };
    if url.port().is_none() {
        let _ = url.set_port(Some(port));
    }
    if bare_host_input && url.scheme() == "https" && port != 443 {
        let _ = url.set_scheme("http");
    }
}

/// Validate entries and write a Servo-compatible hosts file (`IP hostname` lines).
/// Empty / all-invalid lists remove the file so Servo uses system DNS.
pub fn sync_local_hosts_file(path: &Path, entries: &[LocalHostEntry]) -> BrowserResult<usize> {
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        match entry.validated() {
            Ok(v) => {
                if seen.insert(v.domain.clone()) {
                    lines.push(format!("{} {}", v.ip, v.domain));
                }
            }
            Err(reason) => {
                tracing::warn!(domain = %entry.domain, ip = %entry.ip, %reason, "skipping invalid local host");
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if lines.is_empty() {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        return Ok(0);
    }

    let body = format!(
        "# Generated by RustBrowser — do not edit by hand\n{}\n",
        lines.join("\n")
    );
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(lines.len())
}

/// Format hosts-file body for tests / preview (validated entries only).
pub fn format_hosts_file(entries: &[LocalHostEntry]) -> String {
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if let Ok(v) = entry.validated() {
            if seen.insert(v.domain.clone()) {
                lines.push(format!("{} {}", v.ip, v.domain));
            }
        }
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Self::System
    }
}

impl Theme {
    pub fn label_key(&self) -> &'static str {
        match self {
            Self::System => "theme_system",
            Self::Light => "theme_light",
            Self::Dark => "theme_dark",
        }
    }
}

/// Accent / chrome color palette (independent of light/dark mode).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    Violet,
    Ocean,
    Forest,
    Sunset,
    Midnight,
    Sand,
}

impl ColorScheme {
    pub const ALL: [Self; 6] = [
        Self::Violet,
        Self::Ocean,
        Self::Forest,
        Self::Sunset,
        Self::Midnight,
        Self::Sand,
    ];

    pub fn label_key(&self) -> &'static str {
        match self {
            Self::Violet => "color_violet",
            Self::Ocean => "color_ocean",
            Self::Forest => "color_forest",
            Self::Sunset => "color_sunset",
            Self::Midnight => "color_midnight",
            Self::Sand => "color_sand",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    #[default]
    English,
    Russian,
    Ukrainian,
    German,
    Spanish,
    French,
    Chinese,
}

impl UiLanguage {
    pub const ALL: [Self; 7] = [
        Self::English,
        Self::Russian,
        Self::Ukrainian,
        Self::German,
        Self::Spanish,
        Self::French,
        Self::Chinese,
    ];

    pub fn code(&self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
            Self::Ukrainian => "uk",
            Self::German => "de",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::Chinese => "zh",
        }
    }

    /// BCP-47 locale for HTTP `Accept-Language` / Servo `intl_locale_override`.
    pub fn locale_tag(&self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::Russian => "ru-RU",
            Self::Ukrainian => "uk-UA",
            Self::German => "de-DE",
            Self::Spanish => "es-ES",
            Self::French => "fr-FR",
            Self::Chinese => "zh-CN",
        }
    }

    pub fn native_name(&self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
            Self::Ukrainian => "Українська",
            Self::German => "Deutsch",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::Chinese => "中文",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupBehavior {
    Homepage,
    RestorePreviousSession,
    AskToRestore,
}

impl Default for StartupBehavior {
    fn default() -> Self {
        Self::AskToRestore
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub homepage: String,
    pub search_engine: String,
    pub startup_behavior: StartupBehavior,
    pub theme: Theme,
    #[serde(default)]
    pub color_scheme: ColorScheme,
    #[serde(default)]
    pub language: UiLanguage,
    pub default_zoom: f32,
    pub download_directory: PathBuf,
    pub restore_previous_session: bool,
    /// Optional wallpaper for the new-tab / home page (absolute or profile-relative).
    #[serde(default)]
    pub home_background: Option<PathBuf>,
    /// Preferred network route for navigations.
    #[serde(default)]
    pub network_mode: NetworkMode,
    /// HTTP(S) proxy URI when `network_mode == Proxy` (e.g. `http://127.0.0.1:8080`).
    #[serde(default)]
    pub proxy_uri: String,
    #[serde(default)]
    pub proxy_no_proxy: String,
    /// Domain → local IP overrides (address bar keeps the domain; Servo connects to IP).
    #[serde(default)]
    pub local_hosts: Vec<LocalHostEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Direct,
    Proxy,
    /// Requires a real VPN backend; otherwise navigation is Unavailable.
    Vpn,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            homepage: "about:newtab".into(),
            search_engine: "https://duckduckgo.com/?q=%s".into(),
            startup_behavior: StartupBehavior::AskToRestore,
            theme: Theme::System,
            color_scheme: ColorScheme::Violet,
            language: UiLanguage::English,
            default_zoom: 1.0,
            download_directory: PathBuf::from("downloads"),
            restore_previous_session: true,
            home_background: None,
            network_mode: NetworkMode::Direct,
            proxy_uri: String::new(),
            proxy_no_proxy: String::new(),
            local_hosts: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load_or_default(path: &Path) -> BrowserResult<Self> {
        if !path.exists() {
            let settings = Self::default();
            settings.save(path)?;
            return Ok(settings);
        }
        let data = std::fs::read_to_string(path)?;
        serde_json::from_str(&data).map_err(|e| BrowserError::Storage(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> BrowserResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| BrowserError::Storage(e.to_string()))?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn homepage_url(&self) -> BrowserResult<Url> {
        Url::parse(&self.homepage).map_err(|e| BrowserError::InvalidUrl(e.to_string()))
    }

    pub fn is_internal_homepage(&self) -> bool {
        matches!(
            self.homepage.as_str(),
            "about:newtab" | "about:home" | "about:blank" | ""
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn persist_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let mut s = Settings::default();
        s.homepage = "https://example.org".into();
        s.color_scheme = ColorScheme::Ocean;
        s.language = UiLanguage::Russian;
        s.save(&path).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded.homepage, "https://example.org");
        assert_eq!(loaded.color_scheme, ColorScheme::Ocean);
        assert_eq!(loaded.language, UiLanguage::Russian);
    }

    #[test]
    fn loads_legacy_without_new_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(
            &path,
            r#"{
              "homepage": "https://example.com",
              "search_engine": "https://duckduckgo.com/?q=%s",
              "startup_behavior": "ask_to_restore",
              "theme": "dark",
              "default_zoom": 1.0,
              "download_directory": "downloads",
              "restore_previous_session": true
            }"#,
        )
        .unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded.theme, Theme::Dark);
        assert_eq!(loaded.color_scheme, ColorScheme::Violet);
        assert_eq!(loaded.language, UiLanguage::English);
        assert!(loaded.local_hosts.is_empty());
    }

    #[test]
    fn local_hosts_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let mut s = Settings::default();
        s.local_hosts = vec![
            LocalHostEntry::new("App.Local", "192.168.1.10"),
            LocalHostEntry::new("api.dev", "127.0.0.1"),
        ];
        s.save(&path).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded.local_hosts.len(), 2);
        assert_eq!(loaded.local_hosts[0].domain, "App.Local");
        assert_eq!(loaded.local_hosts[1].ip, "127.0.0.1");
    }

    #[test]
    fn validates_and_formats_hosts_file() {
        let entries = vec![
            LocalHostEntry::new("App.Local.", "192.168.1.10"),
            LocalHostEntry::new("bad domain", "1.2.3.4"),
            LocalHostEntry::new("api.dev", "not-an-ip"),
            LocalHostEntry::new("api.dev", "127.0.0.1"),
            LocalHostEntry::new("dup.local", "10.0.0.1"),
            LocalHostEntry::new("DUP.LOCAL", "10.0.0.2"),
        ];
        let body = format_hosts_file(&entries);
        assert_eq!(body, "192.168.1.10 app.local\n127.0.0.1 api.dev\n10.0.0.1 dup.local");
        assert!(LocalHostEntry::new("", "1.2.3.4").validated().is_err());
        assert!(LocalHostEntry::new("ok.local", "999.0.0.1")
            .validated()
            .is_err());
    }

    #[test]
    fn sync_writes_and_removes_hosts_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("local_hosts");
        let n = sync_local_hosts_file(
            &path,
            &[LocalHostEntry::new("app.local", "192.168.0.5")],
        )
        .unwrap();
        assert_eq!(n, 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("192.168.0.5 app.local"));
        let n = sync_local_hosts_file(&path, &[]).unwrap();
        assert_eq!(n, 0);
        assert!(!path.exists());
    }

    #[test]
    fn parses_ip_with_port() {
        let e = LocalHostEntry::from_target("app.local", "192.168.0.1:3000").unwrap();
        assert_eq!(e.ip, "192.168.0.1");
        assert_eq!(e.port, Some(3000));
        assert_eq!(e.target_display(), "192.168.0.1:3000");
        // Hosts file stores IP only.
        assert_eq!(
            format_hosts_file(&[e.clone()]),
            "192.168.0.1 app.local"
        );
        let e6 = LocalHostEntry::from_target("v6.local", "[::1]:8080").unwrap();
        assert_eq!(e6.ip, "::1");
        assert_eq!(e6.port, Some(8080));
    }

    #[test]
    fn apply_port_on_bare_host_navigation() {
        let entries = vec![LocalHostEntry::from_target("app.local", "192.168.0.1:3000").unwrap()];
        let mut url = Url::parse("https://app.local/").unwrap();
        apply_local_host_overrides(&mut url, &entries, true);
        assert_eq!(url.as_str(), "http://app.local:3000/");
        // Explicit port in the address wins.
        let mut url = Url::parse("http://app.local:9000/").unwrap();
        apply_local_host_overrides(&mut url, &entries, true);
        assert_eq!(url.port(), Some(9000));
    }
}
