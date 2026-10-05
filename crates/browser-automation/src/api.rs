//! Wire types of the automation HTTP API and request validation.

use browser_core::is_private_network_url;
use browser_ipc::MAX_SCRIPT_LENGTH;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use url::Url;

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const MIN_TIMEOUT_MS: u64 = 1_000;
/// Extra settle time after `load` so client-side rendering can finish.
pub const DEFAULT_WAIT_MS: u64 = 500;
pub const MAX_WAIT_MS: u64 = 30_000;
pub const MAX_SELECTOR_LENGTH: usize = 1_024;
pub const MAX_WAIT_JS_LENGTH: usize = 8 * 1024;
pub const MAX_NETWORK_IDLE_MS: u64 = 10_000;
/// Rendered HTML is cut to this many UTF-16 units inside the page (keeps IPC small).
pub const MAX_HTML_CHARS: usize = 5_000_000;

#[derive(Debug, Clone, Deserialize)]
pub struct RenderRequest {
    pub url: String,
    /// CSS selector to wait for before taking the snapshot.
    #[serde(default)]
    pub wait_for: Option<String>,
    /// Extra wait after load (and after `wait_for`), ms.
    #[serde(default)]
    pub wait_ms: Option<u64>,
    /// Whole-job budget, ms.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// JavaScript evaluated after the snapshot; its value is returned as `script_result`.
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub screenshot: bool,
    /// Wait until the page text contains this string.
    #[serde(default)]
    pub wait_text: Option<String>,
    /// Wait until this JavaScript expression is truthy.
    #[serde(default)]
    pub wait_js: Option<String>,
    /// Wait until no fetch/XHR has been in flight for this long, ms.
    #[serde(default)]
    pub network_idle_ms: Option<u64>,
    /// Return the request log and captured fetch/XHR responses.
    #[serde(default)]
    pub capture_network: bool,
}

/// All conditions must hold before the snapshot is taken.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaitCondition {
    pub selector: Option<String>,
    pub text: Option<String>,
    pub js: Option<String>,
    pub network_idle_ms: Option<u64>,
}

impl WaitCondition {
    pub fn is_empty(&self) -> bool {
        self.selector.is_none()
            && self.text.is_none()
            && self.js.is_none()
            && self.network_idle_ms.is_none()
    }
}

/// A request that passed validation.
#[derive(Debug, Clone)]
pub struct RenderJob {
    pub url: Url,
    pub condition: WaitCondition,
    pub wait: Duration,
    pub timeout: Duration,
    pub script: Option<String>,
    pub screenshot: bool,
    pub capture_network: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RenderResponse {
    pub final_url: String,
    pub title: String,
    pub html: String,
    pub html_truncated: bool,
    /// False when the page never reported `load` within the budget (snapshot taken anyway).
    pub load_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_for_found: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_png_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_error: Option<String>,
    /// `{requests, responses, hook}` when `capture_network` was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Value>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    BadRequest(String),
    /// Too many queued jobs.
    Busy,
    Timeout(String),
    Failed(String),
}

impl RenderError {
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Busy => 503,
            Self::Timeout(_) => 504,
            Self::Failed(_) => 502,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::BadRequest(m) => format!("bad request: {m}"),
            Self::Busy => "automation queue is full, retry later".into(),
            Self::Timeout(m) => format!("timeout: {m}"),
            Self::Failed(m) => m.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
    pub protocol: u32,
    pub content_pid: Option<u32>,
    pub connected: bool,
    pub active: usize,
    pub queued: usize,
    pub max_tabs: usize,
}

impl RenderRequest {
    pub fn validate(self) -> Result<RenderJob, RenderError> {
        let url = Url::parse(self.url.trim())
            .map_err(|e| RenderError::BadRequest(format!("url: {e}")))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(RenderError::BadRequest(format!(
                "only http/https urls are allowed, got `{}`",
                url.scheme()
            )));
        }
        if is_private_network_url(&url) {
            return Err(RenderError::BadRequest(
                "local / private network urls are not allowed".into(),
            ));
        }
        let trimmed = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let condition = WaitCondition {
            selector: trimmed(self.wait_for),
            text: trimmed(self.wait_text),
            js: trimmed(self.wait_js),
            network_idle_ms: self.network_idle_ms.filter(|ms| *ms > 0).map(|ms| ms.min(MAX_NETWORK_IDLE_MS)),
        };
        for (what, value, max) in [
            ("wait_for selector", &condition.selector, MAX_SELECTOR_LENGTH),
            ("wait_text", &condition.text, MAX_SELECTOR_LENGTH),
            ("wait_js", &condition.js, MAX_WAIT_JS_LENGTH),
        ] {
            if value.as_ref().is_some_and(|s| s.len() > max) {
                return Err(RenderError::BadRequest(format!("{what} is too long")));
            }
        }
        let script = self.script.filter(|s| !s.trim().is_empty());
        if script.as_ref().is_some_and(|s| s.len() > MAX_SCRIPT_LENGTH) {
            return Err(RenderError::BadRequest(format!(
                "script is longer than {MAX_SCRIPT_LENGTH} bytes"
            )));
        }
        let timeout = self
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
        let wait = self.wait_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS);
        Ok(RenderJob {
            url,
            condition,
            wait: Duration::from_millis(wait),
            timeout: Duration::from_millis(timeout),
            script,
            screenshot: self.screenshot,
            capture_network: self.capture_network,
        })
    }
}

