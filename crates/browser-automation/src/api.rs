//! Wire types of the automation HTTP API and request validation.

use base64::Engine as _;
use browser_core::is_private_network_url;
use browser_ipc::{validate_cookies, CookieInfo, MAX_SCRIPT_LENGTH};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
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

    /// Selector/text/JS can be checked while the page is still loading: many sites
    /// never fire `load` (endless analytics requests). Network idle is only meaningful
    /// after it.
    pub fn checkable_during_load(&self) -> bool {
        !self.is_empty() && self.network_idle_ms.is_none()
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
        // The probe may run before the navigation commits; never match the blank page.
        "(() => {{ try {{ if (location.href === 'about:blank') return false; {} return true; }} catch (e) {{ return false; }} }})()",
        checks.join(" ")
    )
}

/// Captured fetch/XHR entries, or `null` when the recorder is not installed.
pub fn network_collect_script() -> &'static str {
    "(() => { const n = window.__rbNet; return n ? n.entries.slice() : null; })()"
}

// ---------------------------------------------------------------------------
// Session / authenticated request API
// ---------------------------------------------------------------------------

pub const DEFAULT_SESSION_ID: &str = "default";
pub const MAX_SESSION_ID_LENGTH: usize = 64;

pub const DEFAULT_HTTP_TIMEOUT_MS: u64 = 30_000;
pub const MAX_HTTP_TIMEOUT_MS: u64 = 120_000;
/// Raw HTTP response bodies are bounded so a single call cannot exhaust memory.
pub const MAX_HTTP_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
/// `fetch()` request bodies are embedded into evaluated JS (bounded by IPC too).
pub const MAX_PAGE_FETCH_BODY_BYTES: usize = 32 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_HEADER_NAME: usize = 128;
pub const MAX_HEADER_VALUE: usize = 8 * 1024;
pub const DEFAULT_LOGIN_TIMEOUT_MS: u64 = 15 * 60 * 1000;
pub const MAX_LOGIN_TIMEOUT_MS: u64 = 60 * 60 * 1000;
const MAX_PAGE_FETCH_RESPONSE_CHARS: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub content_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginOpenRequest {
    pub url: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginOpenResponse {
    pub session: String,
    pub url: String,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CookieQuery {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetCookiesRequest {
    pub url: String,
    pub cookies: Vec<CookieInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CookieListResponse {
    pub url: String,
    pub cookies: Vec<CookieInfo>,
}

/// A raw HTTP request executed with the session's exported cookies.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpRequestSpec {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_base64: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Default `true`.
    #[serde(default)]
    pub follow_redirects: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ValidatedHttpRequest {
    pub url: Url,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    pub follow_redirects: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpResult {
    pub url: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    pub body_length: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// A `fetch()` executed by a page in the session (cookies, CSRF and page JS work).
#[derive(Debug, Clone, Deserialize)]
pub struct PageFetchRequest {
    pub url: String,
    /// Page that should host the fetch; defaults to the request URL's origin.
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_base64: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PageFetchJob {
    pub url: Url,
    pub page_url: Url,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body_base64: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PageFetchResponse {
    pub page_url: String,
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub body_length: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl CreateSessionRequest {
    pub fn validate(self) -> Result<String, RenderError> {
        match self.id {
            None => Ok(DEFAULT_SESSION_ID.to_string()),
            Some(id) => {
                validate_session_id(&id)?;
                Ok(id)
            }
        }
    }
}

pub fn validate_session_id(id: &str) -> Result<(), RenderError> {
    if id.is_empty() || id.len() > MAX_SESSION_ID_LENGTH {
        return Err(RenderError::BadRequest(format!(
            "session id must be 1..={MAX_SESSION_ID_LENGTH} characters"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(RenderError::BadRequest(
            "session id may contain only letters, digits, '-' and '_'".into(),
        ));
    }
    Ok(())
}

impl LoginOpenRequest {
    pub fn validate(self) -> Result<(Url, Duration), RenderError> {
        let url = validate_target_url(&self.url)?;
        let timeout = Duration::from_millis(
            self.timeout_ms
                .unwrap_or(DEFAULT_LOGIN_TIMEOUT_MS)
                .clamp(10_000, MAX_LOGIN_TIMEOUT_MS),
        );
        Ok((url, timeout))
    }
}

impl SetCookiesRequest {
    pub fn validate(self) -> Result<(Url, Vec<CookieInfo>), RenderError> {
        let url = validate_target_url(&self.url)?;
        validate_cookies(&self.cookies).map_err(|e| RenderError::BadRequest(e.to_string()))?;
        Ok((url, self.cookies))
    }
}

impl HttpRequestSpec {
    pub fn validate(self) -> Result<ValidatedHttpRequest, RenderError> {
        let url = validate_target_url(&self.url)?;
        let method = validate_method(self.method.as_deref(), false)?;
        let headers = validate_headers(self.headers, false)?;
        let body = decode_body(self.body, self.body_base64, MAX_HTTP_BODY_BYTES_HARD)?;
        let timeout = Duration::from_millis(
            self.timeout_ms
                .unwrap_or(DEFAULT_HTTP_TIMEOUT_MS)
                .clamp(MIN_TIMEOUT_MS, MAX_HTTP_TIMEOUT_MS),
        );
        Ok(ValidatedHttpRequest {
            url,
            method,
            headers,
            body,
            timeout,
            follow_redirects: self.follow_redirects.unwrap_or(true),
        })
    }
}

/// Raw HTTP bodies can be larger than page `fetch()` bodies.
pub const MAX_HTTP_BODY_BYTES_HARD: usize = MAX_HTTP_RESPONSE_BYTES;

impl PageFetchRequest {
    pub fn validate(self) -> Result<PageFetchJob, RenderError> {
        let url = validate_target_url(&self.url)?;
        let page_url = match self.page_url {
            Some(page) => validate_target_url(&page)?,
            None => default_page_url(&url),
        };
        let method = validate_method(self.method.as_deref(), true)?;
        let headers = validate_headers(self.headers, true)?;
        let body = decode_body(
            self.body,
            self.body_base64,
            MAX_PAGE_FETCH_BODY_BYTES,
        )?;
        if !body.is_empty() && matches!(method.as_str(), "GET" | "HEAD") {
            return Err(RenderError::BadRequest(
                "GET/HEAD requests cannot have a body in page fetch".into(),
            ));
        }
        let body_base64 = if body.is_empty() {
            None
        } else {
            Some(base64::engine::general_purpose::STANDARD.encode(body))
        };
        let timeout = Duration::from_millis(
            self.timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS),
        );
        Ok(PageFetchJob {
            url,
            page_url,
            method,
            headers,
            body_base64,
            timeout,
        })
    }
}

pub(crate) fn validate_target_url(raw: &str) -> Result<Url, RenderError> {
    let url = Url::parse(raw.trim())
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
    Ok(url)
}

fn default_page_url(url: &Url) -> Url {
    let mut root = url.clone();
    root.set_path("/");
    root.set_query(None);
    root.set_fragment(None);
    root
}

fn validate_method(raw: Option<&str>, page_fetch: bool) -> Result<String, RenderError> {
    let method = raw.unwrap_or("GET").trim().to_ascii_uppercase();
    const ALLOWED: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    if !ALLOWED.contains(&method.as_str()) {
        return Err(RenderError::BadRequest(format!(
            "method `{method}` is not allowed"
        )));
    }
    if page_fetch && method == "OPTIONS" {
        return Err(RenderError::BadRequest(
            "OPTIONS is not meaningful for page fetch".into(),
        ));
    }
    Ok(method)
}

fn validate_headers(
    headers: BTreeMap<String, String>,
    page_fetch: bool,
) -> Result<BTreeMap<String, String>, RenderError> {
    if headers.len() > MAX_HEADERS {
        return Err(RenderError::BadRequest(format!(
            "too many headers: {} > {MAX_HEADERS}",
            headers.len()
        )));
    }
    let mut out = BTreeMap::new();
    for (name, value) in headers {
        if name.len() > MAX_HEADER_NAME {
            return Err(RenderError::BadRequest(format!(
                "header name too long: {name}"
            )));
        }
        if value.len() > MAX_HEADER_VALUE {
            return Err(RenderError::BadRequest(format!(
                "header value too long for {name}"
            )));
        }
        let parsed = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| RenderError::BadRequest(format!("header name `{name}`: {e}")))?;
        http::HeaderValue::from_str(&value)
            .map_err(|e| RenderError::BadRequest(format!("header value for `{name}`: {e}")))?;
        if page_fetch {
            // The browser forbids these from page JS anyway; fail loudly.
            if matches!(
                parsed,
                http::header::COOKIE
                    | http::header::HOST
                    | http::header::CONTENT_LENGTH
                    | http::header::CONNECTION
            ) {
                return Err(RenderError::BadRequest(format!(
                    "header `{name}` cannot be set by page fetch"
                )));
            }
        }
        out.insert(parsed.as_str().to_string(), value);
    }
    Ok(out)
}

fn decode_body(
    body: Option<String>,
    body_base64: Option<String>,
    max: usize,
) -> Result<Vec<u8>, RenderError> {
    match (body, body_base64) {
        (Some(_), Some(_)) => Err(RenderError::BadRequest(
            "use either `body` or `body_base64`, not both".into(),
        )),
        (None, None) => Ok(Vec::new()),
        (Some(text), None) => {
            if text.len() > max {
                return Err(RenderError::BadRequest(format!(
                    "body is larger than {max} bytes"
                )));
            }
            Ok(text.into_bytes())
        }
        (None, Some(encoded)) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|e| RenderError::BadRequest(format!("body_base64: {e}")))?;
            if bytes.len() > max {
                return Err(RenderError::BadRequest(format!(
                    "body is larger than {max} bytes"
                )));
            }
            Ok(bytes)
        }
    }
}

/// JS that starts the fetch and stores the outcome on `window.__rbSession`.
pub fn page_fetch_start_script(job: &PageFetchJob) -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let url = quote(job.url.as_str());
    let method = quote(&job.method);
    let headers = serde_json::to_string(&job.headers).unwrap_or_else(|_| "{}".into());
    let body = match &job.body_base64 {
        Some(encoded) => format!(
            "Uint8Array.from(atob({}), (c) => c.charCodeAt(0))",
            quote(encoded)
        ),
        None => "null".into(),
    };
    let timeout_ms = job.timeout.as_millis().min(u128::from(u64::MAX)) as u64;
    format!(
        r#"(() => {{
  window.__rbSession = {{ done: false }};
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), {timeout_ms});
  fetch({url}, {{
    method: {method},
    headers: {headers},
    body: {body},
    credentials: 'include',
    redirect: 'follow',
    signal: controller.signal
  }}).then(async (res) => {{
    const headers = {{}};
    res.headers.forEach((value, key) => {{ headers[key] = value; }});
    const text = await res.text();
    const limit = {MAX_PAGE_FETCH_RESPONSE_CHARS};
    const truncated = text.length > limit;
    window.__rbSession = {{
      done: true,
      ok: res.ok,
      status: res.status,
      status_text: res.statusText,
      url: res.url,
      headers,
      body: truncated ? text.slice(0, limit) : text,
      body_length: text.length,
      truncated
    }};
  }}).catch((error) => {{
    window.__rbSession = {{ done: true, error: String(error) }};
  }}).finally(() => clearTimeout(timer));
  return true;
}})()"#
    )
}

/// Returns the finished fetch result, or `null` while it is still running.
pub fn page_fetch_poll_script() -> &'static str {
    "(window.__rbSession && window.__rbSession.done) ? window.__rbSession : null"
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
    fn validates_session_ids() {
        assert!(validate_session_id("work-1").is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("../etc").is_err());
        assert!(validate_session_id(&"x".repeat(MAX_SESSION_ID_LENGTH + 1)).is_err());
    }

    #[test]
    fn http_request_validates_method_headers_and_body() {
        let req = HttpRequestSpec {
            url: "https://example.com/api".into(),
            method: Some("post".into()),
            headers: BTreeMap::new(),
            body: Some("{}".into()),
            body_base64: None,
            timeout_ms: None,
            follow_redirects: None,
        }
        .validate()
        .unwrap();
        assert_eq!(req.method, "POST");
        assert!(req.follow_redirects);
        assert_eq!(req.body, b"{}");

        let bad_method = HttpRequestSpec {
            url: "https://example.com/".into(),
            method: Some("TRACE".into()),
            headers: BTreeMap::new(),
            body: None,
            body_base64: None,
            timeout_ms: None,
            follow_redirects: None,
        };
        assert!(matches!(bad_method.validate(), Err(RenderError::BadRequest(_))));

        let both_bodies = HttpRequestSpec {
            url: "https://example.com/".into(),
            method: None,
            headers: BTreeMap::new(),
            body: Some("x".into()),
            body_base64: Some("eA==".into()),
            timeout_ms: None,
            follow_redirects: None,
        };
        assert!(matches!(both_bodies.validate(), Err(RenderError::BadRequest(_))));
    }

    #[test]
    fn page_fetch_derives_page_url_and_forbids_cookie_header() {
        let job = PageFetchRequest {
            url: "https://example.com/api/x?q=1".into(),
            page_url: None,
            method: Some("POST".into()),
            headers: BTreeMap::new(),
            body: Some("hello".into()),
            body_base64: None,
            timeout_ms: None,
        }
        .validate()
        .unwrap();
        assert_eq!(job.page_url.as_str(), "https://example.com/");
        assert_eq!(
            job.body_base64.as_deref(),
            Some("aGVsbG8="),
            "body is embedded as base64"
        );

        let mut headers = BTreeMap::new();
        headers.insert("cookie".into(), "sid=1".into());
        let forbidden = PageFetchRequest {
            url: "https://example.com/".into(),
            page_url: None,
            method: None,
            headers,
            body: None,
            body_base64: None,
            timeout_ms: None,
        };
        assert!(matches!(
            forbidden.validate(),
            Err(RenderError::BadRequest(_))
        ));

        let get_with_body = PageFetchRequest {
            url: "https://example.com/".into(),
            page_url: None,
            method: None,
            headers: BTreeMap::new(),
            body: Some("x".into()),
            body_base64: None,
            timeout_ms: None,
        };
        assert!(matches!(
            get_with_body.validate(),
            Err(RenderError::BadRequest(_))
        ));
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
