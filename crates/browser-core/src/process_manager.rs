//! Content process lifecycle and assignment (Browser Process side).
//!
//! Content Process state is disposable. Browser Process owns tab/session truth.

use crate::error::{BrowserError, BrowserResult};
use crate::lifecycle::{ContentResourceLimits, LifecycleEvent, LifecycleState, TransitionError};
use crate::tab::TabId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Alias kept for existing call sites.
pub type ContentProcessState = LifecycleState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentProcessId(pub Uuid);

impl ContentProcessId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ContentProcessId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ContentProcessId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct CrashLoopConfig {
    pub max_crashes: u32,
    pub window: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for CrashLoopConfig {
    fn default() -> Self {
        Self {
            max_crashes: 5,
            window: Duration::from_secs(60),
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SiteKey {
    pub scheme: String,
    pub registrable_domain: String,
}

/// Future site-isolation policy. P1 assigns all tabs to one content process.
#[derive(Debug, Clone, Default)]
pub struct ProcessAssignmentPolicy {
    pub isolate_sites: bool,
}

impl ProcessAssignmentPolicy {
    pub fn assign(
        &self,
        _site: Option<&SiteKey>,
        existing: Option<ContentProcessId>,
        default: ContentProcessId,
    ) -> ContentProcessId {
        if self.isolate_sites {
            warn!("site isolation requested but not enabled");
        }
        existing.unwrap_or(default)
    }
}

#[derive(Debug)]
pub struct ContentProcessHandle {
    pub id: ContentProcessId,
    pub state: ContentProcessState,
    pub child: Option<Child>,
    pub socket_path: PathBuf,
    pub pid: Option<u32>,
    pub last_heartbeat: Instant,
    pub crash_times: Vec<Instant>,
    pub backoff: Duration,
    pub crash_loop: bool,
    /// Monotonic content session generation for IPC stale rejection.
    pub generation: u64,
}

impl ContentProcessHandle {
    fn apply(&mut self, event: LifecycleEvent) -> Result<(), TransitionError> {
        self.state = self.state.transition(event)?;
        Ok(())
    }

    fn force_dead(&mut self) {
        if self.apply(LifecycleEvent::Crash).is_err() {
            self.state = ContentProcessState::Dead;
        }
    }

    pub fn is_alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    warn!(?status, pid = ?self.pid, "content process exited");
                    self.force_dead();
                    self.child = None;
                    false
                }
                Err(err) => {
                    error!(?err, "content process wait failed");
                    false
                }
            },
            None => false,
        }
    }
}

pub struct ContentProcessManager {
    pub processes: HashMap<ContentProcessId, ContentProcessHandle>,
    pub tab_to_process: HashMap<TabId, ContentProcessId>,
    pub policy: ProcessAssignmentPolicy,
    pub crash_cfg: CrashLoopConfig,
    pub limits: ContentResourceLimits,
    pub content_dir: PathBuf,
    pub socket_dir: PathBuf,
    pub exe: PathBuf,
    /// Optional Servo hosts-format file passed to content as `--content-host-file`.
    pub host_file: Option<PathBuf>,
    /// Extra flags for every spawned content process (e.g. `--automation-content`).
    pub extra_args: Vec<String>,
    next_request_id: u64,
    next_generation: u64,
    /// When true, scheduled restarts must not spawn.
    pub shutting_down: bool,
}

impl ContentProcessManager {
    pub fn new(content_dir: PathBuf, socket_dir: PathBuf, exe: PathBuf) -> BrowserResult<Self> {
        Self::with_host_file(content_dir, socket_dir, exe, None)
    }

    pub fn with_host_file(
        content_dir: PathBuf,
        socket_dir: PathBuf,
        exe: PathBuf,
        host_file: Option<PathBuf>,
    ) -> BrowserResult<Self> {
        std::fs::create_dir_all(&content_dir).map_err(|e| BrowserError::Other(e.to_string()))?;
        std::fs::create_dir_all(&socket_dir).map_err(|e| BrowserError::Other(e.to_string()))?;
        Ok(Self {
            processes: HashMap::new(),
            tab_to_process: HashMap::new(),
            policy: ProcessAssignmentPolicy::default(),
            crash_cfg: CrashLoopConfig::default(),
            limits: ContentResourceLimits::default(),
            content_dir,
            socket_dir,
            exe,
            host_file,
            extra_args: Vec::new(),
            next_request_id: 1,
            next_generation: 1,
            shutting_down: false,
        })
    }

