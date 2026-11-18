//! Headless automation server: a local HTTP JSON API that renders pages in
//! the Servo Content Process and returns DOM, script results and screenshots.
//!
//! ```text
//! HTTP client (BoardDo, curl)
//!     ↓  POST /render, GET /health, /sessions/…
//! axum (tokio)  ──mpsc──▶  one Actor thread per session
//!                              ↓  Unix IPC (protocol v5)
//!                          rust-browser --content-process --automation-content
//! ```
//!
//! The content process runs with `--automation-content`: no frame streaming and
//! no requests to localhost / private networks. Requests for such URLs are
//! rejected up front as well.
//!
//! ## Sessions
//!
//! Every session owns one Content Process and one stable profile directory
//! (`<data>/content` for `default`, `<data>/sessions/<id>/content` otherwise).
//! That makes multiple signed-in accounts possible: open a visible window once
//! with `POST /sessions/{id}/login/open`, sign in by hand (Google, reCAPTCHA),
//! close it, and the headless session picks up the saved cookies. Requests can
//! then run inside the page (`POST /sessions/{id}/fetch`) or as raw HTTP with
//! the exported cookies (`POST /sessions/{id}/request`).

mod actor;
mod api;
mod deepseek;
mod sessions;

pub use actor::{Actor, Command, ContentDriver, DeepSeekReply};
pub use api::{
    CookieListResponse, CookieQuery, CreateSessionRequest, Health, HttpRequestSpec, HttpResult,
    LoginOpenRequest, LoginOpenResponse, PageFetchRequest, PageFetchResponse, RenderError,
    RenderJob, RenderRequest, RenderResponse, SessionSummary, SetCookiesRequest, DEFAULT_SESSION_ID,
};
pub use deepseek::{
    DeepSeekChatRequest, DeepSeekChatSummary, DeepSeekChatsQuery, DeepSeekCreateChatRequest,
    DeepSeekDeleteChatsRequest, DeepSeekJob, DeepSeekMessage, DeepSeekRenameChatRequest,
};
pub use sessions::{SessionEntry, SessionRegistry, SessionSpawner, SessionSpec};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use browser_ipc::CookieInfo;
use serde_json::json;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tracing::{error, info};
use url::Url;

use api::{
    validate_session_id, validate_target_url, ValidatedHttpRequest, MAX_HTTP_RESPONSE_BYTES,
};

/// Flag passed to content processes spawned for automation.
pub const AUTOMATION_CONTENT_FLAG: &str = "--automation-content";

/// Time allowed for a session actor to answer cookie / health requests.
const SESSION_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// Extra budget on top of a request's own timeout for IPC / scheduling.
const RESPONSE_SLACK: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct AutomationConfig {
    pub addr: SocketAddr,
    /// Required as `Authorization: Bearer <token>` when set (mandatory off-loopback).
    pub token: Option<String>,
    /// The `rust-browser` binary used to spawn content processes.
    pub exe: PathBuf,
    /// Automation data root: profiles (cookies, cache) live here, never the user's profile.
    pub data_dir: PathBuf,
    /// Short directory for Unix sockets (macOS `sun_path` limit).
    pub socket_dir: PathBuf,
    pub host_file: Option<PathBuf>,
    pub max_tabs: usize,
    pub max_queue: usize,
    /// Shut down when stdin reaches EOF: a supervising parent (BoardDo) keeps the
    /// pipe open, so the browser never outlives it, even after a crash or `kill -9`.
    pub exit_on_stdin_eof: bool,
}

impl AutomationConfig {
    fn session_spec(&self) -> SessionSpec {
        SessionSpec {
            exe: self.exe.clone(),
            data_root: self.data_dir.clone(),
            sessions_root: self.data_dir.join("sessions"),
            socket_root: self.socket_dir.clone(),
            host_file: self.host_file.clone(),
            max_tabs: self.max_tabs,
            max_queue: self.max_queue,
        }
    }
}

