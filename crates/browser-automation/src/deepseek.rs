//! DeepSeek plugin: drives `chat.deepseek.com` through a session's page tab.
//!
//! The plugin is deliberately a separate module with its own request types,
//! validation and JavaScript driver. It talks to the site the same way the
//! browser UI does:
//!
//! * prompts are typed into the real composer and submitted with a synthetic
//!   Enter key, so DeepSeek's PoW / streaming code runs unchanged;
//! * the answer is read from `.ds-assistant-message-main-content`;
//! * when DeepSeek asks to continue a truncated answer, the plugin presses the
//!   real Continue button by invoking its React handler (the visible button
//!   ignores untrusted clicks), so the app's own stream stays in sync;
//! * chat management uses the authenticated `/api/v0/chat_session/*` API from
//!   inside the page, where cookies, WAF and bearer token already exist.
//!
//! Google login / captcha is never automated: use
//! `POST /sessions/{id}/login/open` once with the DeepSeek sign-in URL.

use crate::api::RenderError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

pub const DEEPSEEK_ORIGIN: &str = "https://chat.deepseek.com";
pub const DEEPSEEK_ROOT: &str = "https://chat.deepseek.com/";
pub const DEFAULT_DEEPSEEK_TIMEOUT_MS: u64 = 180_000;
pub const MAX_DEEPSEEK_TIMEOUT_MS: u64 = 900_000;
pub const DEFAULT_DEEPSEEK_SETTLE_MS: u64 = 2_000;
pub const MAX_PROMPT_CHARS: usize = 200_000;
pub const MAX_CHAT_ID_LENGTH: usize = 128;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_DELETE_CHATS: usize = 100;

/// `POST /sessions/{id}/deepseek/chat`
#[derive(Debug, Clone, Deserialize)]
pub struct DeepSeekChatRequest {
    pub prompt: String,
    /// Existing chat to continue; omit to send into a new chat.
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// How long the answer text must stay unchanged before it counts as done.
    #[serde(default)]
    pub settle_ms: Option<u64>,
    /// Safety valve for answers that keep asking to continue.
    #[serde(default)]
    pub max_continues: Option<u32>,
    /// DeepSeek's "DeepThink" reasoning toggle: `true`/`false` forces the
    /// composer switch before sending, omit to keep whatever the page shows.
    #[serde(default, alias = "deep_thinking", alias = "deep_think")]
    pub thinking: Option<bool>,
    /// DeepSeek's "Search" (web search) toggle; same semantics as `thinking`.
    #[serde(default, alias = "smart_search", alias = "web_search")]
    pub search: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DeepSeekChatJob {
    pub prompt: String,
    pub chat_id: Option<String>,
    pub page_url: Url,
    pub timeout: Duration,
    pub settle_ms: u64,
    pub max_continues: u32,
    pub thinking: Option<bool>,
    pub search: Option<bool>,
}

/// `GET /sessions/{id}/deepseek/chats`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeepSeekChatsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `POST /sessions/{id}/deepseek/chats`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeepSeekCreateChatRequest {
    #[serde(default)]
    pub title: Option<String>,
}

/// `DELETE /sessions/{id}/deepseek/chats`
#[derive(Debug, Clone, Deserialize)]
pub struct DeepSeekDeleteChatsRequest {
    pub chat_ids: Vec<String>,
}

/// `POST /sessions/{id}/deepseek/chats/{chat_id}/rename`
#[derive(Debug, Clone, Deserialize)]
pub struct DeepSeekRenameChatRequest {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeepSeekChatSummary {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    pub updated_at: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeepSeekMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub status: String,
}

/// Validated plugin operation, executed by the session actor.
#[derive(Debug, Clone)]
pub enum DeepSeekJob {
    Chat(DeepSeekChatJob),
    ListChats { limit: usize },
    CreateChat { title: Option<String> },
    DeleteChats { chat_ids: Vec<String> },
    RenameChat { chat_id: String, title: String },
    History { chat_id: String },
    Status,
}

impl DeepSeekJob {
    pub fn page_url(&self) -> Url {
        match self {
            Self::Chat(job) => job.page_url.clone(),
            Self::History { chat_id } => chat_url(chat_id),
            _ => Url::parse(DEEPSEEK_ROOT).expect("static DeepSeek URL"),
        }
    }

    /// Chat and history pages are exact: a "new chat" must not reuse an open
    /// chat tab, otherwise the prompt would land in that chat. API-only
    /// operations can reuse any page on the site.
    pub fn needs_exact_page(&self) -> bool {
        matches!(self, Self::Chat(_) | Self::History { .. })
    }

    pub fn matches_tab(&self, url: &Url) -> bool {
        if url.scheme() != "https" || url.host_str() != Some("chat.deepseek.com") {
            return false;
        }
        if !self.needs_exact_page() {
            return true;
        }
        let want = self.page_url();
        url.as_str().trim_end_matches('/') == want.as_str().trim_end_matches('/')
    }

    pub fn timeout(&self) -> Duration {
        match self {
            Self::Chat(job) => job.timeout,
            _ => Duration::from_secs(90),
        }
    }

