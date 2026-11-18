//! Per-session content processes and profiles.
//!
//! Each session owns exactly one Content Process and one stable storage directory:
//!
//! ```text
//! <automation-data>/content                      (default session)
//! <automation-data>/sessions/<id>/content        (named sessions)
//! ```
//!
//! The directory is stable across restarts so Servo's `cookie_jar.json` survives,
//! and the desktop UI can open the same directory with `--content-dir` to let a
//! human sign in (Google / reCAPTCHA) once.

use crate::actor::{Actor, Command};
use crate::api::{
    validate_session_id, LoginOpenResponse, SessionSummary, DEFAULT_SESSION_ID,
};
use crate::AUTOMATION_CONTENT_FLAG;
use browser_ipc::RemoteContentSession;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{info, warn};
use url::Url;

#[derive(Debug, Clone)]
pub struct SessionSpec {
    /// Binary used to spawn content processes and the login window.
    pub exe: PathBuf,
    /// `<automation-data>` root.
    pub data_root: PathBuf,
    /// `<automation-data>/sessions` for named sessions.
    pub sessions_root: PathBuf,
    /// Root for Unix IPC sockets (short path on macOS).
    pub socket_root: PathBuf,
    pub host_file: Option<PathBuf>,
    pub max_tabs: usize,
    pub max_queue: usize,
}

pub struct SessionEntry {
    pub tx: mpsc::Sender<Command>,
    pub thread: Option<JoinHandle<()>>,
    pub content_dir: PathBuf,
}

/// Injectable so the HTTP layer can be tested without Servo.
pub trait SessionSpawner: Send + Sync + 'static {
    fn spawn(&self, id: &str) -> Result<SessionEntry, String>;
}

pub struct RealSpawner {
    spec: SessionSpec,
}

impl RealSpawner {
    pub fn new(spec: SessionSpec) -> Self {
        Self { spec }
    }
}

impl SessionSpawner for RealSpawner {
    fn spawn(&self, id: &str) -> Result<SessionEntry, String> {
        validate_session_id(id).map_err(|e| e.message())?;
        let content_dir = session_content_dir(&self.spec, id);
        let socket_dir = session_socket_dir(&self.spec, id);
        create_private_dir(&content_dir)?;
        create_private_dir(&socket_dir)?;

        info!(
            session = id,
            content_dir = %content_dir.display(),
            "starting session content process"
        );
        let session = RemoteContentSession::start_with_options(
            content_dir.clone(),
            socket_dir,
            self.spec.exe.clone(),
            self.spec.host_file.clone(),
            vec![AUTOMATION_CONTENT_FLAG.to_string()],
        )
        .map_err(|e| format!("content process: {e}"))?;

        let (tx, rx) = mpsc::channel::<Command>();
        let mut actor = Actor::new(session, self.spec.max_tabs, self.spec.max_queue);
        let thread = std::thread::Builder::new()
            .name(format!("automation-{id}"))
            .spawn(move || actor_loop(&mut actor, rx))
            .map_err(|e| format!("spawn actor: {e}"))?;

        Ok(SessionEntry {
            tx,
            thread: Some(thread),
            content_dir,
        })
    }
}

fn actor_loop<D: crate::actor::ContentDriver>(actor: &mut Actor<D>, rx: mpsc::Receiver<Command>) {
    loop {
        loop {
            match rx.try_recv() {
                Ok(cmd) => actor.handle(cmd, Instant::now()),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    info!("automation actor stopping (channel closed)");
                    return;
                }
            }
        }
        actor.tick(Instant::now());
        if actor.stopping() {
            info!("automation actor stopping (shutdown requested)");
            return;
        }
        let idle = if actor.busy() { 5 } else { 25 };
        std::thread::sleep(Duration::from_millis(idle));
    }
}

pub struct SessionRegistry {
    spec: SessionSpec,
    spawner: Arc<dyn SessionSpawner>,
    inner: Mutex<HashMap<String, SessionEntry>>,
    /// Serializes content-process startup without blocking reads of live sessions.
    spawn_lock: Mutex<()>,
    shutting_down: AtomicBool,
}

impl SessionRegistry {
    pub fn new(spec: SessionSpec) -> Self {
        Self::with_spawner(spec.clone(), Arc::new(RealSpawner::new(spec)))
    }

    pub fn with_spawner(spec: SessionSpec, spawner: Arc<dyn SessionSpawner>) -> Self {
        Self {
            spec,
            spawner,
            inner: Mutex::new(HashMap::new()),
            spawn_lock: Mutex::new(()),
            shutting_down: AtomicBool::new(false),
        }
    }

    pub fn get(&self, id: &str) -> Option<mpsc::Sender<Command>> {
        self.inner
            .lock()
            .expect("session registry poisoned")
            .get(id)
            .map(|entry| entry.tx.clone())
    }

    /// Spawn a session if needed. Blocking; use [`Self::ensure`] from async handlers.
    pub fn ensure_blocking(&self, id: &str) -> Result<mpsc::Sender<Command>, String> {
        validate_session_id(id).map_err(|e| e.message())?;
        if let Some(tx) = self.get(id) {
            return Ok(tx);
        }
        let _guard = self.spawn_lock.lock().expect("session spawn lock poisoned");
        if let Some(tx) = self.get(id) {
            return Ok(tx);
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("automation server is shutting down".into());
        }
        let entry = self.spawner.spawn(id)?;
        let tx = entry.tx.clone();
        self.inner
            .lock()
            .expect("session registry poisoned")
            .insert(id.to_string(), entry);
        Ok(tx)
    }