struct AppState {
    sessions: Arc<SessionRegistry>,
    token: Option<String>,
}

/// Start the default content process, then serve HTTP until Ctrl-C.
pub fn run(config: AutomationConfig) -> Result<(), String> {
    if !config.addr.ip().is_loopback() && config.token.is_none() {
        return Err(format!(
            "refusing to listen on {} without --automation-token",
            config.addr
        ));
    }
    let sessions = Arc::new(SessionRegistry::new(config.session_spec()));
    // Fail fast on a broken install, like the single-session server did.
    sessions.ensure_blocking(DEFAULT_SESSION_ID)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let app = router(Arc::clone(&sessions), config.token.clone());
    let parent_gone = config.exit_on_stdin_eof.then(watch_stdin_eof);
    let served = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.addr)
            .await
            .map_err(|e| format!("bind {}: {e}", config.addr))?;
        info!(addr = %config.addr, "automation server listening");
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                match parent_gone {
                    Some(eof) => {
                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = eof => info!("stdin closed — parent process is gone"),
                        }
                    }
                    None => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
                info!("automation server shutting down");
            })
            .await
            .map_err(|e| format!("serve: {e}"))
    });
    // Flush every profile (Servo writes `cookie_jar.json` on graceful exit).
    sessions.shutdown_all_blocking();
    served
}

/// Resolves once stdin hits EOF (or errors). Reads on a plain thread: stdin is blocking.
fn watch_stdin_eof() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("automation-stdin".into())
        .spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 256];
            let mut stdin = std::io::stdin();
            while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
            let _ = tx.send(());
        })
        .expect("spawn stdin watcher");
    rx
}

/// HTTP routes; commands go to per-session actor threads.
pub fn router(sessions: Arc<SessionRegistry>, token: Option<String>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/render", post(render))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", axum::routing::delete(delete_session))
        .route("/sessions/{id}/login/open", post(login_open))
        .route(
            "/sessions/{id}/cookies",
            get(get_cookies).put(set_cookies).post(set_cookies),
        )
        .route("/sessions/{id}/request", post(http_request))
        .route("/sessions/{id}/fetch", post(page_fetch))
        .route("/sessions/{id}/deepseek/chat", post(deepseek_chat))
        .route("/sessions/{id}/deepseek/status", get(deepseek_status))
        .route(
            "/sessions/{id}/deepseek/chats",
            get(deepseek_list_chats)
                .post(deepseek_create_chat)
                .delete(deepseek_delete_chats),
        )
        .route(
            "/sessions/{id}/deepseek/chats/{chat_id}",
            get(deepseek_history),
        )
        .route(
            "/sessions/{id}/deepseek/chats/{chat_id}/rename",
            post(deepseek_rename_chat),
        )
        .with_state(Arc::new(AppState { sessions, token }))
}

fn authorized(headers: &HeaderMap, token: &Option<String>) -> bool {
    let Some(expected) = token else {
        return true;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|got| constant_time_eq(got.trim().as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn error_response(status: u16, message: String) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({ "error": message }))).into_response()
}

/// Resolve a session, spawning its content process on first use.
async fn session_tx(state: &AppState, id: &str) -> Result<mpsc::Sender<Command>, Response> {
    if let Err(e) = validate_session_id(id) {
        return Err(error_response(e.status(), e.message()));
    }
    state
        .sessions
        .ensure(id)
        .await
        .map_err(|e| error_response(502, format!("session `{id}`: {e}")))
}

async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let tx = match session_tx(&state, DEFAULT_SESSION_ID).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let (reply, rx) = oneshot::channel();
    if tx.send(Command::Health { reply }).is_err() {
        return error_response(503, "automation actor is not running".into());
    }
    match tokio::time::timeout(Duration::from_secs(5), rx).await {
        Ok(Ok(health)) => Json(health).into_response(),
        _ => error_response(503, "automation actor did not answer".into()),
    }
}