    /// Read-only operations can be retried if a page reload wipes them.
    /// Mutating operations (chat/create/delete/rename) never are, so a retry
    /// cannot send a prompt or create a chat twice.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Status | Self::ListChats { .. } | Self::History { .. }
        )
    }

    /// JavaScript that starts the operation and stores its state on
    /// `window.__rbDeepSeek`; `deepseek_poll_script` reads it later.
    pub fn setup_script(&self) -> String {
        let config = match self {
            Self::Chat(job) => json!({
                "op": "chat",
                "prompt": &job.prompt,
                "newChat": job.chat_id.is_none(),
                "settleMs": job.settle_ms,
                "maxContinues": job.max_continues,
                "thinking": job.thinking,
                "search": job.search,
                "timeoutMs": job.timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            }),
            Self::ListChats { limit } => json!({ "op": "list", "limit": limit }),
            Self::CreateChat { title } => json!({ "op": "create", "title": title }),
            Self::DeleteChats { chat_ids } => json!({ "op": "delete", "chatIds": chat_ids }),
            Self::RenameChat { chat_id, title } => {
                json!({ "op": "rename", "chatId": chat_id, "title": title })
            }
            Self::History { chat_id } => json!({ "op": "history", "chatId": chat_id }),
            Self::Status => json!({ "op": "status" }),
        };
        let config = serde_json::to_string(&config).unwrap_or_else(|_| "{}".into());
        format!(
            r#"{DEEPSEEK_PRELUDE}
  const CFG = {config};
  (async () => {{
    try {{
      if (!alive()) return;
      if (CFG.op === 'chat') await runChat();
      else if (CFG.op === 'list') await runList();
      else if (CFG.op === 'create') await runCreate();
      else if (CFG.op === 'delete') await runDelete();
      else if (CFG.op === 'rename') await runRename();
      else if (CFG.op === 'history') await runHistory();
      else if (CFG.op === 'status') await runStatus();
      else throw new Error('unknown DeepSeek operation');
    }} catch (error) {{
      fail(error);
    }}
  }})();
  return true;
}})();"#
        )
    }
}

/// Returns `{done, phase, error, result, log}` while running, or `null` when the
/// operation is still in progress. If the page was replaced by a full reload
/// the state object is gone; the `sessionStorage` marker lets us report that
/// instead of polling until the timeout.
pub fn deepseek_poll_script() -> &'static str {
    r#"(() => {
  const s = window.__rbDeepSeek;
  if (s) return s.done ? s : null;
  let gen = null;
  try { gen = sessionStorage.getItem('__rbDeepSeekGen'); } catch (e) {}
  return gen ? {
    done: true,
    error: 'the DeepSeek page reloaded before the operation finished',
    result: null
  } : null;
})()"#
}

fn chat_url(chat_id: &str) -> Url {
    Url::parse(&format!("{DEEPSEEK_ORIGIN}/a/chat/s/{chat_id}")).expect("valid chat URL")
}