/// Page snapshot evaluated in the tab: final URL, title and (bounded) rendered HTML.
pub fn snapshot_script() -> String {
    format!(
        "(() => {{ const h = document.documentElement ? document.documentElement.outerHTML : ''; \
         return {{ url: location.href, title: document.title || '', \
         html: h.slice(0, {MAX_HTML_CHARS}), html_length: h.length }}; }})()"
    )
}

/// `true` once every set condition holds; errors (bad selector, throwing JS) mean "not yet".
pub fn wait_probe_script(cond: &WaitCondition) -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut checks = Vec::new();
    if let Some(sel) = &cond.selector {
        checks.push(format!("if (!document.querySelector({})) return false;", quote(sel)));
    }
    if let Some(text) = &cond.text {
        checks.push(format!(
            "if (!(document.body && document.body.textContent.includes({}))) return false;",
            quote(text)
        ));
    }
    if let Some(js) = &cond.js {
        checks.push(format!("if (!(function () {{ return ({js}); }})()) return false;"));
    }
    if let Some(ms) = cond.network_idle_ms {
        // Without the recorder (non-automation content) there is nothing to wait for.
        checks.push(format!(
            "const n = window.__rbNet; if (n && (n.inflight > 0 || Date.now() - n.last < {ms})) return false;"
        ));
    }
    format!(
        "(() => {{ try {{ {} return true; }} catch (e) {{ return false; }} }})()",
        checks.join(" ")
    )
}

/// Captured fetch/XHR entries, or `null` when the recorder is not installed.
pub fn network_collect_script() -> &'static str {
    "(() => { const n = window.__rbNet; return n ? n.entries.slice() : null; })()"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str) -> RenderRequest {
        RenderRequest {
            url: url.into(),
            wait_for: None,
            wait_ms: None,
            timeout_ms: None,
            script: None,
            screenshot: false,
            wait_text: None,
            wait_js: None,
            network_idle_ms: None,
            capture_network: false,
        }
    }

    #[test]
    fn validates_scheme_and_private_hosts() {
        assert!(req("https://example.com/").validate().is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://localhost:8080/",
            "http://192.168.0.1/",
            "not a url",
        ] {
            assert!(
                matches!(req(bad).validate(), Err(RenderError::BadRequest(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn clamps_timeouts_and_drops_empty_options() {
        let job = RenderRequest {
            timeout_ms: Some(10_000_000),
            wait_ms: Some(999_999),
            wait_for: Some("   ".into()),
            script: Some("  ".into()),
            ..req("https://example.com/")
        }
        .validate()
        .unwrap();
        assert_eq!(job.timeout, Duration::from_millis(MAX_TIMEOUT_MS));
        assert_eq!(job.wait, Duration::from_millis(MAX_WAIT_MS));
        assert!(job.condition.is_empty());
        assert!(job.script.is_none());

        let job = req("https://example.com/").validate().unwrap();
        assert_eq!(job.timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));
        assert_eq!(job.wait, Duration::from_millis(DEFAULT_WAIT_MS));
    }

    #[test]
    fn wait_probe_quotes_input_and_combines_conditions() {
        let s = wait_probe_script(&WaitCondition {
            selector: Some(r#"a[href="x"]'); alert(1); ('"#.into()),
            ..Default::default()
        });
        assert!(s.contains(r#"document.querySelector("a[href=\"x\"]'); alert(1); ('")"#), "{s}");

        let s = wait_probe_script(&WaitCondition {
            selector: Some(".products".into()),
            text: Some("В наличии".into()),
            js: Some("window.app && window.app.ready".into()),
            network_idle_ms: Some(800),
        });
        assert!(s.contains("textContent.includes(\"В наличии\")"), "{s}");
        assert!(s.contains("return (window.app && window.app.ready);"), "{s}");
        assert!(s.contains("Date.now() - n.last < 800"), "{s}");
    }

    #[test]
    fn network_idle_is_bounded() {
        let job = RenderRequest {
            network_idle_ms: Some(999_999),
            wait_text: Some("  ".into()),
            ..req("https://example.com/")
        }
        .validate()
        .unwrap();
        assert_eq!(job.condition.network_idle_ms, Some(MAX_NETWORK_IDLE_MS));
        assert!(job.condition.text.is_none());
    }
}
