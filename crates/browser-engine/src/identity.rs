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
pub fn build_preferences(locale_override: Option<&str>) -> Preferences {
    let mut preferences = Preferences::default();
    preferences.user_agent = desktop_user_agent();
    if let Some(locale) = locale_override.map(str::trim).filter(|s| !s.is_empty()) {
        preferences.intl_locale_override = locale.to_string();
    }
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