async fn render(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<RenderRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let job = match request.validate() {
        Ok(job) => job,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let tx = match session_tx(&state, DEFAULT_SESSION_ID).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let budget = job.timeout + RESPONSE_SLACK;
    let (reply, rx) = oneshot::channel();
    if tx.send(Command::Render { job, reply }).is_err() {
        return error_response(503, "automation actor is not running".into());
    }
    match tokio::time::timeout(budget, rx).await {
        Ok(Ok(Ok(resp))) => Json(resp).into_response(),
        Ok(Ok(Err(e))) => error_response(e.status(), e.message()),
        Ok(Err(_)) => {
            error!("automation actor dropped a render job");
            error_response(502, "render job was dropped".into())
        }
        Err(_) => error_response(504, "timeout: render job did not finish".into()),
    }
}

async fn list_sessions(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    Json(json!({ "sessions": state.sessions.list() })).into_response()
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<CreateSessionRequest>>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let id = match body
        .map(|Json(request)| request)
        .unwrap_or(CreateSessionRequest { id: None })
        .validate()
    {
        Ok(id) => id,
        Err(e) => return error_response(e.status(), e.message()),
    };
    if let Err(resp) = session_tx(&state, &id).await {
        return resp;
    }
    let summary = SessionSummary {
        content_dir: state.sessions.content_dir(&id).display().to_string(),
        id,
    };
    Json(summary).into_response()
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let sessions = Arc::clone(&state.sessions);
    let stop_id = id.clone();
    match tokio::task::spawn_blocking(move || sessions.stop_blocking(&stop_id)).await {
        Ok(Ok(())) => Json(json!({ "stopped": id })).into_response(),
        Ok(Err(e)) => error_response(500, e),
        Err(e) => error_response(500, format!("session stop task: {e}")),
    }
}

async fn login_open(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<LoginOpenRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let (url, timeout) = match request.validate() {
        Ok(valid) => valid,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let sessions = Arc::clone(&state.sessions);
    let login_id = id.clone();
    let budget = timeout + Duration::from_secs(60);
    let task = tokio::task::spawn_blocking(move || {
        sessions.login_blocking(&login_id, &url, timeout)
    });
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(Ok(resp))) => Json(resp).into_response(),
        Ok(Ok(Err(e))) => error_response(502, format!("login: {e}")),
        Ok(Err(e)) => error_response(500, format!("login task: {e}")),
        Err(_) => error_response(
            504,
            "timeout: login window did not close".into(),
        ),
    }
}

