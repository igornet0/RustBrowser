//! Network identity for outbound HTTP requests.
//!
//! Servo's default User-Agent includes a `Servo/…` token. Many sites (Google in
//! particular) treat that as unusual traffic and send `/sorry` / reCAPTCHA pages.
//! We present as a mainstream desktop Firefox build so Accept / UA negotiation
//! matches what sites expect from a real browser.

use servo::{Preferences, UserAgentPlatform};

/// Firefox ESR-aligned major version — keep in sync with Servo's Gecko claim.
const FIREFOX_MAJOR: &str = "140";

/// Build Servo preferences used for every navigation / fetch.
///
/// Servo's compiled-in defaults target the engine's test suite, not real websites:
/// CSS Grid, `@font-face`, IndexedDB, Container Queries and variable fonts are all
/// disabled out of the box. Browsing a modern site with those off produces broken
/// layouts, so the web-platform features that are usable in Servo 0.5 are enabled
/// here. Anything still marked experimental in Servo (Service Workers, WebGPU,
/// WebRTC) stays off — it is the usual source of wedged pages.
pub fn build_preferences(locale_override: Option<&str>) -> Preferences {
    let mut preferences = Preferences::default();
    preferences.user_agent = desktop_user_agent();
    if let Some(locale) = locale_override.map(str::trim).filter(|s| !s.is_empty()) {
        preferences.intl_locale_override = locale.to_string();
    }

    // ── CSS ────────────────────────────────────────────────────────────────
    // Without Grid a large fraction of 2020s sites collapses into a single column.
    preferences.layout_grid_enabled = true;
    preferences.layout_container_queries_enabled = true;
    preferences.layout_columns_enabled = true;
    preferences.layout_variable_fonts_enabled = true;

    // ── Web fonts ──────────────────────────────────────────────────────────
    // `@font-face` is off by default, so sites silently fall back to system fonts.
    preferences.dom_fontface_enabled = true;

    // ── Web platform APIs commonly required by sites ───────────────────────
    preferences.dom_indexeddb_enabled = true;
    preferences.dom_adoptedstylesheet_enabled = true;
    preferences.dom_intersection_observer_enabled = true;
    preferences.dom_async_clipboard_enabled = true;
    preferences.dom_web_animations_enabled = true;
    preferences.dom_offscreen_canvas_enabled = true;
    preferences.dom_webgl2_enabled = true;
    preferences.dom_entries_api_enabled = true;

    // ── Navigation / network ──────────────────────────────────────────────
    // 5 MiB default caps out on large documents and downloads.
    preferences.network_max_content_length = 128 * 1024 * 1024;
    // Keep a usable Back history (Servo defaults to 20 entries).
    preferences.session_history_max_length = 100;

    preferences
}

/// Platform-appropriate Firefox desktop User-Agent (no Servo token).
pub fn desktop_user_agent() -> String {
    match UserAgentPlatform::default() {
        UserAgentPlatform::Desktop if cfg!(all(target_os = "windows", target_arch = "x86_64")) => {
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{FIREFOX_MAJOR}.0) Gecko/20100101 Firefox/{FIREFOX_MAJOR}.0"
            )
        }
        UserAgentPlatform::Desktop if cfg!(target_os = "macos") => {
            format!(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:{FIREFOX_MAJOR}.0) Gecko/20100101 Firefox/{FIREFOX_MAJOR}.0"
            )
        }
        UserAgentPlatform::Desktop => {
            format!(
                "Mozilla/5.0 (X11; Linux x86_64; rv:{FIREFOX_MAJOR}.0) Gecko/20100101 Firefox/{FIREFOX_MAJOR}.0"
            )
        }
        UserAgentPlatform::Android => {
            format!(
                "Mozilla/5.0 (Android 14; Mobile; rv:{FIREFOX_MAJOR}.0) Gecko/{FIREFOX_MAJOR}.0 Firefox/{FIREFOX_MAJOR}.0"
            )
        }
        UserAgentPlatform::Ios => {
            format!(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) FxiOS/{FIREFOX_MAJOR}.0 Mobile/15E148"
            )
        }
        UserAgentPlatform::OpenHarmony => {
            format!(
                "Mozilla/5.0 (X11; Linux x86_64; rv:{FIREFOX_MAJOR}.0) Gecko/20100101 Firefox/{FIREFOX_MAJOR}.0"
            )
        }
    }
}