    /// Async wrapper: content-process startup can take hundreds of milliseconds.
    pub async fn ensure(self: &Arc<Self>, id: &str) -> Result<mpsc::Sender<Command>, String> {
        if let Some(tx) = self.get(id) {
            return Ok(tx);
        }
        let registry = Arc::clone(self);
        let id = id.to_string();
        tokio::task::spawn_blocking(move || registry.ensure_blocking(&id))
            .await
            .map_err(|e| format!("session spawn task: {e}"))?
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let mut out: Vec<_> = self
            .inner
            .lock()
            .expect("session registry poisoned")
            .iter()
            .map(|(id, entry)| SessionSummary {
                id: id.clone(),
                content_dir: entry.content_dir.display().to_string(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn content_dir(&self, id: &str) -> PathBuf {
        session_content_dir(&self.spec, id)
    }

    /// Gracefully stop one session. Blocking: waits for the content process to flush
    /// its cookie jar before joining the actor thread.
    pub fn stop_blocking(&self, id: &str) -> Result<(), String> {
        let entry = self
            .inner
            .lock()
            .expect("session registry poisoned")
            .remove(id);
        let Some(mut entry) = entry else {
            return Ok(());
        };
        // Ask the actor to fail in-flight work, then join it. Joining (rather than
        // `blocking_recv`) keeps this safe when called from an async runtime thread;
        // dropping the session flushes Servo's cookie jar.
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = entry.tx.send(Command::Shutdown { reply: reply_tx });
        if let Some(thread) = entry.thread.take() {
            let _ = thread.join();
        }
        info!(session = id, "session stopped");
        Ok(())
    }

    pub fn shutdown_all_blocking(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let ids: Vec<String> = self
            .inner
            .lock()
            .expect("session registry poisoned")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.stop_blocking(&id);
        }
    }

    /// Open a visible browser window on the session profile so the user can sign in
    /// manually (Google, reCAPTCHA). Blocks until the window closes or `timeout`.
    ///
    /// The content process is stopped first (one writer per profile) and restarted
    /// afterwards, which makes the headless session pick up the new cookies.
    pub fn login_blocking(
        &self,
        id: &str,
        url: &Url,
        timeout: Duration,
    ) -> Result<LoginOpenResponse, String> {
        validate_session_id(id).map_err(|e| e.message())?;
        let content_dir = self.content_dir(id);
        create_private_dir(&content_dir)?;

        self.stop_blocking(id)?;

        let started = Instant::now();
        let mut child = ProcessCommand::new(&self.spec.exe)
            .arg("--open")
            .arg(url.as_str())
            .arg("--content-dir")
            .arg(&content_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn login window: {e}"))?;

        let status = wait_with_timeout(&mut child, timeout);
        let timed_out = status.is_none();
        if let Some(status) = status {
            if !status.success() {
                warn!(
                    session = id,
                    ?status,
                    "login window exited with a non-zero status"
                );
            }
        }
        if timed_out {
            warn!(session = id, "login window timed out; killing it");
            let _ = child.kill();
            let _ = child.wait();
            // The UI's Content Process is a separate child: it exits when its IPC
            // disconnects and flushes the profile. Give it a moment before a new
            // headless writer opens the same directory.
            std::thread::sleep(Duration::from_millis(1_000));
        }

        // Re-open the headless session on the updated profile.
        self.ensure_blocking(id)?;

        if timed_out {
            return Err(format!(
                "login window did not close within {}s",
                timeout.as_secs()
            ));
        }
        Ok(LoginOpenResponse {
            session: id.to_string(),
            url: url.to_string(),
            exit_code: status.and_then(|s| s.code()),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

impl Drop for SessionRegistry {
    fn drop(&mut self) {
        self.shutdown_all_blocking();
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(err) => {
                warn!(?err, "login window wait failed");
                return None;
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn session_content_dir(spec: &SessionSpec, id: &str) -> PathBuf {
    if id == DEFAULT_SESSION_ID {
        spec.data_root.join("content")
    } else {
        spec.sessions_root.join(id).join("content")
    }
}

fn session_socket_dir(spec: &SessionSpec, id: &str) -> PathBuf {
    if id == DEFAULT_SESSION_ID {
        spec.socket_root.clone()
    } else {
        spec.socket_root.join(id)
    }
}

/// Session profiles hold session tokens: keep them owner-only.
fn create_private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_session_keeps_legacy_content_dir() {
        let spec = SessionSpec {
            exe: PathBuf::from("/bin/true"),
            data_root: PathBuf::from("/data"),
            sessions_root: PathBuf::from("/data/sessions"),
            socket_root: PathBuf::from("/tmp/rb"),
            host_file: None,
            max_tabs: 1,
            max_queue: 1,
        };
        assert_eq!(
            session_content_dir(&spec, DEFAULT_SESSION_ID),
            PathBuf::from("/data/content")
        );
        assert_eq!(
            session_content_dir(&spec, "acc-1"),
            PathBuf::from("/data/sessions/acc-1/content")
        );
        assert_eq!(
            session_socket_dir(&spec, "acc-1"),
            PathBuf::from("/tmp/rb/acc-1")
        );
    }
}