async fn get_cookies(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<CookieQuery>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let url = match validate_target_url(&query.url) {
        Ok(url) => url,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let tx = match session_tx(&state, &id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    match export_cookies(&tx, url).await {
        Ok(cookies) => Json(CookieListResponse {
            url: query.url,
            cookies,
        })
        .into_response(),
        Err(resp) => resp,
    }
}

async fn set_cookies(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<SetCookiesRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let (url, cookies) = match request.validate() {
        Ok(valid) => valid,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let tx = match session_tx(&state, &id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let (reply, rx) = oneshot::channel();
    if tx
        .send(Command::SetCookies { url, cookies, reply })
        .is_err()
    {
        return error_response(503, "session actor is not running".into());
    }
    match tokio::time::timeout(SESSION_REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok(()))) => Json(json!({ "ok": true })).into_response(),
        Ok(Ok(Err(e))) => error_response(502, e),
        Ok(Err(_)) => error_response(502, "session dropped the cookie request".into()),
        Err(_) => error_response(504, "timeout: cookie import did not finish".into()),
    }
}

/// Raw HTTP with the session's cookies attached (cookies stay out of the page).
async fn http_request(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<HttpRequestSpec>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let request = match request.validate() {
        Ok(valid) => valid,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let tx = match session_tx(&state, &id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let cookies = match export_cookies(&tx, request.url.clone()).await {
        Ok(cookies) => cookies,
        Err(resp) => return resp,
    };
    let budget = request.timeout + RESPONSE_SLACK;
    let task = tokio::task::spawn_blocking(move || execute_http(request, cookies));
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(Ok(result))) => Json(result).into_response(),
        Ok(Ok(Err(e))) => error_response(502, format!("request: {e}")),
        Ok(Err(e)) => error_response(500, format!("request task: {e}")),
        Err(_) => error_response(504, "timeout: request did not finish".into()),
    }
}

async fn page_fetch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<PageFetchRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let job = match request.validate() {
        Ok(job) => job,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let tx = match session_tx(&state, &id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let budget = job.timeout + RESPONSE_SLACK;
    let (reply, rx) = oneshot::channel();
    if tx.send(Command::Fetch { job, reply }).is_err() {
        return error_response(503, "session actor is not running".into());
    }
    match tokio::time::timeout(budget, rx).await {
        Ok(Ok(Ok(resp))) => Json(resp).into_response(),
        Ok(Ok(Err(e))) => error_response(e.status(), e.message()),
        Ok(Err(_)) => error_response(502, "session fetch was dropped".into()),
        Err(_) => error_response(504, "timeout: page fetch did not finish".into()),
    }
}

/// Run a DeepSeek plugin job on the session actor and await its result.
async fn run_deepseek(
    state: &Arc<AppState>,
    id: &str,
    job: DeepSeekJob,
) -> Result<serde_json::Value, Response> {
    let tx = session_tx(state, id).await?;
    let budget = job.timeout() + RESPONSE_SLACK;
    let (reply, rx) = oneshot::channel();
    if tx.send(Command::DeepSeek { job, reply }).is_err() {
        return Err(error_response(503, "session actor is not running".into()));
    }
    match tokio::time::timeout(budget, rx).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(e))) => Err(error_response(e.status(), e.message())),
        Ok(Err(_)) => Err(error_response(502, "deepseek job was dropped".into())),
        Err(_) => Err(error_response(504, "timeout: deepseek job did not finish".into())),
    }
}

/// `POST /sessions/{id}/deepseek/chat` — send a prompt and wait for the answer.
async fn deepseek_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DeepSeekChatRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let job = match request.validate() {
        Ok(job) => job,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::Chat(job)).await {
        Ok(result) => Json(result).into_response(),
        Err(resp) => resp,
    }
}

/// `GET /sessions/{id}/deepseek/status` — is the session signed in?
async fn deepseek_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    match run_deepseek(&state, &id, DeepSeekJob::Status).await {
        Ok(result) => Json(result).into_response(),
        Err(resp) => resp,
    }
}

/// `GET /sessions/{id}/deepseek/chats` — list chats from the sidebar API.
async fn deepseek_list_chats(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeepSeekChatsQuery>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let limit = match query.validate() {
        Ok(limit) => limit,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::ListChats { limit }).await {
        Ok(result) => match deepseek::parse_chat_list(&result) {
            Ok(chats) => Json(json!({ "chats": chats })).into_response(),
            Err(e) => error_response(502, e),
        },
        Err(resp) => resp,
    }
}

/// `POST /sessions/{id}/deepseek/chats` — create a chat (optionally titled).
async fn deepseek_create_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DeepSeekCreateChatRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let title = match request.validate() {
        Ok(title) => title,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::CreateChat { title }).await {
        Ok(result) => Json(result).into_response(),
        Err(resp) => resp,
    }
}

/// `DELETE /sessions/{id}/deepseek/chats` — delete one or more chats.
async fn deepseek_delete_chats(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DeepSeekDeleteChatsRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let chat_ids = match request.validate() {
        Ok(ids) => ids,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::DeleteChats { chat_ids }).await {
        Ok(result) => Json(result).into_response(),
        Err(resp) => resp,
    }
}