pub(crate) fn validate_chat_id(raw: &str) -> Result<String, RenderError> {
    let id = raw.trim();
    if id.is_empty() || id.len() > MAX_CHAT_ID_LENGTH {
        return Err(RenderError::BadRequest(format!(
            "chat id must be 1..={MAX_CHAT_ID_LENGTH} characters"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(RenderError::BadRequest(
            "chat id may contain only letters, digits, '-' and '_'".into(),
        ));
    }
    Ok(id.to_string())
}

fn validate_title(raw: &str) -> Result<String, RenderError> {
    let title = raw.trim();
    if title.is_empty() {
        return Err(RenderError::BadRequest("title must not be empty".into()));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(RenderError::BadRequest(format!(
            "title is longer than {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(title.to_string())
}

impl DeepSeekChatRequest {
    pub fn validate(self) -> Result<DeepSeekChatJob, RenderError> {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(RenderError::BadRequest("prompt must not be empty".into()));
        }
        if prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(RenderError::BadRequest(format!(
                "prompt is longer than {MAX_PROMPT_CHARS} characters"
            )));
        }
        let chat_id = match self.chat_id {
            Some(id) => Some(validate_chat_id(&id)?),
            None => None,
        };
        let page_url = match &chat_id {
            Some(id) => chat_url(id),
            None => Url::parse(DEEPSEEK_ROOT).expect("static DeepSeek URL"),
        };
        let timeout = Duration::from_millis(
            self.timeout_ms
                .unwrap_or(DEFAULT_DEEPSEEK_TIMEOUT_MS)
                .clamp(30_000, MAX_DEEPSEEK_TIMEOUT_MS),
        );
        let settle_ms = self
            .settle_ms
            .unwrap_or(DEFAULT_DEEPSEEK_SETTLE_MS)
            .clamp(500, 30_000);
        let max_continues = self.max_continues.unwrap_or(10).min(50);
        Ok(DeepSeekChatJob {
            prompt,
            chat_id,
            page_url,
            timeout,
            settle_ms,
            max_continues,
            thinking: self.thinking,
            search: self.search,
        })
    }
}

impl DeepSeekChatsQuery {
    pub fn validate(self) -> Result<usize, RenderError> {
        Ok(self.limit.unwrap_or(50).clamp(1, 200))
    }
}

impl DeepSeekCreateChatRequest {
    pub fn validate(self) -> Result<Option<String>, RenderError> {
        self.title.map(|t| validate_title(&t)).transpose()
    }
}

impl DeepSeekDeleteChatsRequest {
    pub fn validate(self) -> Result<Vec<String>, RenderError> {
        if self.chat_ids.is_empty() {
            return Err(RenderError::BadRequest(
                "chat_ids must contain at least one id".into(),
            ));
        }
        if self.chat_ids.len() > MAX_DELETE_CHATS {
            return Err(RenderError::BadRequest(format!(
                "at most {MAX_DELETE_CHATS} chats can be deleted at once"
            )));
        }
        self.chat_ids
            .iter()
            .map(|id| validate_chat_id(id))
            .collect()
    }
}

impl DeepSeekRenameChatRequest {
    pub fn validate(self) -> Result<String, RenderError> {
        validate_title(&self.title)
    }
}

/// Parse the normalized `chat_sessions` list produced by the page script.
pub fn parse_chat_list(value: &Value) -> Result<Vec<DeepSeekChatSummary>, String> {
    let chats = value
        .get("chats")
        .and_then(Value::as_array)
        .ok_or_else(|| "plugin returned no chat list".to_string())?;
    let mut out = Vec::with_capacity(chats.len());
    for chat in chats {
        let Some(id) = chat.get("id").and_then(Value::as_str) else {
            continue;
        };
        out.push(DeepSeekChatSummary {
            id: id.to_string(),
            title: chat
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            pinned: chat.get("pinned").and_then(Value::as_bool).unwrap_or(false),
            updated_at: chat
                .get("updated_at")
                .and_then(Value::as_u64)
                .or_else(|| {
                    chat.get("updated_at")
                        .and_then(Value::as_f64)
                        .filter(|n| *n >= 0.0)
                        .map(|n| n as u64)
                })
                .unwrap_or(0),
            url: chat
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| chat_url(id).to_string()),
        });
    }
    Ok(out)
}

/// Parse the `messages` list produced by the page script.
pub fn parse_messages(value: &Value) -> Result<Vec<DeepSeekMessage>, String> {
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "plugin returned no messages".to_string())?;
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        out.push(DeepSeekMessage {
            id: message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            role: message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase(),
            text: message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: message
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    Ok(out)
}

/// Shared helpers for every plugin operation.
const DEEPSEEK_PRELUDE: &str = r#"(() => {
  const GEN = (window.__rbDeepSeekGen || 0) + 1;
  window.__rbDeepSeekGen = GEN;
  const S = window.__rbDeepSeek = {
    done: false, running: true, phase: 'starting', error: null,
    result: null, startedAt: Date.now(), log: []
  };
  try { sessionStorage.setItem('__rbDeepSeekGen', String(GEN)); } catch (e) {}
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const alive = () => window.__rbDeepSeekGen === GEN;
  const txt = (el) => el ? (el.innerText || el.textContent || '') : '';
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const CONTINUE_LABELS = ['continue', 'продолжить', '继续生成', '続きを生成', '계속 생성', 'continuar', 'fortsetzen', 'devam et'];
  const STOP_LABELS = ['stop', 'остановить', 'стоп', '停止生成', '停止', '중지', 'detener', 'arrêter', 'stoppen', 'durdur'];
  const isContinueText = (s) => CONTINUE_LABELS.indexOf(norm(s)) >= 0;
  const isStopText = (s) => STOP_LABELS.indexOf(norm(s)) >= 0;

  function inputEl() {
    return document.querySelector('textarea[name="user query"]')
      || document.querySelector('textarea[placeholder]')
      || document.querySelector('textarea');
  }
  function assistantNodes() {
    return Array.prototype.slice.call(document.querySelectorAll('.ds-assistant-message-main-content'));
  }
  function lastAssistant() {
    const nodes = assistantNodes();
    return nodes.length ? nodes[nodes.length - 1] : null;
  }
  function itemOf(el) {
    let node = el;
    while (node && node !== document.body) {
      if (node.getAttribute && node.getAttribute('data-virtual-list-item-key') != null) return node;
      node = node.parentElement;
    }
    return null;
  }
  function chatIdFromUrl() {
    const match = location.pathname.match(/\/a\/chat\/s\/([A-Za-z0-9_-]+)/);
    return match ? match[1] : null;
  }
  function buttons(scope) {
    return Array.prototype.slice.call((scope || document).querySelectorAll('button,[role="button"]'));
  }
  function findContinue(node) {
    if (!node) return null;
    const item = itemOf(node);
    const scope = item || document;
    let list = buttons(scope).filter((b) => isContinueText(txt(b)));
    if (!list.length && scope !== document) {
      list = buttons(document).filter((b) => isContinueText(txt(b)));
    }
    if (!list.length) return null;
    const exact = list.filter((b) => itemOf(b) === item);
    return exact.length ? exact[0] : list[list.length - 1];
  }
  // DeepSeek's Continue button ignores untrusted clicks, so invoke the React
  // onClick prop directly with a native event marked as trusted. This runs the
  // app's own continueCompletion(), which keeps the UI stream in sync.
  function reactClick(element) {
    let node = element;
    for (let depth = 0; depth < 4 && node; depth++, node = node.parentElement) {
      const key = Object.keys(node).find((k) => k.indexOf('__reactProps$') === 0);
      const props = key ? node[key] : null;
      if (props && typeof props.onClick === 'function') {
        const native = new MouseEvent('click', { bubbles: true, cancelable: true, view: window });
        try { Object.defineProperty(native, 'isTrusted', { value: true }); } catch (e) {}
        const event = {
          nativeEvent: native,
          currentTarget: node,
          target: element,
          type: 'click',
          bubbles: true,
          cancelable: true,
          defaultPrevented: false,
          preventDefault() { this.defaultPrevented = true; },
          stopPropagation() {},
          isDefaultPrevented() { return this.defaultPrevented; },
          isPropagationStopped() { return false; },
          persist() {}
        };
        props.onClick(event);
        return true;
      }
    }
    return false;
  }
  function hasStopButton() {
    return buttons(document).some((b) =>
      isStopText(txt(b)) ||
      isStopText(b.getAttribute('aria-label') || '') ||
      isStopText(b.getAttribute('title') || ''));
  }
  // DeepSeek renders its "DeepThink" and "Search" switches as `.ds-toggle-button`
  // chips in the composer; `aria-pressed` / `--selected` carry their state.
  // They are clicked through their React `onClick`, the same handler the user
  // hits, so the app's own store (and the outgoing payload) stays consistent.
  const THINK_LABELS = ['deepthink', 'deep think', 'глубокое мышление', 'глубокое размышление', '深度思考', '智能思考'];
  const SEARCH_LABELS = ['search', 'smart search', 'поиск', 'умный поиск', 'веб-поиск', '联网搜索', '智能搜索'];
  function composerToggles() {
    let node = inputEl();
    for (let depth = 0; depth < 10 && node; depth++, node = node.parentElement) {
      const list = Array.prototype.slice.call(node.querySelectorAll('.ds-toggle-button'));
      if (list.length) return list;
    }
    return Array.prototype.slice.call(document.querySelectorAll('.ds-toggle-button'));
  }
  function findToggle(kind) {
    const list = composerToggles();
    const labels = kind === 'thinking' ? THINK_LABELS : SEARCH_LABELS;
    const byText = list.filter((el) => labels.indexOf(norm(txt(el))) >= 0);
    if (byText.length) return byText[0];
    // Unknown locale: the composer renders DeepThink first, then Search.
    if (list.length === 2) return kind === 'thinking' ? list[0] : list[1];
    return null;
  }
  function toggleEnabled(el) {
    return el.getAttribute('aria-pressed') === 'true'
      || el.classList.contains('ds-toggle-button--selected');
  }
  function toggleDisabled(el) {
    return el.getAttribute('aria-disabled') === 'true'
      || el.classList.contains('ds-toggle-button--disabled');
  }
  async function setToggle(kind, want) {
    const el = await waitFor(() => findToggle(kind), 5000);
    if (!el) throw new Error('the ' + kind + ' toggle is not available in this chat');
    if (toggleDisabled(el)) throw new Error('the ' + kind + ' toggle is disabled in this chat');
    for (let attempt = 0; attempt < 3; attempt++) {
      if (toggleEnabled(el) === want) return;
      if (!reactClick(el)) el.click();
      if (await waitFor(() => toggleEnabled(el) === want, 1500)) return;
      if (!alive()) throw new Error('cancelled');
    }
    throw new Error('could not turn ' + (want ? 'on' : 'off') + ' the ' + kind + ' toggle');
  }
  function toggleState(kind) {
    const el = findToggle(kind);
    return el ? toggleEnabled(el) : null;
  }
  // The composer's main button becomes the Stop button while a generation is
  // running (empty input + enabled button). The Stop button itself is an icon
  // with no label, so this state check is more reliable than text matching.
  function composerButton(input) {
    let node = input;
    for (let depth = 0; depth < 10 && node; depth++, node = node.parentElement) {
      if (node.querySelector && node.querySelector('[data-virtual-list-item-key]')) return null;
      const list = buttons(node);
      if (list.length) return list[list.length - 1];
    }
    return null;
  }
  function isGenerating(input) {
    if (!input || (input.value || '').trim()) return false;
    const button = composerButton(input);
    if (!button) return hasStopButton();
    return !button.disabled && button.getAttribute('aria-disabled') !== 'true';
  }
  // Signed-out sessions still render a composer, so check the site's own
  // storage token and the login call-to-action before typing.
  function looksSignedOut() {
    if (/\/sign_in/.test(location.pathname)) return true;
    if (authToken()) return false;
    // A visible composer means the app is usable even if the token key moved.
    if (inputEl()) return false;
    const labels = ['log in', 'login', 'sign in', 'signin', '登录', '登陆', 'войти', 'вход', 'iniciar sesión', 'connexion', 'anmelden'];
    return buttons(document).some((b) => labels.indexOf(norm(txt(b))) >= 0);
  }
  function requireSignedIn() {
    if (looksSignedOut()) throw new Error('session is not signed in to DeepSeek');
  }
  function findToken(value, depth) {
    if (!value || depth > 3 || typeof value !== 'object') return null;
    if (typeof value.token === 'string' && value.token.length > 20) return value.token;
    for (const key in value) {
      if (value[key] && typeof value[key] === 'object') {
        const found = findToken(value[key], depth + 1);
        if (found) return found;
      }
    }
    return null;
  }
  function tokenFromRaw(raw) {
    if (!raw) return null;
    try {
      const parsed = JSON.parse(raw);
      if (typeof parsed === 'string' && parsed.length > 20) return parsed;
      const found = findToken(parsed, 0);
      if (found) return found;
    } catch (e) {}
    const plain = raw.replace(/^"|"$/g, '');
    if (plain.length > 40 && plain.split('.').length === 3 && /^[A-Za-z0-9_.-]+$/.test(plain)) {
      return plain;
    }
    return null;
  }
  // The web app persists the bearer token under `userToken` (and mirrors the
  // user record under `__appKit_userInfo`). Prefer those keys, then fall back
  // to scanning the stores so a renamed key does not break the plugin.
  function authToken() {
    try {
      const stores = [localStorage, sessionStorage];
      const keys = ['userToken', '__appKit_userInfo'];
      for (let s = 0; s < stores.length; s++) {
        for (let k = 0; k < keys.length; k++) {
          const found = tokenFromRaw(stores[s].getItem(keys[k]));
          if (found) return found;
        }
      }
      for (let s = 0; s < stores.length; s++) {
        const store = stores[s];
        for (let i = 0; i < store.length; i++) {
          const found = tokenFromRaw(store.getItem(store.key(i)));
          if (found) return found;
        }
      }
    } catch (e) {}
    return null;
  }
  function apiHeaders(extra) {
    const headers = Object.assign({ 'content-type': 'application/json' }, extra || {});
    const token = authToken();
    if (token) headers.authorization = 'Bearer ' + token;
    return headers;
  }
  async function api(path, init) {
    init = init || {};
    const opts = Object.assign({}, init);
    opts.credentials = 'include';
    opts.headers = apiHeaders(init.headers);
    const response = await fetch(path, opts);
    const raw = await response.text();
    let data = null;
    try { data = raw ? JSON.parse(raw) : null; } catch (e) {}
    return { status: response.status, data: data, raw: raw };
  }
  function bizOf(response) {
    const top = response && response.data;
    if (!top) {
      throw new Error('empty API response (HTTP ' + (response ? response.status : '?') + ')');
    }
    if (top.code !== 0) {
      throw new Error('API code ' + top.code + ': ' + (top.msg || ''));
    }
    const inner = top.data || {};
    if (inner.biz_code !== undefined && inner.biz_code !== 0) {
      throw new Error('biz_code ' + inner.biz_code + ': ' + (inner.biz_msg || ''));
    }
    return inner.biz_data || {};
  }
  function finish(result) {
    S.result = result;
    S.phase = 'done';
    S.done = true;
    S.running = false;
  }
  function fail(error) {
    S.error = String(error && error.message ? error.message : error);
    S.phase = 'error';
    S.done = true;
    S.running = false;
  }
  async function waitFor(fn, ms) {
    const until = Date.now() + ms;
    while (Date.now() < until) {
      const value = fn();
      if (value) return value;
      if (!alive()) throw new Error('cancelled');
      await sleep(250);
    }
    return null;
  }
  function collectMessages() {
    const out = [];
    const items = Array.prototype.slice.call(document.querySelectorAll('[data-virtual-list-item-key]'));
    for (const item of items) {
      const assistant = item.querySelector('.ds-assistant-message-main-content');
      const text = txt(assistant || item);
      if (!text) continue;
      out.push({ id: item.getAttribute('data-virtual-list-item-key') || '', role: assistant ? 'assistant' : 'user', text: text, status: 'FINISHED' });
    }
    return out;
  }
  async function runChat() {
    S.phase = 'waiting-for-input';
    if (looksSignedOut()) throw new Error('session is not signed in to DeepSeek');
    const found = await waitFor(
      () => inputEl() ? 'input' : (looksSignedOut() ? 'signin' : null),
      15000);
    if (found === 'signin') throw new Error('session is not signed in to DeepSeek');
    if (!found) throw new Error('chat input not found — sign in to DeepSeek in this session first');
    let input = inputEl();

    if (CFG.newChat && chatIdFromUrl()) {
      // Some sessions restore the last chat on the root URL; start a clean one.
      const labels = ['new chat', 'new conversation', '新对话', 'новый чат', 'nuevo chat', 'nouveau chat', 'neuer chat', 'nuova chat'];
      const fresh = buttons(document).find((b) => labels.indexOf(norm(txt(b))) >= 0);
      if (fresh) {
        fresh.click();
        await sleep(700);
        if (!alive()) throw new Error('cancelled');
        input = inputEl() || input;
      }
    }

    // Optional per-request composer switches; applied before typing so the
    // app's store already holds them when the prompt is submitted.
    if (CFG.thinking != null || CFG.search != null) {
      S.phase = 'toggling';
      if (CFG.thinking != null) await setToggle('thinking', !!CFG.thinking);
      if (CFG.search != null) await setToggle('search', !!CFG.search);
      await sleep(200);
      if (!alive()) throw new Error('cancelled');
    }
    // Snapshot what the composer is actually sending with this prompt.
    const thinkingState = toggleState('thinking');
    const searchState = toggleState('search');

    const initialNode = lastAssistant();
    const initialText = txt(initialNode);
    const initialCount = assistantNodes().length;

    S.phase = 'typing';
    input.focus();
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value');
    if (setter && setter.set) setter.set.call(input, CFG.prompt); else input.value = CFG.prompt;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    await sleep(150);

    S.phase = 'submitting';
    const submit = () => {
      input.focus();
      input.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true
      }));
    };
    submit();

    let targetSeen = false;
    const submitDeadline = Date.now() + 8000;
    while (Date.now() < submitDeadline) {
      await sleep(400);
      if (!alive()) throw new Error('cancelled');
      const node = lastAssistant();
      if (node && (node !== initialNode || txt(node) !== initialText || assistantNodes().length > initialCount)) {
        targetSeen = true;
        break;
      }
      if (input.value && input.value !== CFG.prompt) break;
    }

    if (!targetSeen && (input.value || '').trim() && !isGenerating(input)) {
      let scope = input;
      for (let i = 0; i < 8 && scope; i++, scope = scope.parentElement) {
        const candidates = buttons(scope).filter((b) => !b.disabled && b.getAttribute('aria-disabled') !== 'true');
        const button = candidates.length ? candidates[candidates.length - 1] : null;
        if (button) { button.click(); break; }
      }
      await sleep(800);
    }

    if (!targetSeen) {
      // Give the page a bounded window to start rendering the answer. A
      // signed-out or broken composer would otherwise burn the full timeout.
      let generationActive = false;
      const noTargetDeadline = Date.now() + 25000;
      while (Date.now() < noTargetDeadline) {
        await sleep(500);
        if (!alive()) throw new Error('cancelled');
        const node = lastAssistant();
        if (node && (node !== initialNode || txt(node) !== initialText || assistantNodes().length > initialCount)) {
          targetSeen = true;
          break;
        }
        if (isGenerating(input)) generationActive = true;
      }
      if (!targetSeen && !generationActive) {
        throw new Error('the prompt was not sent — sign in to DeepSeek in this session first');
      }
    }

    S.phase = 'generating';
    let lastObserved = '';
    let lastChange = Date.now();
    let sawTarget = targetSeen;
    let continues = 0;
    let continueFails = 0;
    let lastContinueAt = 0;

    while (Date.now() - S.startedAt < CFG.timeoutMs) {
      if (!alive()) throw new Error('cancelled');
      const node = lastAssistant();
      const item = node ? (itemOf(node) || node) : null;
      const observed = item ? txt(item) : '';
      const main = node ? txt(node) : '';
      const isNew = !!node && (node !== initialNode || main !== initialText || assistantNodes().length > initialCount);
      if (isNew) sawTarget = true;
      if (sawTarget && observed !== lastObserved) {
        lastObserved = observed;
        lastChange = Date.now();
      }

      const continueButton = sawTarget ? findContinue(node) : null;
      const continueEnabled = continueButton
        && !continueButton.disabled
        && continueButton.getAttribute('aria-disabled') !== 'true';
      if (continueEnabled && Date.now() - lastContinueAt > 2500) {
        S.phase = 'continuing';
        if (reactClick(continueButton)) {
          continues += 1;
          continueFails = 0;
          S.log.push('continue clicked (' + continues + ')');
        } else {
          continueFails += 1;
          S.log.push('continue button has no React onClick');
          if (continueFails >= 3) throw new Error('could not press the Continue button');
        }
        if (continues > CFG.maxContinues) throw new Error('max continues exceeded');
        lastContinueAt = Date.now();
        lastChange = Date.now();
        await sleep(800);
        S.phase = 'generating';
        continue;
      }

      const idle = Date.now() - lastChange;
      if (
        sawTarget && main && !continueEnabled &&
        !isGenerating(input) && !hasStopButton() &&
        idle >= CFG.settleMs
      ) {
        // Confirm on a second sample so a short pause mid-stream does not end
        // the job with a truncated answer.
        await sleep(500);
        if (!alive()) throw new Error('cancelled');
        const settledNode = lastAssistant();
        const settledText = txt(settledNode);
        if (
          settledText && settledText === main &&
          !isGenerating(input) &&
          !findContinue(settledNode)
        ) {
          finish({
            chat_id: chatIdFromUrl() || '',
            chat_url: location.href,
            answer: settledText,
            continued: continues,
            thinking_enabled: thinkingState,
            search_enabled: searchState,
            messages: collectMessages()
          });
          return;
        }
        lastChange = Date.now();
      }
      await sleep(400);
    }
    throw new Error('timed out waiting for the DeepSeek answer');
  }
  async function runList() {
    requireSignedIn();
    S.phase = 'listing';
    const chats = [];
    const seen = {};
    const cursor = Math.floor(Date.now() / 1000) + 60;
    let firstError = null;
    for (const pinned of [true, false]) {
      try {
        const response = await api(
          '/api/v0/chat_session/fetch_page?count=' + CFG.limit +
          '&lte_cursor.pinned=' + pinned + '&lte_cursor.updated_at=' + cursor,
          { method: 'GET' });
        const data = bizOf(response);
        const sessions = data.chat_sessions || [];
        for (const chat of sessions) {
          if (!chat || !chat.id || seen[chat.id]) continue;
          seen[chat.id] = true;
          chats.push({
            id: chat.id,
            title: chat.title || '',
            pinned: !!chat.pinned,
            updated_at: chat.updated_at || 0,
            url: location.origin + '/a/chat/s/' + chat.id
          });
        }
      } catch (error) {
        if (!firstError) firstError = error;
        S.log.push('list pinned=' + pinned + ': ' + String(error));
      }
    }
    if (!chats.length && firstError) throw firstError;
    chats.sort((a, b) => {
      if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
      return (b.updated_at || 0) - (a.updated_at || 0);
    });
    finish({ chats: chats });
  }
  async function runCreate() {
    requireSignedIn();
    S.phase = 'creating';
    const response = await api('/api/v0/chat_session/create', { method: 'POST', body: '{}' });
    const data = bizOf(response);
    const session = data.chat_session || {};
    if (!session.id) throw new Error('create response has no chat_session.id');
    if (CFG.title) {
      const renamed = await api('/api/v0/chat_session/update_title', {
        method: 'POST',
        body: JSON.stringify({ chat_session_id: session.id, title: CFG.title })
      });
      bizOf(renamed);
    }
    finish({
      id: session.id,
      title: CFG.title || session.title || '',
      url: location.origin + '/a/chat/s/' + session.id
    });
  }
  async function runDelete() {
    requireSignedIn();
    S.phase = 'deleting';
    const response = await api('/api/v0/chat_session/delete', {
      method: 'POST',
      body: JSON.stringify({ chat_session_ids: CFG.chatIds })
    });
    bizOf(response);
    finish({ ok: true, deleted: CFG.chatIds });
  }
  async function runRename() {
    requireSignedIn();
    S.phase = 'renaming';
    const response = await api('/api/v0/chat_session/update_title', {
      method: 'POST',
      body: JSON.stringify({ chat_session_id: CFG.chatId, title: CFG.title })
    });
    bizOf(response);
    finish({ id: CFG.chatId, title: CFG.title, url: location.origin + '/a/chat/s/' + CFG.chatId });
  }
  async function runHistory() {
    requireSignedIn();
    S.phase = 'history';
    const response = await api(
      '/api/v0/chat/history_messages?chat_session_id=' + encodeURIComponent(CFG.chatId),
      { method: 'GET' });
    const data = bizOf(response);
    const raw = data.chat_messages || [];
    const messages = raw.map((m) => ({
      id: m.message_id || '',
      role: (m.role || '').toLowerCase(),
      text: m.content || '',
      status: m.status || ''
    }));
    finish({ chat_id: CFG.chatId, messages: messages });
  }
  async function runStatus() {
    finish({
      logged_in: !/\/sign_in/.test(location.pathname) && !!inputEl(),
      url: location.href,
      chat_id: chatIdFromUrl(),
      token_found: !!authToken()
    });
  }"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_req(prompt: &str) -> DeepSeekChatRequest {
        DeepSeekChatRequest {
            prompt: prompt.into(),
            chat_id: None,
            timeout_ms: None,
            settle_ms: None,
            max_continues: None,
            thinking: None,
            search: None,
        }
    }

    #[test]
    fn validates_prompt_and_chat_ids() {
        assert!(chat_req("  ").validate().is_err());
        assert!(chat_req(&"x".repeat(MAX_PROMPT_CHARS + 1)).validate().is_err());

        let job = DeepSeekChatRequest {
            chat_id: Some("abc-123_XY".into()),
            ..chat_req("hello")
        }
        .validate()
        .unwrap();
        assert_eq!(job.page_url.as_str(), "https://chat.deepseek.com/a/chat/s/abc-123_XY");

        let bad = DeepSeekChatRequest {
            chat_id: Some("../etc".into()),
            ..chat_req("hello")
        };
        assert!(bad.validate().is_err());

        let root = chat_req("hello").validate().unwrap();
        assert_eq!(root.page_url.as_str(), DEEPSEEK_ROOT);
        assert_eq!(root.timeout, Duration::from_millis(DEFAULT_DEEPSEEK_TIMEOUT_MS));
        assert_eq!(root.settle_ms, DEFAULT_DEEPSEEK_SETTLE_MS);
        assert_eq!(root.max_continues, 10);
    }

    #[test]
    fn clamps_limits() {
        let job = DeepSeekChatRequest {
            timeout_ms: Some(u64::MAX),
            settle_ms: Some(1),
            max_continues: Some(999),
            ..chat_req("hello")
        }
        .validate()
        .unwrap();
        assert_eq!(job.timeout, Duration::from_millis(MAX_DEEPSEEK_TIMEOUT_MS));
        assert_eq!(job.settle_ms, 500);
        assert_eq!(job.max_continues, 50);
        assert_eq!(DeepSeekChatsQuery { limit: Some(999) }.validate().unwrap(), 200);
        assert_eq!(DeepSeekChatsQuery { limit: None }.validate().unwrap(), 50);
    }

    #[test]
    fn chat_switches_reach_the_page() {
        let job = DeepSeekChatRequest {
            thinking: Some(true),
            search: Some(false),
            ..chat_req("hello")
        }
        .validate()
        .unwrap();
        assert_eq!(job.thinking, Some(true));
        assert_eq!(job.search, Some(false));
        let script = DeepSeekJob::Chat(job).setup_script();
        assert!(script.contains(r#""thinking":true"#));
        assert!(script.contains(r#""search":false"#));
        assert!(script.contains("ds-toggle-button"), "toggle driver missing");

        // Omitted switches leave the composer as the user left it.
        let plain = chat_req("hello").validate().unwrap();
        assert_eq!(plain.thinking, None);
        let script = DeepSeekJob::Chat(plain).setup_script();
        assert!(script.contains(r#""thinking":null"#));
    }

    #[test]
    fn chat_request_accepts_switch_aliases() {
        let request: DeepSeekChatRequest = serde_json::from_str(
            r#"{"prompt":"hi","deep_thinking":true,"smart_search":true}"#,
        )
        .unwrap();
        assert_eq!(request.thinking, Some(true));
        assert_eq!(request.search, Some(true));
    }

    #[test]
    fn delete_and_rename_validation() {
        assert!(DeepSeekDeleteChatsRequest { chat_ids: vec![] }.validate().is_err());
        assert!(DeepSeekDeleteChatsRequest {
            chat_ids: vec!["../bad".into()]
        }
        .validate()
        .is_err());
        assert_eq!(
            DeepSeekDeleteChatsRequest {
                chat_ids: vec!["a-1".into(), "b_2".into()]
            }
            .validate()
            .unwrap(),
            vec!["a-1", "b_2"]
        );
        assert!(DeepSeekRenameChatRequest { title: "  ".into() }.validate().is_err());
        assert_eq!(
            DeepSeekRenameChatRequest { title: " Plan ".into() }
                .validate()
                .unwrap(),
            "Plan"
        );
    }

    #[test]
    fn scripts_quote_user_input() {
        let job = DeepSeekChatRequest {
            prompt: "line\n'); alert(1); ('\"quote".into(),
            ..chat_req("unused")
        }
        .validate()
        .unwrap();
        let script = DeepSeekJob::Chat(job).setup_script();
        assert!(script.contains(r#"alert(1)"#), "prompt must be embedded as JSON");
        assert!(!script.contains("CFG.prompt = '"), "no raw prompt concatenation");

        let status = DeepSeekJob::Status.setup_script();
        assert!(status.contains(r#""op":"status""#));
        assert!(DeepSeekJob::Status.matches_tab(
            &Url::parse("https://chat.deepseek.com/sign_in").unwrap()
        ));
        assert!(!DeepSeekJob::Status.matches_tab(
            &Url::parse("https://example.com/").unwrap()
        ));
    }

    #[test]
    fn new_chat_does_not_reuse_an_open_chat_tab() {
        let root = Url::parse("https://chat.deepseek.com/").unwrap();
        let chat_page = Url::parse("https://chat.deepseek.com/a/chat/s/abc").unwrap();

        let new_chat = DeepSeekJob::Chat(chat_req("hi").validate().unwrap());
        assert!(new_chat.matches_tab(&root));
        assert!(!new_chat.matches_tab(&chat_page));

        let existing = DeepSeekJob::Chat(
            DeepSeekChatRequest {
                chat_id: Some("abc".into()),
                ..chat_req("hi")
            }
            .validate()
            .unwrap(),
        );
        assert!(existing.matches_tab(&chat_page));
        assert!(!existing.matches_tab(&root));

        // API-only operations reuse whichever page is open.
        assert!(DeepSeekJob::Status.matches_tab(&chat_page));
        assert!(DeepSeekJob::ListChats { limit: 10 }.matches_tab(&chat_page));
    }

    #[test]
    fn script_uses_site_storage_keys_and_react_continue() {
        let job = chat_req("hello").validate().unwrap();
        let script = DeepSeekJob::Chat(job).setup_script();
        // Bearer token lookup prefers the keys the web app writes.
        assert!(script.contains("'userToken'"));
        assert!(script.contains("'__appKit_userInfo'"));
        // Continue is pressed through the button's React handler.
        assert!(script.contains("__reactProps$"));
        assert!(script.contains("nativeEvent"));
        // The answer is read from the assistant message body.
        assert!(script.contains(".ds-assistant-message-main-content"));
        assert!(script.contains("lte_cursor.updated_at"));
        assert!(script.contains("chat_session/fetch_page"));
        // A page reload is reported instead of polling until the timeout.
        assert!(crate::deepseek::deepseek_poll_script().contains("sessionStorage"));
    }

    #[test]
    fn parses_plugin_results() {
        let value = json!({
            "chats": [
                { "id": "a", "title": "One", "pinned": true, "updated_at": 10.0, "url": "https://chat.deepseek.com/a/chat/s/a" },
                { "id": "b", "title": "Two", "pinned": false, "updated_at": 5 }
            ]
        });
        let chats = parse_chat_list(&value).unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].id, "a");
        assert!(chats[0].pinned);
        assert_eq!(chats[1].updated_at, 5);
        assert!(chats[1].url.ends_with("/a/chat/s/b"));

        let messages = parse_messages(&json!({
            "messages": [
                { "id": "m1", "role": "ASSISTANT", "text": "hi", "status": "FINISHED" }
            ]
        }))
        .unwrap();
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[0].text, "hi");
    }
}
