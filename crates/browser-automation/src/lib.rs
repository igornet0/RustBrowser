//! Headless automation server: a local HTTP JSON API that renders pages in
//! the Servo Content Process and returns DOM, script results and screenshots.
//!
//! ```text
//! HTTP client (BoardDo, curl)
//!     ↓  POST /render, GET /health
//! axum (tokio)  ──mpsc──▶  Actor thread (owns RemoteContentSession)
//!                              ↓  Unix IPC (protocol v4)
//!                          rust-browser --content-process --automation-content
//! ```
//!
//! The content process runs with `--automation-content`: no frame streaming and
//! no requests to localhost / private networks. Requests for such URLs are
//! rejected up front as well.

mod actor;
mod api;

pub use actor::{Actor, Command, ContentDriver};
pub use api::{Health, RenderError, RenderJob, RenderRequest, RenderResponse};

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use browser_ipc::RemoteContentSession;
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{error, info};

/// Flag passed to content processes spawned for automation.
pub const AUTOMATION_CONTENT_FLAG: &str = "--automation-content";

#[derive(Debug, Clone)]
pub struct AutomationConfig {
    pub addr: SocketAddr,
    /// Required as `Authorization: Bearer <token>` when set (mandatory off-loopback).
    pub token: Option<String>,
    /// The `rust-browser` binary used to spawn content processes.
    pub exe: PathBuf,
    /// Ephemeral automation profile (cookies, cache) — never the user's profile.
    pub data_dir: PathBuf,
    /// Short directory for Unix sockets (macOS `sun_path` limit).
    pub socket_dir: PathBuf,
    pub host_file: Option<PathBuf>,
    pub max_tabs: usize,
    pub max_queue: usize,
}

struct AppState {
    tx: mpsc::Sender<Command>,
    token: Option<String>,
}

/// Start the content process, then serve HTTP until Ctrl-C.
pub fn run(config: AutomationConfig) -> Result<(), String> {
    if !config.addr.ip().is_loopback() && config.token.is_none() {
        return Err(format!(
            "refusing to listen on {} without --automation-token",
            config.addr
        ));
    }
    let (tx, rx) = mpsc::channel::<Command>();
    let actor_thread = spawn_actor(&config, rx)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let app = router(tx, config.token.clone());
    let served = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.addr)
            .await
            .map_err(|e| format!("bind {}: {e}", config.addr))?;
        info!(addr = %config.addr, "automation server listening");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
                info!("automation server shutting down");
            })
            .await
            .map_err(|e| format!("serve: {e}"))
    });
    // Router (and its Sender) is gone: the actor loop ends and shuts content down.
    let _ = actor_thread.join();
    served
}

fn spawn_actor(
    config: &AutomationConfig,
    rx: mpsc::Receiver<Command>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let cfg = config.clone();
    let handle = std::thread::Builder::new()
        .name("automation-actor".into())
        .spawn(move || {
            let session = RemoteContentSession::start_with_options(
                cfg.data_dir.join("content"),
                cfg.socket_dir.clone(),
                cfg.exe.clone(),
                cfg.host_file.clone(),
                vec![AUTOMATION_CONTENT_FLAG.to_string()],
            );
            let session = match session {
                Ok(s) => {
                    let _ = ready_tx.send(Ok(()));
                    s
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("content process: {e}")));
                    return;
                }
            };
            let mut actor = Actor::new(session, cfg.max_tabs, cfg.max_queue);
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => actor.handle(cmd, Instant::now()),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            info!("automation actor stopping");
                            return;
                        }
                    }
                }
                actor.tick(Instant::now());
                let idle = if actor.busy() { 5 } else { 25 };
                std::thread::sleep(Duration::from_millis(idle));
            }
        })
        .map_err(|e| format!("spawn actor: {e}"))?;
    ready_rx
        .recv()
        .map_err(|_| "actor thread exited during startup".to_string())??;
    Ok(handle)
}

/// HTTP routes; `tx` feeds the actor thread.
pub fn router(tx: mpsc::Sender<Command>, token: Option<String>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/render", post(render))
        .with_state(Arc::new(AppState { tx, token }))
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

async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(401, "missing or invalid bearer token".into());
    }
    let (reply, rx) = oneshot::channel();
    if state.tx.send(Command::Health { reply }).is_err() {
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
    let budget = job.timeout + Duration::from_secs(5);
    let (reply, rx) = oneshot::channel();
    if state.tx.send(Command::Render { job, reply }).is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Answers health checks like the real actor; render jobs get a canned page.
    fn fake_actor() -> mpsc::Sender<Command> {
        let (tx, rx) = mpsc::channel::<Command>();
        std::thread::spawn(move || {
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
                }
            }
        });
        tx
    }

    async fn call(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
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
        let app = router(fake_actor(), Some("s3cret".into()));
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
        let app = router(fake_actor(), None);
        let (status, json) =
            call(app, render_req(json!({ "url": "http://127.0.0.1:8080/admin" }), None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("private"));
    }

    #[tokio::test]
    async fn health_reports_actor_state() {
        let app = router(fake_actor(), None);
        let (status, json) = call(
            app,
            Request::get("/health").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ok");
        assert_eq!(json["protocol"], browser_ipc::PROTOCOL_VERSION);
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
        };
        assert!(run(cfg).unwrap_err().contains("--automation-token"));
    }
}