/// `GET /sessions/{id}/deepseek/chats/{chat_id}` — chat history.
async fn deepseek_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, chat_id)): Path<(String, String)>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let chat_id = match deepseek::validate_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::History { chat_id }).await {
        Ok(result) => match deepseek::parse_messages(&result) {
            Ok(messages) => Json(json!({
                "chat_id": result.get("chat_id").cloned().unwrap_or_default(),
                "messages": messages,
            }))
            .into_response(),
            Err(e) => error_response(502, e),
        },
        Err(resp) => resp,
    }
}

/// `POST /sessions/{id}/deepseek/chats/{chat_id}/rename`
async fn deepseek_rename_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, chat_id)): Path<(String, String)>,
    Json(request): Json<DeepSeekRenameChatRequest>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    if let Err(e) = validate_session_id(&id) {
        return error_response(e.status(), e.message());
    }
    let chat_id = match deepseek::validate_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(e) => return error_response(e.status(), e.message()),
    };
    let title = match request.validate() {
        Ok(title) => title,
        Err(e) => return error_response(e.status(), e.message()),
    };
    match run_deepseek(&state, &id, DeepSeekJob::RenameChat { chat_id, title }).await {
        Ok(result) => Json(result).into_response(),
        Err(resp) => resp,
    }
}

async fn export_cookies(
    tx: &mpsc::Sender<Command>,
    url: Url,
) -> Result<Vec<CookieInfo>, Response> {
    let (reply, rx) = oneshot::channel();
    if tx.send(Command::GetCookies { url, reply }).is_err() {
        return Err(error_response(503, "session actor is not running".into()));
    }
    match tokio::time::timeout(SESSION_REPLY_TIMEOUT, rx).await {
        Ok(Ok(Ok(cookies))) => Ok(cookies),
        Ok(Ok(Err(e))) => Err(error_response(502, format!("cookies: {e}"))),
        Ok(Err(_)) => Err(error_response(502, "session dropped the cookie request".into())),
        Err(_) => Err(error_response(
            504,
            "timeout: cookie export did not finish".into(),
        )),
    }
}