    pub fn alloc_generation(&mut self) -> u64 {
        let g = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        g
    }

    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }

    pub fn socket_for(&self, id: ContentProcessId) -> PathBuf {
        let short = id.0.as_simple().to_string();
        self.socket_dir.join(format!("{}.s", &short[..8]))
    }

    pub fn can_assign_tab(&self, process: ContentProcessId) -> bool {
        let n = self
            .tab_to_process
            .values()
            .filter(|p| **p == process)
            .count();
        n < self.limits.max_tabs_per_content
    }

    /// Allocate id + socket, then spawn. Caller must already be listening on `socket_for(id)`.
    pub fn spawn_with_id(&mut self, id: ContentProcessId) -> BrowserResult<()> {
        if self.shutting_down {
            return Err(BrowserError::Other(
                "refusing content spawn during browser shutdown".into(),
            ));
        }
        let generation = self.alloc_generation();
        let socket = self.socket_for(id);
        let child_storage = self.content_dir.join(id.to_string());
        std::fs::create_dir_all(&child_storage)
            .map_err(|e| BrowserError::Other(e.to_string()))?;

        info!(%id, generation, socket = %socket.display(), "spawning content process");

        let mut cmd = Command::new(&self.exe);
        cmd.arg("--content-process")
            .arg(&socket)
            .arg("--content-storage")
            .arg(&child_storage);
        if let Some(host_file) = &self.host_file {
            if host_file.is_file() {
                cmd.arg("--content-host-file").arg(host_file);
            }
        }
        cmd.args(&self.extra_args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env(
                "RUST_LOG",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,browser_ipc=debug".into()),
            )
            .env("HOME", std::env::var("HOME").unwrap_or_default());

        #[cfg(target_os = "macos")]
        {
            if let Ok(v) = std::env::var("DYLD_LIBRARY_PATH") {
                cmd.env("DYLD_LIBRARY_PATH", v);
            }
        }

        let child = cmd
            .spawn()
            .map_err(|e| BrowserError::Other(format!("spawn content: {e}")))?;
        let pid = child.id();

        self.processes.insert(
            id,
            ContentProcessHandle {
                id,
                state: ContentProcessState::Starting,
                child: Some(child),
                socket_path: socket,
                pid: Some(pid),
                last_heartbeat: Instant::now(),
                crash_times: Vec::new(),
                backoff: self.crash_cfg.initial_backoff,
                crash_loop: false,
                generation,
            },
        );
        Ok(())
    }

    pub fn spawn(&mut self) -> BrowserResult<ContentProcessId> {
        let id = ContentProcessId::new();
        self.spawn_with_id(id)?;
        Ok(id)
    }

    pub fn ensure_default(&mut self) -> BrowserResult<ContentProcessId> {
        if let Some((id, handle)) = self.processes.iter_mut().find(|(_, h)| {
            matches!(
                h.state,
                ContentProcessState::Ready
                    | ContentProcessState::Running
                    | ContentProcessState::Starting
            )
        }) {
            let id = *id;
            if handle.is_alive() || matches!(handle.state, ContentProcessState::Starting) {
                return Ok(id);
            }
        }
        self.spawn()
    }

    pub fn assign_tab(&mut self, tab: TabId, process: ContentProcessId) -> BrowserResult<()> {
        if !self.can_assign_tab(process)
            && !self
                .tab_to_process
                .get(&tab)
                .is_some_and(|p| *p == process)
        {
            return Err(BrowserError::Other(format!(
                "max tabs per content ({}) exceeded",
                self.limits.max_tabs_per_content
            )));
        }
        self.tab_to_process.insert(tab, process);
        Ok(())
    }

    pub fn process_for_tab(&self, tab: TabId) -> Option<ContentProcessId> {
        self.tab_to_process.get(&tab).copied()
    }

    pub fn mark_ready(&mut self, id: ContentProcessId) {
        if let Some(h) = self.processes.get_mut(&id) {
            let _ = h.apply(LifecycleEvent::HelloOk);
            let _ = h.apply(LifecycleEvent::MarkRunning);
            h.last_heartbeat = Instant::now();
            h.backoff = self.crash_cfg.initial_backoff;
        }
    }

    pub fn note_heartbeat(&mut self, id: ContentProcessId) {
        if let Some(h) = self.processes.get_mut(&id) {
            h.last_heartbeat = Instant::now();
        }
    }

    pub fn heartbeat_timed_out(&self, id: ContentProcessId, timeout: Duration) -> bool {
        self.processes
            .get(&id)
            .map(|h| h.last_heartbeat.elapsed() > timeout)
            .unwrap_or(true)
    }

    pub fn generation_of(&self, id: ContentProcessId) -> Option<u64> {
        self.processes.get(&id).map(|h| h.generation)
    }

    /// Record a crash and decide whether to restart or enter crash-loop lockout.
    pub fn on_crash(&mut self, id: ContentProcessId) -> BrowserResult<RestartDecision> {
        if self.shutting_down {
            if let Some(handle) = self.processes.get_mut(&id) {
                let _ = handle.apply(LifecycleEvent::Shutdown);
                if let Some(mut child) = handle.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                handle.pid = None;
            }
            return Ok(RestartDecision::Gone);
        }
        let cfg = self.crash_cfg.clone();
        let Some(handle) = self.processes.get_mut(&id) else {
            return Ok(RestartDecision::Gone);
        };
        handle.force_dead();
        if let Some(mut child) = handle.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Invalidate stale sockets/pids immediately.
        handle.pid = None;
        let _ = std::fs::remove_file(&handle.socket_path);

        let now = Instant::now();
        handle.crash_times.push(now);
        handle
            .crash_times
            .retain(|t| now.duration_since(*t) <= cfg.window);
        if handle.crash_times.len() as u32 >= cfg.max_crashes {
            handle.crash_loop = true;
            let _ = handle.apply(LifecycleEvent::CrashLoop);
            error!(%id, "content crash loop detected");
            return Ok(RestartDecision::CrashLoop);
        }
        let backoff = handle.backoff;
        handle.backoff = (handle.backoff * 2).min(cfg.max_backoff);
        let _ = handle.apply(LifecycleEvent::RestartScheduled);
        Ok(RestartDecision::RestartAfter(backoff))
    }

    pub fn replace_process(&mut self, old: ContentProcessId) -> BrowserResult<ContentProcessId> {
        if self.shutting_down {
            return Err(BrowserError::Other(
                "refusing replace_process during shutdown".into(),
            ));
        }
        let tabs: Vec<TabId> = self
            .tab_to_process
            .iter()
            .filter(|(_, p)| **p == old)
            .map(|(t, _)| *t)
            .collect();
        self.processes.remove(&old);
        let new_id = self.spawn()?;
        for tab in tabs {
            self.tab_to_process.insert(tab, new_id);
        }
        Ok(new_id)
    }

    pub fn terminate(&mut self, id: ContentProcessId) -> BrowserResult<()> {
        if let Some(handle) = self.processes.get_mut(&id) {
            if handle.apply(LifecycleEvent::Shutdown).is_err() {
                handle.state = ContentProcessState::Shutdown;
            }
            if let Some(mut child) = handle.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            handle.pid = None;
            let _ = std::fs::remove_file(&handle.socket_path);
        }
        Ok(())
    }

    pub fn shutdown_all(&mut self) {
        self.shutting_down = true;
        let ids: Vec<_> = self.processes.keys().copied().collect();
        for id in ids {
            let _ = self.terminate(id);
        }
    }

    pub fn content_dir(&self) -> &Path {
        &self.content_dir
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartDecision {
    RestartAfter(Duration),
    CrashLoop,
    Gone,
}

/// Whether OOP content isolation is enabled (default: true).
///
/// `RUST_BROWSER_CONTENT=inprocess` is **legacy/debug only** and disables process isolation.
pub fn content_isolation_enabled() -> bool {
    match std::env::var("RUST_BROWSER_CONTENT") {
        Ok(v)
            if v.eq_ignore_ascii_case("inprocess")
                || v == "0"
                || v.eq_ignore_ascii_case("local") =>
        {
            tracing::warn!(
                "WARNING: RUST_BROWSER_CONTENT=inprocess is legacy/debug mode. It disables P1 process isolation."
            );
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_loop_trips_after_threshold() {
        let dir = tempfile_dir();
        let mut mgr = ContentProcessManager::new(
            dir.join("content"),
            dir.join("sock"),
            PathBuf::from("/bin/false"),
        )
        .unwrap();
        mgr.crash_cfg.max_crashes = 3;
        mgr.crash_cfg.window = Duration::from_secs(60);
        let id = ContentProcessId::new();
        mgr.processes.insert(
            id,
            ContentProcessHandle {
                id,
                state: ContentProcessState::Running,
                child: None,
                socket_path: dir.join("x.sock"),
                pid: Some(1),
                last_heartbeat: Instant::now(),
                crash_times: Vec::new(),
                backoff: Duration::from_secs(1),
                crash_loop: false,
                generation: 1,
            },
        );
        assert!(matches!(
            mgr.on_crash(id).unwrap(),
            RestartDecision::RestartAfter(_)
        ));
        assert!(matches!(
            mgr.on_crash(id).unwrap(),
            RestartDecision::RestartAfter(_)
        ));
        assert_eq!(mgr.on_crash(id).unwrap(), RestartDecision::CrashLoop);
    }

    #[test]
    fn shutdown_blocks_respawn() {
        let dir = tempfile_dir();
        let mut mgr = ContentProcessManager::new(
            dir.join("content"),
            dir.join("sock"),
            PathBuf::from("/bin/false"),
        )
        .unwrap();
        mgr.shutting_down = true;
        assert!(mgr.spawn().is_err());
    }

    #[test]
    fn generation_increments() {
        let dir = tempfile_dir();
        let mut mgr = ContentProcessManager::new(
            dir.join("content"),
            dir.join("sock"),
            PathBuf::from("/bin/false"),
        )
        .unwrap();
        let a = mgr.alloc_generation();
        let b = mgr.alloc_generation();
        assert_ne!(a, b);
    }

    fn tempfile_dir() -> PathBuf {
        let p = PathBuf::from("/tmp").join(format!("rb-p12-{}", &Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
