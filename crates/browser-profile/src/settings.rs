use browser_core::{BrowserError, BrowserResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use url::Url;

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
    }
}