/// Execute one request outside the async runtime (`reqwest::blocking`).
fn execute_http(
    request: ValidatedHttpRequest,
    cookies: Vec<CookieInfo>,
) -> Result<HttpResult, String> {
    let started = Instant::now();
    let mut builder = reqwest::blocking::Client::builder().timeout(request.timeout);
    if !request.follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    let client = builder.build().map_err(|e| e.to_string())?;

    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|e| format!("method {}: {e}", request.method))?;
    let mut outgoing = client.request(method, request.url.as_str());
    let has_cookie_header = request
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("cookie"));
    for (name, value) in &request.headers {
        outgoing = outgoing.header(name.as_str(), value.as_str());
    }
    if !has_cookie_header {
        let header = cookie_header(&request.url, &cookies);
        if !header.is_empty() {
            outgoing = outgoing.header(header::COOKIE, header);
        }
    }
    if !request.body.is_empty() {
        outgoing = outgoing.body(request.body);
    }

    let response = outgoing.send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let mut response_headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in response.headers() {
        let name = name.as_str().to_string();
        let value = value.to_str().unwrap_or_default().to_string();
        response_headers
            .entry(name)
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(&value);
            })
            .or_insert(value);
    }

    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let body_length = bytes.len();
    let truncated = body_length > MAX_HTTP_RESPONSE_BYTES;
    let slice = &bytes[..body_length.min(MAX_HTTP_RESPONSE_BYTES)];
    let textual = response_headers
        .get("content-type")
        .map(|value| is_textual_content_type(value))
        .unwrap_or_else(|| std::str::from_utf8(slice).is_ok());
    let (body, body_base64) = if textual {
        (Some(String::from_utf8_lossy(slice).into_owned()), None)
    } else {
        (
            None,
            Some(base64::engine::general_purpose::STANDARD.encode(slice)),
        )
    };

    Ok(HttpResult {
        url: final_url,
        status,
        headers: response_headers,
        body,
        body_base64,
        body_length,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn is_textual_content_type(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("text/")
        || value.contains("json")
        || value.contains("xml")
        || value.contains("javascript")
        || value.contains("x-www-form-urlencoded")
}

/// `Cookie` header for the request URL, filtered by domain/path/secure/expiry.
fn cookie_header(url: &Url, cookies: &[CookieInfo]) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let host = url.host_str().unwrap_or_default();
    let path = url.path();
    cookies
        .iter()
        .filter(|cookie| !cookie.name.is_empty())
        .filter(|cookie| cookie.expires_unix.map_or(true, |expires| expires > now))
        .filter(|cookie| !cookie.secure || url.scheme() == "https")
        .filter(|cookie| match &cookie.domain {
            None => true,
            Some(domain) => {
                let domain = domain.trim_start_matches('.');
                host == domain || host.ends_with(&format!(".{domain}"))
            }
        })
        .filter(|cookie| match &cookie.path {
            None => true,
            Some(cookie_path) => path.starts_with(cookie_path.as_str()),
        })
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::path::PathBuf;
    use tower::ServiceExt;

    /// Spawns in-process actors that answer like the real ones; no Servo needed.
    struct FakeSpawner;

    impl SessionSpawner for FakeSpawner {
        fn spawn(&self, id: &str) -> Result<SessionEntry, String> {
            let (tx, rx) = mpsc::channel::<Command>();
            let thread = std::thread::Builder::new()
                .name(format!("fake-automation-{id}"))
                .spawn(move || {
                    while let Ok(cmd) = rx.recv() {
                        match cmd {
                            Command::Health { reply } => {
                                let _ = reply.send(Health {
                                    status: "ok",
                                    version: "test",
                                    protocol: browser_ipc::PROTOCOL_VERSION,
                                    content_pid: Some(1),
                                    connected: true,
                                    active: 0,
                                    queued: 0,
                                    max_tabs: 4,
                                });
                            }
                            Command::Render { job, reply } => {
                                let _ = reply.send(Ok(RenderResponse {
                                    final_url: job.url.to_string(),
                                    title: "Fake".into(),
                                    html: "<html></html>".into(),
                                    load_complete: true,
                                    ..Default::default()
                                }));
                            }
                            Command::GetCookies { reply, .. } => {
                                let _ = reply.send(Ok(vec![CookieInfo {
                                    name: "sid".into(),
                                    value: "abc".into(),
                                    ..Default::default()
                                }]));
                            }
                            Command::SetCookies { reply, .. } => {
                                let _ = reply.send(Ok(()));
                            }
                            Command::Fetch { job, reply } => {
                                let _ = reply.send(Ok(PageFetchResponse {
                                    page_url: job.page_url.to_string(),
                                    url: job.url.to_string(),
                                    final_url: job.url.to_string(),
                                    status: 200,
                                    body: "{}".into(),
                                    body_length: 2,
                                    ..Default::default()
                                }));
                            }
                            Command::DeepSeek { job, reply } => {
                                let result = match &job {
                                    DeepSeekJob::Chat(_) => json!({
                                        "chat_id": "c1",
                                        "chat_url": "https://chat.deepseek.com/a/chat/s/c1",
                                        "answer": "fake answer",
                                        "continued": 0,
                                        "messages": []
                                    }),
                                    DeepSeekJob::ListChats { .. } => json!({
                                        "chats": [{
                                            "id": "c1",
                                            "title": "Fake chat",
                                            "pinned": false,
                                            "updated_at": 1,
                                            "url": "https://chat.deepseek.com/a/chat/s/c1"
                                        }]
                                    }),
                                    DeepSeekJob::CreateChat { .. } => json!({
                                        "id": "c2",
                                        "title": "New",
                                        "url": "https://chat.deepseek.com/a/chat/s/c2"
                                    }),
                                    DeepSeekJob::DeleteChats { .. } => json!({ "ok": true }),
                                    DeepSeekJob::RenameChat { .. } => json!({ "ok": true }),
                                    DeepSeekJob::History { chat_id } => json!({
                                        "chat_id": chat_id,
                                        "messages": [{
                                            "id": "m1",
                                            "role": "assistant",
                                            "text": "hi",
                                            "status": "FINISHED"
                                        }]
                                    }),
                                    DeepSeekJob::Status => json!({
                                        "logged_in": true,
                                        "url": "https://chat.deepseek.com/"
                                    }),
                                };
                                let _ = reply.send(Ok(result));
                            }
                            Command::Shutdown { reply } => {
                                let _ = reply.send(());
                                return;
                            }
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(SessionEntry {
                tx,
                thread: Some(thread),
                content_dir: PathBuf::from("/tmp").join(id),
            })
        }
    }

    fn test_router(token: Option<&str>) -> Router {
        let spec = SessionSpec {
            exe: PathBuf::from("/bin/true"),
            data_root: PathBuf::from("/tmp/rb-test"),
            sessions_root: PathBuf::from("/tmp/rb-test/sessions"),
            socket_root: PathBuf::from("/tmp/rb-test/sockets"),
            host_file: None,
            max_tabs: 4,
            max_queue: 8,
        };
        let registry = SessionRegistry::with_spawner(spec, Arc::new(FakeSpawner));
        router(Arc::new(registry), token.map(str::to_string))
    }

    async fn call(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn json_req(method: &str, path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn render_req(body: serde_json::Value, token: Option<&str>) -> Request<Body> {
        let mut b = Request::post("/render").header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn token_is_enforced() {
        let app = test_router(Some("s3cret"));
        let body = json!({ "url": "https://example.com/" });
        let (status, _) = call(app.clone(), render_req(body.clone(), None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(app.clone(), render_req(body.clone(), Some("nope"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, json) = call(app, render_req(body, Some("s3cret"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["title"], "Fake");
    }

    #[tokio::test]
    async fn rejects_private_urls_before_rendering() {
        let app = test_router(None);
        let (status, json) =
            call(app, render_req(json!({ "url": "http://127.0.0.1:8080/admin" }), None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("private"));
    }

    #[tokio::test]
    async fn health_reports_actor_state() {
        let app = test_router(None);
        let (status, json) = call(
            app,
            Request::get("/health").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ok");
        assert_eq!(json["protocol"], browser_ipc::PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn creates_and_lists_sessions() {
        let app = test_router(None);
        let (status, json) = call(
            app.clone(),
            json_req("POST", "/sessions", json!({ "id": "acc-1" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "acc-1");
        assert!(json["content_dir"]
            .as_str()
            .unwrap()
            .ends_with("acc-1/content"));

        let (status, json) =
            call(app, Request::get("/sessions").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        let sessions = json["sessions"].as_array().unwrap();
        assert!(sessions.iter().any(|s| s["id"] == "acc-1"));
    }

    #[tokio::test]
    async fn rejects_bad_session_ids() {
        let app = test_router(None);
        let (status, json) = call(
            app.clone(),
            json_req("POST", "/sessions", json!({ "id": "../etc" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("session id"));

        let (status, _) = call(
            app,
            Request::get("/sessions/..%2Fetc/cookies?url=https%3A%2F%2Fexample.com%2F")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn exports_and_imports_cookies() {
        let app = test_router(None);
        let (status, json) = call(
            app.clone(),
            Request::get("/sessions/acc-1/cookies?url=https%3A%2F%2Fexample.com%2F")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["cookies"][0]["name"], "sid");

        let (status, json) = call(
            app,
            json_req(
                "PUT",
                "/sessions/acc-1/cookies",
                json!({
                    "url": "https://example.com/",
                    "cookies": [{ "name": "sid", "value": "xyz" }]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn page_fetch_runs_in_the_session() {
        let app = test_router(None);
        let (status, json) = call(
            app,
            json_req(
                "POST",
                "/sessions/acc-1/fetch",
                json!({ "url": "https://example.com/api/me" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], 200);
        assert_eq!(json["url"], "https://example.com/api/me");
    }

    #[tokio::test]
    async fn raw_request_rejects_private_targets() {
        let app = test_router(None);
        let (status, json) = call(
            app,
            json_req(
                "POST",
                "/sessions/acc-1/request",
                json!({ "url": "http://localhost:8080/" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("private"));
    }

    #[tokio::test]
    async fn deepseek_status_and_chat_routes() {
        let app = test_router(None);
        let (status, json) = call(
            app.clone(),
            Request::get("/sessions/acc-1/deepseek/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["logged_in"], true);

        let (status, json) = call(
            app.clone(),
            json_req(
                "POST",
                "/sessions/acc-1/deepseek/chat",
                json!({ "prompt": "hello" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["answer"], "fake answer");
        assert_eq!(json["chat_id"], "c1");

        let (status, json) = call(
            app.clone(),
            Request::get("/sessions/acc-1/deepseek/chats")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["chats"][0]["id"], "c1");

        let (status, json) = call(
            app.clone(),
            json_req("POST", "/sessions/acc-1/deepseek/chats", json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "c2");

        let (status, json) = call(
            app.clone(),
            Request::get("/sessions/acc-1/deepseek/chats/c1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["messages"][0]["text"], "hi");

        let (status, _) = call(
            app,
            json_req(
                "DELETE",
                "/sessions/acc-1/deepseek/chats",
                json!({ "chat_ids": ["c1"] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn deepseek_rejects_bad_input() {
        let app = test_router(None);
        let (status, json) = call(
            app.clone(),
            json_req(
                "POST",
                "/sessions/acc-1/deepseek/chat",
                json!({ "prompt": "  " }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("prompt"));

        let (status, _) = call(
            app.clone(),
            json_req(
                "POST",
                "/sessions/acc-1/deepseek/chat",
                json!({ "prompt": "hi", "chat_id": "../etc" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = call(
            app,
            json_req(
                "DELETE",
                "/sessions/acc-1/deepseek/chats",
                json!({ "chat_ids": [] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn cookie_header_filters_by_scope() {
        let url = Url::parse("https://api.example.com/v1/me").unwrap();
        let cookies = vec![
            CookieInfo {
                name: "a".into(),
                value: "1".into(),
                ..Default::default()
            },
            CookieInfo {
                name: "secure".into(),
                value: "2".into(),
                secure: true,
                ..Default::default()
            },
            CookieInfo {
                name: "other".into(),
                value: "3".into(),
                domain: Some(".other.com".into()),
                ..Default::default()
            },
            CookieInfo {
                name: "expired".into(),
                value: "4".into(),
                expires_unix: Some(1),
                ..Default::default()
            },
            CookieInfo {
                name: "scoped".into(),
                value: "5".into(),
                domain: Some(".example.com".into()),
                path: Some("/v1".into()),
                ..Default::default()
            },
        ];
        assert_eq!(cookie_header(&url, &cookies), "a=1; secure=2; scoped=5");
    }

    #[test]
    fn refuses_public_bind_without_token() {
        let cfg = AutomationConfig {
            addr: "0.0.0.0:9333".parse().unwrap(),
            token: None,
            exe: PathBuf::from("rust-browser"),
            data_dir: std::env::temp_dir(),
            socket_dir: std::env::temp_dir(),
            host_file: None,
            max_tabs: 1,
            max_queue: 1,
            exit_on_stdin_eof: false,
        };
        assert!(run(cfg).unwrap_err().contains("--automation-token"));
    }
}
