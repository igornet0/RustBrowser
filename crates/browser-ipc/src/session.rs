//! Browser-side session to a remote Content Process (Unix IPC).
//!
//! Shared by the desktop UI and the headless automation server.

use browser_core::{
    BrowserError, BrowserResult, ContentProcessId, ContentProcessManager, ContentProcessState,
    RestartDecision, TabId,
};
use crate::{
    content_msg_tab, listen_unix, validate_content_to_browser, BrowserToContent, ContentToBrowser,
    Envelope, FrameBuffer, InboundOrder, InputEventMsg, IpcError, IpcReader, IpcWriter,
    PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use url::Url;

pub struct RemoteContentSession {
    pub manager: ContentProcessManager,
    pub process_id: ContentProcessId,
    writer: Option<IpcWriter>,
    reader: Option<IpcReader>,
    pub last_frames: HashMap<TabId, FrameBuffer>,
    pub pending_events: Vec<ContentToBrowser>,
    /// Replies to request/response commands (script, screenshot), keyed by `request_id`.
    pending_replies: HashMap<u64, ContentToBrowser>,
    pub crash_loop_locked: bool,
    restart_after: Option<Instant>,
    next_request_id: u64,
    next_sequence: u64,
    heartbeat_every: Duration,
    last_heartbeat_sent: Instant,
    hang_timeout: Duration,
    inbound: InboundOrder,
    generation: u64,
}

impl RemoteContentSession {
    pub fn start(content_dir: PathBuf, socket_dir: PathBuf, exe: PathBuf) -> BrowserResult<Self> {
        Self::start_with_host_file(content_dir, socket_dir, exe, None)
    }

    pub fn start_with_host_file(
        content_dir: PathBuf,
        socket_dir: PathBuf,
        exe: PathBuf,
        host_file: Option<PathBuf>,
    ) -> BrowserResult<Self> {
        Self::start_with_options(content_dir, socket_dir, exe, host_file, Vec::new())
    }

    /// Like [`Self::start_with_host_file`], passing `extra_args` to every content spawn.
    pub fn start_with_options(
        content_dir: PathBuf,
        socket_dir: PathBuf,
        exe: PathBuf,
        host_file: Option<PathBuf>,
        extra_args: Vec<String>,
    ) -> BrowserResult<Self> {
        let mut manager =
            ContentProcessManager::with_host_file(content_dir, socket_dir, exe, host_file)?;
        manager.extra_args = extra_args;
        let process_id = ContentProcessId::new();
        let path = manager.socket_for(process_id);
        let listener = listen_unix(&path).map_err(|e| BrowserError::Other(e.to_string()))?;
        manager.spawn_with_id(process_id)?;
        let generation = manager.generation_of(process_id).unwrap_or(1);

        let mut session = Self {
            manager,
            process_id,
            writer: None,
            reader: None,
            last_frames: HashMap::new(),
            pending_events: Vec::new(),
            pending_replies: HashMap::new(),
            crash_loop_locked: false,
            restart_after: None,
            next_request_id: 1,
            next_sequence: 1,
            heartbeat_every: Duration::from_secs(2),
            last_heartbeat_sent: Instant::now(),
            hang_timeout: Duration::from_secs(15),
            inbound: InboundOrder::new(generation),
            generation,
        };
        session.accept_hello(listener)?;
        Ok(session)
    }

    fn accept_hello(&mut self, listener: std::os::unix::net::UnixListener) -> BrowserResult<()> {
        listener
            .set_nonblocking(false)
            .map_err(|e| BrowserError::Other(e.to_string()))?;
        let (stream, _) = listener
            .accept()
            .map_err(|e| BrowserError::Other(format!("accept content: {e}")))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(5)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .ok();

        let mut writer = IpcWriter::new(
            stream
                .try_clone()
                .map_err(|e| BrowserError::Other(e.to_string()))?,
        );
        writer.generation = self.generation;
        let mut reader = IpcReader::new(stream);
        // During hello, Content may not yet echo generation; accept gen 0 or current.
        reader.expected_generation = None;

        let rid = self.alloc_rid();
        writer
            .send(&Envelope::with_generation(
                rid,
                self.generation,
                BrowserToContent::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
            ))
            .map_err(|e| BrowserError::Other(e.to_string()))?;

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut got_ack = false;
        let mut got_ready = false;
        while Instant::now() < deadline && !(got_ack && got_ready) {
            match reader.try_recv::<ContentToBrowser>() {
                Ok(Some(env)) => {
                    if let Err(err) = validate_content_to_browser(&env.payload) {
                        return Err(BrowserError::Other(err.to_string()));
                    }
                    match env.payload {
                        ContentToBrowser::HelloAck { pid, .. } => {
                            info!(pid, generation = self.generation, "content hello ack");
                            if let Some(h) = self.manager.processes.get_mut(&self.process_id) {
                                h.pid = Some(pid);
                            }
                            got_ack = true;
                        }
                        ContentToBrowser::ProtocolMismatch { expected, got } => {
                            return Err(BrowserError::Other(format!(
                                "protocol mismatch expected={expected} got={got}"
                            )));
                        }
                        ContentToBrowser::Ready => {
                            got_ready = true;
                            self.manager.mark_ready(self.process_id);
                        }
                        other => self.pending_events.push(other),
                    }
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(BrowserError::Other(e.to_string())),
            }
        }
        if !(got_ack && got_ready) {
            return Err(BrowserError::Other(
                "content process did not become ready".into(),
            ));
        }

        reader.expected_generation = Some(self.generation);
        self.writer = Some(writer);
        self.reader = Some(reader);
        self.inbound.reset(self.generation);
        info!(%self.process_id, generation = self.generation, "remote content session ready");
        Ok(())
    }

    fn alloc_rid(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }

    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        s
    }

    fn send(&mut self, payload: BrowserToContent) -> BrowserResult<()> {
        self.send_with_rid(payload).map(|_| ())
    }

    fn send_with_rid(&mut self, payload: BrowserToContent) -> BrowserResult<u64> {
        let rid = self.alloc_rid();
        let seq = self.alloc_seq();
        let Some(writer) = self.writer.as_mut() else {
            return Err(BrowserError::Other("content ipc not connected".into()));
        };
        writer.generation = self.generation;
        writer
            .send(&Envelope::full(rid, self.generation, seq, payload))
            .map_err(|e| BrowserError::Other(e.to_string()))?;
        Ok(rid)
    }

    pub fn create_tab(&mut self, tab_id: TabId, url: Option<Url>) -> BrowserResult<()> {
        self.manager.assign_tab(tab_id, self.process_id)?;
        self.send(BrowserToContent::CreateTab { tab_id, url })
    }

    pub fn navigate(&mut self, tab_id: TabId, url: Url) -> BrowserResult<()> {
        self.send(BrowserToContent::Navigate { tab_id, url })
    }

    pub fn reload(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::Reload { tab_id })
    }

    pub fn go_back(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::GoBack { tab_id })
    }

    pub fn go_forward(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::GoForward { tab_id })
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.manager.tab_to_process.remove(&tab_id);
        self.send(BrowserToContent::CloseTab { tab_id })
    }

    pub fn focus_tab(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::FocusTab { tab_id })
    }

    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) -> BrowserResult<()> {
        self.send(BrowserToContent::Resize {
            width,
            height,
            scale_factor,
        })
    }

    pub fn set_viewport(
        &mut self,
        tab_id: TabId,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> BrowserResult<()> {
        self.send(BrowserToContent::SetViewport {
            tab_id,
            width,
            height,
            scale_factor,
        })
    }

    pub fn suspend_tab(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::SuspendTab { tab_id })
    }

    pub fn resume_tab(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::ResumeTab { tab_id })
    }

    pub fn input(&mut self, tab_id: TabId, event: InputEventMsg) -> BrowserResult<()> {
        self.send(BrowserToContent::Input { tab_id, event })
    }

    pub fn request_frame(&mut self, tab_id: TabId) -> BrowserResult<()> {
        self.send(BrowserToContent::RequestFrame { tab_id })
    }

    /// Start a script evaluation; poll, then fetch the answer with [`Self::take_reply`].
    pub fn evaluate_script(&mut self, tab_id: TabId, script: String) -> BrowserResult<u64> {
        self.send_with_rid(BrowserToContent::EvaluateScript { tab_id, script })
    }

    /// Request the tab's network log; the answer arrives via [`Self::take_reply`].
    pub fn network_log(&mut self, tab_id: TabId) -> BrowserResult<u64> {
        self.send_with_rid(BrowserToContent::GetNetworkLog { tab_id })
    }

    /// Request a PNG screenshot; the answer arrives via [`Self::take_reply`].
    pub fn capture_screenshot(&mut self, tab_id: TabId) -> BrowserResult<u64> {
        self.send_with_rid(BrowserToContent::CaptureScreenshot { tab_id })
    }

    /// Reply to a request/response command, once [`Self::poll`] has received it.
    pub fn take_reply(&mut self, request_id: u64) -> Option<ContentToBrowser> {
        self.pending_replies.remove(&request_id)
    }

    /// Drop a reply that will no longer be awaited (timed out / cancelled).
    pub fn forget_reply(&mut self, request_id: u64) {
        self.pending_replies.remove(&request_id);
    }

    pub fn poll(&mut self) -> BrowserResult<Vec<ContentToBrowser>> {
        self.pump_reader()?;
        self.maybe_heartbeat()?;
        self.check_liveness()?;
        Ok(std::mem::take(&mut self.pending_events))
    }

    fn pump_reader(&mut self) -> BrowserResult<()> {
        let Some(reader) = self.reader.as_mut() else {
            return Ok(());
        };
        loop {
            match reader.try_recv::<ContentToBrowser>() {
                Ok(Some(env)) => {
                    if let Err(err) = validate_content_to_browser(&env.payload) {
                        warn!(%err, "dropping invalid content message");
                        continue;
                    }
                    match self.inbound.accept(
                        env.generation,
                        env.sequence,
                        content_msg_tab(&env.payload),
                    ) {
                        Ok(false) => {
                            warn!(
                                generation = env.generation,
                                sequence = env.sequence,
                                "dropping stale/duplicate content event"
                            );
                            continue;
                        }
                        Err(err) => {
                            warn!(%err, "dropping content event (generation/sequence)");
                            continue;
                        }
                        Ok(true) => {}
                    }
                    match &env.payload {
                        ContentToBrowser::HeartbeatAck => {
                            self.manager.note_heartbeat(self.process_id);
                        }
                        ContentToBrowser::Frame { tab_id, frame } => {
                            self.last_frames.insert(*tab_id, frame.clone());
                        }
                        ContentToBrowser::ScriptResult { .. }
                        | ContentToBrowser::Screenshot { .. }
                        | ContentToBrowser::NetworkLog { .. } => {
                            self.pending_replies.insert(env.request_id, env.payload);
                            continue;
                        }
                        _ => {}
                    }
                    self.pending_events.push(env.payload);
                }
                Ok(None) => break,
                Err(IpcError::Disconnected) => {
                    warn!("content ipc disconnected");
                    self.handle_death()?;
                    break;
                }
                Err(IpcError::Validation(msg)) => {
                    warn!(%msg, "ipc validation failed — fail-closed drop");
                    continue;
                }
                Err(IpcError::VersionMismatch { expected, got }) => {
                    return Err(BrowserError::Other(format!(
                        "protocol mismatch expected={expected} got={got}"
                    )));
                }
                Err(IpcError::TooLarge(n)) => {
                    warn!(n, "oversized content message dropped");
                    continue;
                }
                Err(e) => return Err(BrowserError::Other(e.to_string())),
            }
        }
        Ok(())
    }

    fn maybe_heartbeat(&mut self) -> BrowserResult<()> {
        if self.last_heartbeat_sent.elapsed() >= self.heartbeat_every {
            self.send(BrowserToContent::Heartbeat)?;
            self.last_heartbeat_sent = Instant::now();
        }
        Ok(())
    }

    fn check_liveness(&mut self) -> BrowserResult<()> {
        if self.manager.shutting_down {
            self.restart_after = None;
            return Ok(());
        }
        if let Some(at) = self.restart_after {
            if Instant::now() >= at && !self.crash_loop_locked {
                self.restart_after = None;
                self.respawn_and_restore()?;
            }
            return Ok(());
        }

        let alive = self
            .manager
            .processes
            .get_mut(&self.process_id)
            .map(|h| h.is_alive())
            .unwrap_or(false);

        if !alive {
            self.handle_death()?;
            return Ok(());
        }

        if self
            .manager
            .heartbeat_timed_out(self.process_id, self.hang_timeout)
        {
            warn!("content hang — terminating process");
            let _ = self.manager.terminate(self.process_id);
            self.handle_death()?;
        }
        Ok(())
    }

    fn handle_death(&mut self) -> BrowserResult<()> {
        self.writer = None;
        self.reader = None;
        match self.manager.on_crash(self.process_id)? {
            RestartDecision::CrashLoop => {
                self.crash_loop_locked = true;
                self.pending_events.push(ContentToBrowser::Error {
                    tab_id: None,
                    message: "Web content process repeatedly crashed. [Retry]".into(),
                });
            }
            RestartDecision::RestartAfter(delay) => {
                if self.manager.shutting_down {
                    return Ok(());
                }
                self.restart_after = Some(Instant::now() + delay);
                self.pending_events.push(ContentToBrowser::Error {
                    tab_id: None,
                    message: format!("Content process died — restarting in {delay:?}"),
                });
            }
            RestartDecision::Gone => {}
        }
        Ok(())
    }

    pub fn retry_after_crash_loop(&mut self) -> BrowserResult<()> {
        if self.manager.shutting_down {
            return Err(BrowserError::Other("browser shutting down".into()));
        }
        self.crash_loop_locked = false;
        if let Some(h) = self.manager.processes.get_mut(&self.process_id) {
            h.crash_times.clear();
            h.crash_loop = false;
            h.backoff = self.manager.crash_cfg.initial_backoff;
            h.state = ContentProcessState::Restarting;
        }
        self.respawn_and_restore()
    }

    fn respawn_and_restore(&mut self) -> BrowserResult<()> {
        if self.manager.shutting_down {
            return Err(BrowserError::Other(
                "refusing content respawn during browser shutdown".into(),
            ));
        }
        let tabs: Vec<TabId> = self.manager.tab_to_process.keys().copied().collect();
        let old_pid = self
            .manager
            .processes
            .get(&self.process_id)
            .and_then(|h| h.pid);
        let old_sock = self.manager.socket_for(self.process_id);
        let new_id = ContentProcessId::new();
        for tab in &tabs {
            self.manager.tab_to_process.insert(*tab, new_id);
        }
        self.manager.processes.remove(&self.process_id);
        let _ = std::fs::remove_file(&old_sock);
        if let Some(pid) = old_pid {
            warn!(old_pid = pid, "discarding stale content pid after restart");
        }
        let path = self.manager.socket_for(new_id);
        let listener = listen_unix(&path).map_err(|e| BrowserError::Other(e.to_string()))?;
        self.manager.spawn_with_id(new_id)?;
        self.process_id = new_id;
        self.generation = self.manager.generation_of(new_id).unwrap_or(1);
        self.inbound.reset(self.generation);
        self.next_sequence = 1;
        self.accept_hello(listener)?;
        self.pending_events.push(ContentToBrowser::Error {
            tab_id: None,
            message: format!(
                "content restarted (generation={}) — restore {} tabs",
                self.generation,
                tabs.len()
            ),
        });
        Ok(())
    }

    pub fn content_pid(&self) -> Option<u32> {
        self.manager
            .processes
            .get(&self.process_id)
            .and_then(|h| h.pid)
    }

    pub fn content_generation(&self) -> u64 {
        self.generation
    }

    pub fn writer_connected(&self) -> bool {
        self.writer.is_some() && self.reader.is_some()
    }

    pub fn shutdown(&mut self) {
        self.manager.shutting_down = true;
        self.restart_after = None;
        let _ = self.send(BrowserToContent::Shutdown);
        self.manager.shutdown_all();
        self.writer = None;
        self.reader = None;
    }

    pub fn needs_tab_restore(&self) -> bool {
        matches!(
            self.manager.processes.get(&self.process_id).map(|h| &h.state),
            Some(ContentProcessState::Ready | ContentProcessState::Running)
        ) && self.restart_after.is_none()
            && self.writer.is_some()
    }
}

impl Drop for RemoteContentSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
