//! Single-threaded job runner that owns the Content Process session.
//!
//! Each render job is a small state machine advanced by [`Actor::tick`]:
//! load → wait for selector → settle → snapshot → user script → screenshot → done.

use crate::api::{
    network_collect_script, page_fetch_poll_script, page_fetch_start_script, snapshot_script,
    wait_probe_script, Health, PageFetchJob, PageFetchResponse, RenderError, RenderJob,
    RenderResponse,
};
use crate::deepseek::{deepseek_poll_script, DeepSeekJob};
use browser_core::{BrowserResult, TabId};
use browser_ipc::{ContentToBrowser, CookieInfo, RemoteContentSession, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{info, warn};
use url::Url;

/// How often the wait condition re-checks the page.
const SELECTOR_POLL: Duration = Duration::from_millis(250);
/// Share of the job budget allowed for the `load` event; the rest is for snapshot work.
const LOAD_SHARE: f64 = 0.7;
/// Time kept back from waiting so the snapshot still fits into the budget.
const SNAPSHOT_RESERVE: Duration = Duration::from_secs(3);
/// A wait probe sent while the navigation commits can be lost with the old document;
/// after this long without a reply it is sent again.
const PROBE_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
/// How often a running page `fetch()` is polled for completion.
const FETCH_POLL: Duration = Duration::from_millis(100);

/// What the actor needs from the Content Process (a seam for tests).
pub trait ContentDriver {
    fn create_tab(&mut self, tab: TabId, url: Url) -> BrowserResult<()>;
    fn close_tab(&mut self, tab: TabId);
    fn navigate(&mut self, tab: TabId, url: Url) -> BrowserResult<()>;
    fn evaluate(&mut self, tab: TabId, script: String) -> BrowserResult<u64>;
    fn screenshot(&mut self, tab: TabId) -> BrowserResult<u64>;
    fn network_log(&mut self, tab: TabId) -> BrowserResult<u64>;
    fn get_cookies(&mut self, url: Url) -> BrowserResult<u64>;
    fn set_cookies(&mut self, url: Url, cookies: Vec<CookieInfo>) -> BrowserResult<u64>;
    fn take_reply(&mut self, request_id: u64) -> Option<ContentToBrowser>;
    fn forget_reply(&mut self, request_id: u64);
    fn poll(&mut self) -> BrowserResult<Vec<ContentToBrowser>>;
    fn connected(&self) -> bool;
    fn content_pid(&self) -> Option<u32>;
}

impl ContentDriver for RemoteContentSession {
    fn create_tab(&mut self, tab: TabId, url: Url) -> BrowserResult<()> {
        RemoteContentSession::create_tab(self, tab, Some(url))
    }
    fn close_tab(&mut self, tab: TabId) {
        let _ = RemoteContentSession::close_tab(self, tab);
    }
    fn navigate(&mut self, tab: TabId, url: Url) -> BrowserResult<()> {
        RemoteContentSession::navigate(self, tab, url)
    }
    fn evaluate(&mut self, tab: TabId, script: String) -> BrowserResult<u64> {
        self.evaluate_script(tab, script)
    }
    fn screenshot(&mut self, tab: TabId) -> BrowserResult<u64> {
        self.capture_screenshot(tab)
    }
    fn network_log(&mut self, tab: TabId) -> BrowserResult<u64> {
        RemoteContentSession::network_log(self, tab)
    }
    fn get_cookies(&mut self, url: Url) -> BrowserResult<u64> {
        RemoteContentSession::get_cookies(self, url)
    }
    fn set_cookies(&mut self, url: Url, cookies: Vec<CookieInfo>) -> BrowserResult<u64> {
        RemoteContentSession::set_cookies(self, url, cookies)
    }
    fn take_reply(&mut self, request_id: u64) -> Option<ContentToBrowser> {
        RemoteContentSession::take_reply(self, request_id)
    }
    fn forget_reply(&mut self, request_id: u64) {
        RemoteContentSession::forget_reply(self, request_id)
    }
    fn poll(&mut self) -> BrowserResult<Vec<ContentToBrowser>> {
        RemoteContentSession::poll(self)
    }
    fn connected(&self) -> bool {
        self.writer_connected()
    }
    fn content_pid(&self) -> Option<u32> {
        RemoteContentSession::content_pid(self)
    }
}

pub type RenderReply = oneshot::Sender<Result<RenderResponse, RenderError>>;
pub type FetchReply = oneshot::Sender<Result<PageFetchResponse, RenderError>>;
pub type CookiesReply = oneshot::Sender<Result<Vec<CookieInfo>, String>>;
pub type DeepSeekReply = oneshot::Sender<Result<Value, RenderError>>;

pub enum Command {
    Render {
        job: RenderJob,
        reply: RenderReply,
    },
    Health {
        reply: oneshot::Sender<Health>,
    },
    /// Export cookies (including HttpOnly) for a URL.
    GetCookies {
        url: Url,
        reply: CookiesReply,
    },
    /// Import cookies for a URL into this session's jar.
    SetCookies {
        url: Url,
        cookies: Vec<CookieInfo>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Run `fetch()` inside a page of this session (cookies + page JS).
    Fetch {
        job: PageFetchJob,
        reply: FetchReply,
    },
    /// Drive the DeepSeek plugin in a dedicated page tab.
    DeepSeek {
        job: DeepSeekJob,
        reply: DeepSeekReply,
    },
    /// Flush state and stop the actor thread.
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

#[derive(Debug)]
enum Stage {
    Loading,
    WaitCondition { next_poll: Instant, pending: Option<(u64, Instant)> },
    Settle { until: Instant },
    Snapshot { rid: u64 },
    Network {
        hook_rid: u64,
        log_rid: u64,
        responses: Option<Value>,
        requests: Option<Value>,
    },
    Script { rid: u64 },
    Screenshot { rid: u64 },
    Done,
}

struct Running {
    tab: TabId,
    job: RenderJob,
    reply: Option<RenderReply>,
    started: Instant,
    deadline: Instant,
    load_deadline: Instant,
    stage: Stage,
    load_complete: bool,
    snapshot_taken: bool,
    response: RenderResponse,
}

impl Running {
    fn pending_rid(&self) -> Option<u64> {
        match self.stage {
            Stage::WaitCondition { pending, .. } => pending.map(|(rid, _)| rid),
            Stage::Snapshot { rid } | Stage::Script { rid } | Stage::Screenshot { rid } => Some(rid),
            Stage::Network { hook_rid, .. } => Some(hook_rid),
            _ => None,
        }
    }
}

struct PendingCookies {
    rid: u64,
    sent: Instant,
    reply: CookiesReply,
}

#[derive(Debug)]
enum FetchStage {
    Loading,
    Starting {
        pending: Option<(u64, Instant)>,
    },
    Polling {
        next_poll: Instant,
        pending: Option<(u64, Instant)>,
    },
    Done,
}

struct FetchRun {
    job: PageFetchJob,
    reply: Option<FetchReply>,
    tab: TabId,
    stage: FetchStage,
    started: Instant,
    deadline: Instant,
    load_complete: bool,
}

#[derive(Debug)]
enum DeepSeekStage {
    Loading,
    /// Page loaded; wait briefly so delayed client-side redirects settle before
    /// the plugin script is installed.
    Settling {
        until: Instant,
    },
    Setup {
        pending: Option<(u64, Instant)>,
    },
    Polling {
        next_poll: Instant,
        pending: Option<(u64, Instant)>,
    },
    Done,
}

struct DeepSeekRun {
    job: DeepSeekJob,
    reply: Option<DeepSeekReply>,
    tab: TabId,
    stage: DeepSeekStage,
    started: Instant,
    deadline: Instant,
    /// SPA route changes may not fire `load`; start the plugin after this anyway.
    load_deadline: Instant,
    load_complete: bool,
    /// Idempotent operations are retried once if a full page reload wipes them.
    retries: u8,
}

pub struct Actor<D: ContentDriver> {
    driver: D,
    max_tabs: usize,
    max_queue: usize,
    active: Vec<Running>,
    queue: VecDeque<(RenderJob, RenderReply)>,
    stopping: bool,
    pending_cookies: Vec<PendingCookies>,
    /// One persistent tab per session: keeps the logged-in page state for `fetch`.
    fetch_tab: Option<TabId>,
    fetch_tab_url: Option<Url>,
    fetch: Option<FetchRun>,
    fetch_queue: VecDeque<(PageFetchJob, FetchReply)>,
    /// Dedicated page tab for the DeepSeek plugin.
    plugin_tab: Option<TabId>,
    plugin_tab_url: Option<Url>,
    /// When the plugin tab URL last changed; used to let redirects settle.
    plugin_url_changed_at: Instant,
    /// True while the plugin tab is navigating; scripts must not run yet.
    plugin_loading: bool,
    deepseek: Option<DeepSeekRun>,
    deepseek_queue: VecDeque<(DeepSeekJob, DeepSeekReply)>,
}

impl<D: ContentDriver> Actor<D> {
    pub fn new(driver: D, max_tabs: usize, max_queue: usize) -> Self {
        Self {
            driver,
            max_tabs: max_tabs.max(1),
            max_queue,
            active: Vec::new(),
            queue: VecDeque::new(),
            stopping: false,
            pending_cookies: Vec::new(),
            fetch_tab: None,
            fetch_tab_url: None,
            fetch: None,
            fetch_queue: VecDeque::new(),
            plugin_tab: None,
            plugin_tab_url: None,
            plugin_url_changed_at: Instant::now(),
            plugin_loading: false,
            deepseek: None,
            deepseek_queue: VecDeque::new(),
        }
    }

    pub fn busy(&self) -> bool {
        !self.active.is_empty()
            || !self.queue.is_empty()
            || self.fetch.is_some()
            || !self.fetch_queue.is_empty()
            || self.deepseek.is_some()
            || !self.deepseek_queue.is_empty()
    }

    pub fn stopping(&self) -> bool {
        self.stopping
    }

    pub fn handle(&mut self, cmd: Command, now: Instant) {
        match cmd {
            Command::Health { reply } => {
                let connected = self.driver.connected();
                let _ = reply.send(Health {
                    status: if connected { "ok" } else { "degraded" },
                    version: env!("CARGO_PKG_VERSION"),
                    protocol: PROTOCOL_VERSION,
                    content_pid: self.driver.content_pid(),
                    connected,
                    active: self.active.len(),
                    queued: self.queue.len(),
                    max_tabs: self.max_tabs,
                });
            }
            Command::Render { job, reply } => {
                if self.queue.len() >= self.max_queue {
                    let _ = reply.send(Err(RenderError::Busy));
                    return;
                }
                self.queue.push_back((job, reply));
                self.start_queued(now);
            }
            Command::GetCookies { url, reply } => match self.driver.get_cookies(url) {
                Ok(rid) => self.pending_cookies.push(PendingCookies {
                    rid,
                    sent: now,
                    reply,
                }),
                Err(err) => {
                    let _ = reply.send(Err(err.to_string()));
                }
            },
            Command::SetCookies {
                url,
                cookies,
                reply,
            } => {
                let result = self
                    .driver
                    .set_cookies(url, cookies)
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            Command::Fetch { job, reply } => {
                if self.fetch_queue.len() >= self.max_queue {
                    let _ = reply.send(Err(RenderError::Busy));
                    return;
                }
                self.fetch_queue.push_back((job, reply));
                self.start_fetch(now);
            }
            Command::DeepSeek { job, reply } => {
                if self.deepseek_queue.len() >= self.max_queue {
                    let _ = reply.send(Err(RenderError::Busy));
                    return;
                }
                self.deepseek_queue.push_back((job, reply));
                self.start_deepseek(now);
            }
            Command::Shutdown { reply } => {
                self.stopping = true;
                // Fail in-flight work instead of leaving clients hanging.
                for job in &mut self.active {
                    fail(job, RenderError::Failed("session is shutting down".into()));
                }
                if let Some(fetch) = &mut self.fetch {
                    fetch.stage = FetchStage::Done;
                    if let Some(reply) = fetch.reply.take() {
                        let _ = reply.send(Err(RenderError::Failed(
                            "session is shutting down".into(),
                        )));
                    }
                }
                self.fetch_queue.clear();
                if let Some(plugin) = &mut self.deepseek {
                    plugin.stage = DeepSeekStage::Done;
                    if let Some(reply) = plugin.reply.take() {
                        let _ = reply.send(Err(RenderError::Failed(
                            "session is shutting down".into(),
                        )));
                    }
                }
                self.deepseek_queue.clear();
                for pending in self.pending_cookies.drain(..) {
                    let _ = pending
                        .reply
                        .send(Err("session is shutting down".into()));
                }
                let _ = reply.send(());
            }
        }
    }

    fn start_queued(&mut self, now: Instant) {
        while self.active.len() < self.max_tabs {
            let Some((job, reply)) = self.queue.pop_front() else {
                return;
            };
            if reply.is_closed() {
                continue; // HTTP client gave up while queued.
            }
            let tab = TabId::new();
            if let Err(err) = self.driver.create_tab(tab, job.url.clone()) {
                let _ = reply.send(Err(RenderError::Failed(format!("create tab: {err}"))));
                continue;
            }
            info!(%tab, url = %job.url, "automation render started");
            let load_budget = job.timeout.mul_f64(LOAD_SHARE);
            self.active.push(Running {
                tab,
                response: RenderResponse {
                    final_url: job.url.to_string(),
                    ..Default::default()
                },
                deadline: now + job.timeout,
                load_deadline: now + load_budget,
                job,
                reply: Some(reply),
                started: now,
                stage: Stage::Loading,
                load_complete: false,
                snapshot_taken: false,
            });
        }
    }

    /// Pump content events, advance every job, finish the ones that are done.
    pub fn tick(&mut self, now: Instant) {
        if self.stopping {
            return;
        }
        let events = match self.driver.poll() {
            Ok(events) => events,
            Err(err) => {
                warn!(%err, "content poll failed");
                Vec::new()
            }
        };
        for event in events {
            self.route_event(event, now);
        }
        if !self.driver.connected() {
            // Tabs do not survive a content restart: fail what was running.
            for job in &mut self.active {
                fail(job, RenderError::Failed("content process crashed".into()));
            }
            self.fetch_tab = None;
            self.fetch_tab_url = None;
            if let Some(fetch) = &mut self.fetch {
                fail_fetch(fetch, RenderError::Failed("content process crashed".into()));
            }
            self.plugin_tab = None;
            self.plugin_tab_url = None;
            if let Some(plugin) = &mut self.deepseek {
                fail_deepseek(
                    plugin,
                    RenderError::Failed("content process crashed".into()),
                );
            }
        }

        self.pump_cookies(now);

        for i in 0..self.active.len() {
            let job = &mut self.active[i];
            advance(&mut self.driver, job, now);
        }

        let mut finished = Vec::new();
        self.active.retain_mut(|job| {
            if matches!(job.stage, Stage::Done) {
                finished.push(job.tab);
                false
            } else {
                true
            }
        });
        for tab in finished {
            self.driver.close_tab(tab);
        }
        self.advance_fetch(now);
        self.advance_deepseek(now);
        self.start_queued(now);
        self.start_fetch(now);
        self.start_deepseek(now);
    }

    /// Deliver pending cookie replies and time out stale ones.
    fn pump_cookies(&mut self, now: Instant) {
        let mut done = Vec::new();
        for (i, pending) in self.pending_cookies.iter().enumerate() {
            match self.driver.take_reply(pending.rid) {
                Some(ContentToBrowser::Cookies { cookies }) => {
                    done.push((i, Ok(cookies)));
                }
                Some(other) => {
                    done.push((i, Err(format!("unexpected cookie reply: {other:?}"))));
                }
                None if now.duration_since(pending.sent) > Duration::from_secs(5) => {
                    self.driver.forget_reply(pending.rid);
                    done.push((i, Err("cookie request timed out".into())));
                }
                None => {}
            }
        }
        for (i, result) in done.into_iter().rev() {
            let pending = self.pending_cookies.remove(i);
            let _ = pending.reply.send(result);
        }
    }

    fn route_event(&mut self, event: ContentToBrowser, now: Instant) {
        if let Some(fetch_tab) = self.fetch_tab {
            match &event {
                ContentToBrowser::NavigationFinished { tab_id, .. }
                | ContentToBrowser::LoadStatusChanged {
                    tab_id,
                    loading: false,
                } if *tab_id == fetch_tab => {
                    if let Some(fetch) = &mut self.fetch {
                        fetch.load_complete = true;
                    }
                }
                ContentToBrowser::UrlChanged { tab_id, url } if *tab_id == fetch_tab => {
                    self.fetch_tab_url = Some(url.clone());
                }
                ContentToBrowser::TabCrashed { tab_id, reason } if *tab_id == fetch_tab => {
                    if let Some(fetch) = &mut self.fetch {
                        fail_fetch(
                            fetch,
                            RenderError::Failed(format!("tab crashed: {reason}")),
                        );
                    }
                    self.fetch_tab = None;
                    self.fetch_tab_url = None;
                }
                ContentToBrowser::Error {
                    tab_id: Some(tab_id),
                    message,
                } if *tab_id == fetch_tab => {
                    if let Some(fetch) = &mut self.fetch {
                        fail_fetch(fetch, RenderError::Failed(message.clone()));
                    }
                }
                _ => {}
            }
        }
        if let Some(plugin_tab) = self.plugin_tab {
            match &event {
                ContentToBrowser::NavigationStarted { tab_id, .. } if *tab_id == plugin_tab => {
                    self.plugin_loading = true;
                }
                ContentToBrowser::NavigationFinished { tab_id, .. }
                | ContentToBrowser::LoadStatusChanged {
                    tab_id,
                    loading: false,
                } if *tab_id == plugin_tab => {
                    self.plugin_loading = false;
                    if let Some(plugin) = &mut self.deepseek {
                        plugin.load_complete = true;
                    }
                }
                ContentToBrowser::LoadStatusChanged {
                    tab_id,
                    loading: true,
                } if *tab_id == plugin_tab => {
                    self.plugin_loading = true;
                }
                ContentToBrowser::UrlChanged { tab_id, url } if *tab_id == plugin_tab => {
                    self.plugin_tab_url = Some(url.clone());
                    self.plugin_url_changed_at = now;
                }
                ContentToBrowser::TabCrashed { tab_id, reason } if *tab_id == plugin_tab => {
                    if let Some(plugin) = &mut self.deepseek {
                        fail_deepseek(
                            plugin,
                            RenderError::Failed(format!("tab crashed: {reason}")),
                        );
                    }
                    self.plugin_tab = None;
                    self.plugin_tab_url = None;
                    self.plugin_loading = false;
                }
                ContentToBrowser::Error {
                    tab_id: Some(tab_id),
                    message,
                } if *tab_id == plugin_tab => {
                    if let Some(plugin) = &mut self.deepseek {
                        fail_deepseek(plugin, RenderError::Failed(message.clone()));
                    }
                }
                _ => {}
            }
        }
        match event {
            ContentToBrowser::LoadStatusChanged {
                tab_id,
                loading: false,
            }
            | ContentToBrowser::NavigationFinished { tab_id, .. } => {
                if let Some(job) = self.active.iter_mut().find(|j| j.tab == tab_id) {
                    job.load_complete = true;
                }
            }
            ContentToBrowser::UrlChanged { tab_id, url } => {
                if let Some(job) = self.active.iter_mut().find(|j| j.tab == tab_id) {
                    job.response.final_url = url.to_string();
                }
            }
            ContentToBrowser::TabCrashed { tab_id, reason } => {
                if let Some(job) = self.active.iter_mut().find(|j| j.tab == tab_id) {
                    fail(job, RenderError::Failed(format!("tab crashed: {reason}")));
                }
            }
            ContentToBrowser::Error {
                tab_id: Some(tab_id),
                message,
            } => {
                if let Some(job) = self.active.iter_mut().find(|j| j.tab == tab_id) {
                    fail(job, RenderError::Failed(message));
                }
            }
            _ => {}
        }
    }

    /// Start the next queued page fetch on the persistent session tab.
    fn start_fetch(&mut self, now: Instant) {
        if self.fetch.is_some() {
            return;
        }
        let (job, reply) = loop {
            let Some((job, reply)) = self.fetch_queue.pop_front() else {
                return;
            };
            if !reply.is_closed() {
                break (job, reply);
            }
        };

        let reuse = self.fetch_tab.is_some()
            && self.fetch_tab_url.as_ref() == Some(&job.page_url)
            && self.driver.connected();
        let (tab, stage, load_complete) = if reuse {
            (
                self.fetch_tab.expect("checked above"),
                FetchStage::Starting { pending: None },
                true,
            )
        } else if let Some(tab) = self.fetch_tab {
            if let Err(err) = self.driver.navigate(tab, job.page_url.clone()) {
                let _ = reply.send(Err(RenderError::Failed(format!("navigate: {err}"))));
                return;
            }
            (tab, FetchStage::Loading, false)
        } else {
            let tab = TabId::new();
            if let Err(err) = self.driver.create_tab(tab, job.page_url.clone()) {
                let _ = reply.send(Err(RenderError::Failed(format!("create tab: {err}"))));
                return;
            }
            self.fetch_tab = Some(tab);
            (tab, FetchStage::Loading, false)
        };

        self.fetch_tab_url = Some(job.page_url.clone());
        info!(tab = %tab, url = %job.url, "session fetch started");
        let deadline = now + job.timeout;
        self.fetch = Some(FetchRun {
            job,
            reply: Some(reply),
            tab,
            stage,
            started: now,
            deadline,
            load_complete,
        });
    }

    fn advance_fetch(&mut self, now: Instant) {
        let Some(mut run) = self.fetch.take() else {
            return;
        };
        if now >= run.deadline {
            fail_fetch(
                &mut run,
                RenderError::Timeout("page fetch did not finish in time".into()),
            );
            return;
        }
        loop {
            match std::mem::replace(&mut run.stage, FetchStage::Done) {
                FetchStage::Done => return,
                FetchStage::Loading => {
                    if !run.load_complete {
                        run.stage = FetchStage::Loading;
                        self.fetch = Some(run);
                        return;
                    }
                    match self
                        .driver
                        .evaluate(run.tab, page_fetch_start_script(&run.job))
                    {
                        Ok(rid) => {
                            run.stage = FetchStage::Starting {
                                pending: Some((rid, now)),
                            };
                            self.fetch = Some(run);
                            return;
                        }
                        Err(err) => {
                            fail_fetch(
                                &mut run,
                                RenderError::Failed(format!("fetch start: {err}")),
                            );
                            return;
                        }
                    }
                }
                FetchStage::Starting { pending } => {
                    let Some((rid, sent)) = pending else {
                        run.stage = FetchStage::Polling {
                            next_poll: now,
                            pending: None,
                        };
                        continue;
                    };
                    match self.driver.take_reply(rid) {
                        Some(reply) => match script_reply(reply) {
                            Ok(_) => {
                                run.stage = FetchStage::Polling {
                                    next_poll: now,
                                    pending: None,
                                };
                                continue;
                            }
                            Err(err) => {
                                fail_fetch(
                                    &mut run,
                                    RenderError::Failed(format!("fetch start: {err}")),
                                );
                                return;
                            }
                        },
                        None if now.duration_since(sent) > PROBE_REPLY_TIMEOUT => {
                            self.driver.forget_reply(rid);
                            run.stage = FetchStage::Starting { pending: None };
                            self.fetch = Some(run);
                            return;
                        }
                        None => {
                            run.stage = FetchStage::Starting {
                                pending: Some((rid, sent)),
                            };
                            self.fetch = Some(run);
                            return;
                        }
                    }
                }
                FetchStage::Polling { next_poll, pending } => {
                    if let Some((rid, sent)) = pending {
                        match self.driver.take_reply(rid) {
                            Some(reply) => match script_reply(reply) {
                                Ok(Value::Null) => {
                                    run.stage = FetchStage::Polling {
                                        next_poll: now + FETCH_POLL,
                                        pending: None,
                                    };
                                    self.fetch = Some(run);
                                    return;
                                }
                                Ok(value) => {
                                    match page_fetch_response(&run, value, now) {
                                        Ok(response) => {
                                            if let Some(reply) = run.reply.take() {
                                                info!(
                                                    tab = %run.tab,
                                                    status = response.status,
                                                    elapsed_ms = response.elapsed_ms,
                                                    "session fetch done"
                                                );
                                                let _ = reply.send(Ok(response));
                                            }
                                        }
                                        Err(err) => fail_fetch(&mut run, err),
                                    }
                                    return;
                                }
                                Err(err) => {
                                    fail_fetch(
                                        &mut run,
                                        RenderError::Failed(format!("fetch poll: {err}")),
                                    );
                                    return;
                                }
                            },
                            None if now.duration_since(sent) > PROBE_REPLY_TIMEOUT => {
                                self.driver.forget_reply(rid);
                                run.stage = FetchStage::Polling {
                                    next_poll: now,
                                    pending: None,
                                };
                                self.fetch = Some(run);
                                return;
                            }
                            None => {
                                run.stage = FetchStage::Polling {
                                    next_poll,
                                    pending: Some((rid, sent)),
                                };
                                self.fetch = Some(run);
                                return;
                            }
                        }
                    } else if now >= next_poll {
                        match self
                            .driver
                            .evaluate(run.tab, page_fetch_poll_script().to_string())
                        {
                            Ok(rid) => {
                                run.stage = FetchStage::Polling {
                                    next_poll,
                                    pending: Some((rid, now)),
                                };
                                self.fetch = Some(run);
                                return;
                            }
                            Err(err) => {
                                fail_fetch(
                                    &mut run,
                                    RenderError::Failed(format!("fetch poll: {err}")),
                                );
                                return;
                            }
                        }
                    } else {
                        run.stage = FetchStage::Polling {
                            next_poll,
                            pending: None,
                        };
                        self.fetch = Some(run);
                        return;
                    }
                }
            }
        }
    }
    /// Start the next queued DeepSeek operation in the plugin tab.
    fn start_deepseek(&mut self, now: Instant) {
        if self.deepseek.is_some() {
            return;
        }
        let (job, reply) = loop {
            let Some((job, reply)) = self.deepseek_queue.pop_front() else {
                return;
            };
            if !reply.is_closed() {
                break (job, reply);
            }
        };

        let desired = job.page_url();
        let reuse = self.plugin_tab.is_some()
            && self
                .plugin_tab_url
                .as_ref()
                .is_some_and(|url| job.matches_tab(url))
            && self.driver.connected();
        let (tab, stage, load_complete) = if reuse {
            let tab = self.plugin_tab.expect("checked above");
            if self.plugin_loading {
                // A client-side redirect may still be in flight; running the
                // setup script now would be wiped by the new document.
                (tab, DeepSeekStage::Loading, false)
            } else {
                (tab, DeepSeekStage::Setup { pending: None }, true)
            }
        } else if let Some(tab) = self.plugin_tab {
            if let Err(err) = self.driver.navigate(tab, desired.clone()) {
                let _ = reply.send(Err(RenderError::Failed(format!("navigate: {err}"))));
                return;
            }
            self.plugin_loading = true;
            (tab, DeepSeekStage::Loading, false)
        } else {
            let tab = TabId::new();
            if let Err(err) = self.driver.create_tab(tab, desired.clone()) {
                let _ = reply.send(Err(RenderError::Failed(format!("create tab: {err}"))));
                return;
            }
            self.plugin_tab = Some(tab);
            self.plugin_loading = true;
            (tab, DeepSeekStage::Loading, false)
        };

        // A reused tab keeps its real URL (which may be a chat page); only
        // navigations change it.
        if !reuse {
            self.plugin_tab_url = Some(desired);
        }
        info!(tab = %tab, "deepseek plugin started");
        let deadline = now + job.timeout();
        self.deepseek = Some(DeepSeekRun {
            job,
            reply: Some(reply),
            tab,
            stage,
            started: now,
            deadline,
            load_deadline: now + Duration::from_secs(20),
            load_complete,
            retries: 0,
        });
    }

    fn advance_deepseek(&mut self, now: Instant) {
        let Some(mut run) = self.deepseek.take() else {
            return;
        };
        if now >= run.deadline {
            fail_deepseek(
                &mut run,
                RenderError::Timeout("deepseek plugin did not finish in time".into()),
            );
            return;
        }
        loop {
            match std::mem::replace(&mut run.stage, DeepSeekStage::Done) {
                DeepSeekStage::Done => return,
                DeepSeekStage::Loading => {
                    if !run.load_complete && now < run.load_deadline {
                        run.stage = DeepSeekStage::Loading;
                        self.deepseek = Some(run);
                        return;
                    }
                    run.stage = DeepSeekStage::Settling {
                        until: now + DEEPSEEK_SETTLE,
                    };
                    continue;
                }
                DeepSeekStage::Settling { until } => {
                    // Wait until the URL has been stable for a moment so a
                    // delayed client-side redirect does not wipe the script.
                    let target = until.max(self.plugin_url_changed_at + DEEPSEEK_URL_STABLE);
                    let cap = run.started + Duration::from_secs(20);
                    if now < target && now < cap {
                        run.stage = DeepSeekStage::Settling { until: target };
                        self.deepseek = Some(run);
                        return;
                    }
                    run.stage = DeepSeekStage::Setup { pending: None };
                    continue;
                }
                DeepSeekStage::Setup { pending } => {
                    let Some((rid, sent)) = pending else {
                        match self.driver.evaluate(run.tab, run.job.setup_script()) {
                            Ok(rid) => {
                                run.stage = DeepSeekStage::Setup {
                                    pending: Some((rid, now)),
                                };
                                self.deepseek = Some(run);
                                return;
                            }
                            Err(err) => {
                                fail_deepseek(
                                    &mut run,
                                    RenderError::Failed(format!("deepseek setup: {err}")),
                                );
                                return;
                            }
                        }
                    };
                    match self.driver.take_reply(rid) {
                        Some(reply) => match script_reply(reply) {
                            Ok(_) => {
                                run.stage = DeepSeekStage::Polling {
                                    next_poll: now,
                                    pending: None,
                                };
                                continue;
                            }
                            Err(err) => {
                                fail_deepseek(
                                    &mut run,
                                    RenderError::Failed(format!("deepseek setup: {err}")),
                                );
                                return;
                            }
                        },
                        None if now.duration_since(sent) > PROBE_REPLY_TIMEOUT => {
                            self.driver.forget_reply(rid);
                            run.stage = DeepSeekStage::Setup { pending: None };
                            self.deepseek = Some(run);
                            return;
                        }
                        None => {
                            run.stage = DeepSeekStage::Setup {
                                pending: Some((rid, sent)),
                            };
                            self.deepseek = Some(run);
                            return;
                        }
                    }
                }
                DeepSeekStage::Polling {
                    next_poll,
                    pending,
                } => {
                    if let Some((rid, sent)) = pending {
                        match self.driver.take_reply(rid) {
                            Some(reply) => match script_reply(reply) {
                                Ok(Value::Null) => {
                                    run.stage = DeepSeekStage::Polling {
                                        next_poll: now + DEEPSEEK_POLL,
                                        pending: None,
                                    };
                                    self.deepseek = Some(run);
                                    return;
                                }
                                Ok(value) => {
                                    match deepseek_outcome(&run, value, now) {
                                        Ok(result) => {
                                            if let Some(reply) = run.reply.take() {
                                                info!(
                                                    tab = %run.tab,
                                                    elapsed_ms = result
                                                        .get("elapsed_ms")
                                                        .and_then(json_u64)
                                                        .unwrap_or(0),
                                                    "deepseek plugin done"
                                                );
                                                let _ = reply.send(Ok(result));
                                            }
                                        }
                                        Err(err)
                                            if run.retries == 0
                                                && run.job.retryable()
                                                && err.message().contains("page reloaded") =>
                                        {
                                            // The document was replaced before the
                                            // operation made a side effect; run it
                                            // once more on the new page.
                                            run.retries += 1;
                                            run.load_complete = true;
                                            run.stage = DeepSeekStage::Settling {
                                                until: now + DEEPSEEK_SETTLE,
                                            };
                                            self.deepseek = Some(run);
                                            return;
                                        }
                                        Err(err) => fail_deepseek(&mut run, err),
                                    }
                                    return;
                                }
                                Err(err) => {
                                    fail_deepseek(
                                        &mut run,
                                        RenderError::Failed(format!("deepseek poll: {err}")),
                                    );
                                    return;
                                }
                            },
                            None if now.duration_since(sent) > PROBE_REPLY_TIMEOUT => {
                                self.driver.forget_reply(rid);
                                run.stage = DeepSeekStage::Polling {
                                    next_poll: now,
                                    pending: None,
                                };
                                self.deepseek = Some(run);
                                return;
                            }
                            None => {
                                run.stage = DeepSeekStage::Polling {
                                    next_poll,
                                    pending: Some((rid, sent)),
                                };
                                self.deepseek = Some(run);
                                return;
                            }
                        }
                    } else if now >= next_poll {
                        match self
                            .driver
                            .evaluate(run.tab, deepseek_poll_script().to_string())
                        {
                            Ok(rid) => {
                                run.stage = DeepSeekStage::Polling {
                                    next_poll,
                                    pending: Some((rid, now)),
                                };
                                self.deepseek = Some(run);
                                return;
                            }
                            Err(err) => {
                                fail_deepseek(
                                    &mut run,
                                    RenderError::Failed(format!("deepseek poll: {err}")),
                                );
                                return;
                            }
                        }
                    } else {
                        run.stage = DeepSeekStage::Polling {
                            next_poll,
                            pending: None,
                        };
                        self.deepseek = Some(run);
                        return;
                    }
                }
            }
        }
    }
}

/// How often the page-side DeepSeek driver is polled.
const DEEPSEEK_POLL: Duration = Duration::from_millis(400);
/// Grace period after `load` before installing the plugin script, so delayed
/// client-side redirects (for example signed-out pages) settle first.
const DEEPSEEK_SETTLE: Duration = Duration::from_millis(700);
/// The plugin tab URL must be unchanged for this long before a script runs.
const DEEPSEEK_URL_STABLE: Duration = Duration::from_millis(600);

fn fail_deepseek(run: &mut DeepSeekRun, err: RenderError) {
    if let Some(reply) = run.reply.take() {
        warn!(tab = %run.tab, error = %err.message(), "deepseek plugin failed");
        let _ = reply.send(Err(err));
    }
    run.stage = DeepSeekStage::Done;
}

/// Convert the JSON written by the page driver into an API result.
fn deepseek_outcome(
    run: &DeepSeekRun,
    value: Value,
    now: Instant,
) -> Result<Value, RenderError> {
    if let Some(err) = value.get("error").and_then(Value::as_str) {
        return Err(RenderError::Failed(format!("deepseek: {err}")));
    }
    let mut result = value.get("result").cloned().unwrap_or(Value::Null);
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "elapsed_ms".into(),
            json!(now.duration_since(run.started).as_millis() as u64),
        );
    }
    Ok(result)
}

fn fail(job: &mut Running, err: RenderError) {
    if let Some(reply) = job.reply.take() {
        warn!(tab = %job.tab, error = %err.message(), "automation render failed");
        let _ = reply.send(Err(err));
    }
    job.stage = Stage::Done;
}

fn finish(job: &mut Running, now: Instant) {
    job.response.load_complete = job.load_complete;
    job.response.elapsed_ms = now.duration_since(job.started).as_millis() as u64;
    if let Some(reply) = job.reply.take() {
        info!(tab = %job.tab, elapsed_ms = job.response.elapsed_ms, "automation render done");
        let _ = reply.send(Ok(std::mem::take(&mut job.response)));
    }
    job.stage = Stage::Done;
}

/// JS numbers cross IPC as JSON floats (`200.0`), so `as_u64` alone is not enough.
fn json_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|n| n as u64)
    })
}

fn fail_fetch(run: &mut FetchRun, err: RenderError) {
    if let Some(reply) = run.reply.take() {
        warn!(tab = %run.tab, error = %err.message(), "session fetch failed");
        let _ = reply.send(Err(err));
    }
    run.stage = FetchStage::Done;
}

/// Convert the JSON written by `page_fetch_start_script` into an API response.
fn page_fetch_response(
    run: &FetchRun,
    value: Value,
    now: Instant,
) -> Result<PageFetchResponse, RenderError> {
    if let Some(err) = value.get("error").and_then(Value::as_str) {
        return Err(RenderError::Failed(format!("page fetch: {err}")));
    }
    let status = value.get("status").and_then(json_u64).unwrap_or(0);
    if status == 0 {
        let raw = value.to_string();
        let raw = raw.chars().take(400).collect::<String>();
        return Err(RenderError::Failed(format!(
            "page fetch returned no HTTP status: {raw}"
        )));
    }
    let headers = value
        .get("headers")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|v| (name.clone(), v.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(PageFetchResponse {
        page_url: run.job.page_url.to_string(),
        url: run.job.url.to_string(),
        final_url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: status as u16,
        headers,
        body: value
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        body_length: value.get("body_length").and_then(json_u64).unwrap_or(0) as usize,
        truncated: value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        elapsed_ms: now.duration_since(run.started).as_millis() as u64,
    })
}

fn script_reply(msg: ContentToBrowser) -> Result<Value, String> {
    match msg {
        ContentToBrowser::ScriptResult { result, .. } => result,
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// Advance one job as far as it can go right now.
fn advance<D: ContentDriver>(driver: &mut D, job: &mut Running, now: Instant) {
    if job.reply.as_ref().is_some_and(|r| r.is_closed()) {
        job.stage = Stage::Done; // client disconnected
    }
    if now >= job.deadline && !matches!(job.stage, Stage::Done) {
        if let Some(rid) = job.pending_rid() {
            driver.forget_reply(rid);
        }
        if job.snapshot_taken {
            // Keep the page; report the optional steps that did not make it.
            match job.stage {
                Stage::Script { .. } => {
                    job.response.script_error = Some("timed out".into());
                    if job.job.screenshot {
                        job.response.screenshot_error = Some("timed out".into());
                    }
                }
                Stage::Screenshot { .. } => {
                    job.response.screenshot_error = Some("timed out".into())
                }
                _ => {}
            }
            finish(job, now);
        } else {
            let what = if job.load_complete {
                "page snapshot"
            } else {
                "page load"
            };
            fail(job, RenderError::Timeout(format!("{what} did not finish in time")));
        }
        return;
    }

    loop {
        let next = match &mut job.stage {
            Stage::Done => return,
            Stage::Loading => {
                let early = job.job.condition.checkable_during_load();
                if !(early || job.load_complete || now >= job.load_deadline) {
                    return;
                }
                if !job.job.condition.is_empty() {
                    Stage::WaitCondition {
                        next_poll: now,
                        pending: None,
                    }
                } else {
                    Stage::Settle {
                        until: now + job.job.wait,
                    }
                }
            }
            Stage::WaitCondition { next_poll, pending } => {
                if let Some((rid, sent)) = *pending {
                    let Some(reply) = driver.take_reply(rid) else {
                        if now >= sent + PROBE_REPLY_TIMEOUT {
                            driver.forget_reply(rid);
                            *pending = None;
                        }
                        return;
                    };
                    if matches!(script_reply(reply), Ok(Value::Bool(true))) {
                        job.response.wait_for_found = Some(true);
                        Stage::Settle {
                            until: now + job.job.wait,
                        }
                    } else {
                        *pending = None;
                        *next_poll = now + SELECTOR_POLL;
                        return;
                    }
                } else if now + SNAPSHOT_RESERVE >= job.deadline {
                    job.response.wait_for_found = Some(false);
                    Stage::Settle { until: now }
                } else if now >= *next_poll {
                    match driver.evaluate(job.tab, wait_probe_script(&job.job.condition)) {
                        Ok(rid) => {
                            *pending = Some((rid, now));
                            return;
                        }
                        Err(err) => {
                            fail(job, RenderError::Failed(format!("evaluate: {err}")));
                            return;
                        }
                    }
                } else {
                    return;
                }
            }
            Stage::Settle { until } => {
                if now < *until {
                    return;
                }
                match driver.evaluate(job.tab, snapshot_script()) {
                    Ok(rid) => Stage::Snapshot { rid },
                    Err(err) => {
                        fail(job, RenderError::Failed(format!("evaluate: {err}")));
                        return;
                    }
                }
            }
            Stage::Snapshot { rid } => {
                let Some(reply) = driver.take_reply(*rid) else {
                    return;
                };
                match script_reply(reply) {
                    Ok(snapshot) => {
                        let text = |k: &str| {
                            snapshot
                                .get(k)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        let url = text("url");
                        if !url.is_empty() {
                            job.response.final_url = url;
                        }
                        job.response.title = text("title");
                        job.response.html = text("html");
                        let full = snapshot
                            .get("html_length")
                            .and_then(json_u64)
                            .unwrap_or(0);
                        job.response.html_truncated =
                            full as usize > job.response.html.encode_utf16().count();
                        job.snapshot_taken = true;
                    }
                    Err(err) => {
                        fail(job, RenderError::Failed(format!("snapshot: {err}")));
                        return;
                    }
                }
                match next_step(driver, job, Step::Snapshot) {
                    Some(stage) => stage,
                    None => return,
                }
            }
            Stage::Network {
                hook_rid,
                log_rid,
                responses,
                requests,
            } => {
                if responses.is_none() {
                    if let Some(reply) = driver.take_reply(*hook_rid) {
                        *responses = Some(script_reply(reply).unwrap_or(Value::Null));
                    }
                }
                if requests.is_none() {
                    if let Some(reply) = driver.take_reply(*log_rid) {
                        *requests = Some(match reply {
                            ContentToBrowser::NetworkLog { requests, .. } => json!(requests),
                            _ => json!([]),
                        });
                    }
                }
                let (Some(resp), Some(reqs)) = (responses.as_ref(), requests.as_ref()) else {
                    return;
                };
                job.response.network = Some(json!({
                    "requests": reqs,
                    "responses": if resp.is_null() { json!([]) } else { resp.clone() },
                    "hook": !resp.is_null(),
                }));
                match next_step(driver, job, Step::Network) {
                    Some(stage) => stage,
                    None => return,
                }
            }
            Stage::Script { rid } => {
                let Some(reply) = driver.take_reply(*rid) else {
                    return;
                };
                match script_reply(reply) {
                    Ok(v) => job.response.script_result = Some(v),
                    Err(e) => job.response.script_error = Some(e),
                }
                match next_step(driver, job, Step::Script) {
                    Some(stage) => stage,
                    None => return,
                }
            }
            Stage::Screenshot { rid } => {
                let Some(reply) = driver.take_reply(*rid) else {
                    return;
                };
                match reply {
                    ContentToBrowser::Screenshot { result: Ok(png), .. } => {
                        job.response.screenshot_png_base64 = Some(png)
                    }
                    ContentToBrowser::Screenshot { result: Err(e), .. } => {
                        job.response.screenshot_error = Some(e)
                    }
                    other => job.response.screenshot_error = Some(format!("unexpected {other:?}")),
                }
                finish(job, now);
                return;
            }
        };
        job.stage = next;
    }
}

/// Steps after the snapshot, in order: network capture, user script, screenshot.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Step {
    Snapshot,
    Network,
    Script,
}

/// Start the next optional step after `done`; `None` means the job finished here.
fn next_step<D: ContentDriver>(driver: &mut D, job: &mut Running, done: Step) -> Option<Stage> {
    if done < Step::Network && job.job.capture_network {
        match (
            driver.evaluate(job.tab, network_collect_script().to_string()),
            driver.network_log(job.tab),
        ) {
            (Ok(hook_rid), Ok(log_rid)) => {
                return Some(Stage::Network {
                    hook_rid,
                    log_rid,
                    responses: None,
                    requests: None,
                })
            }
            (Err(e), _) | (_, Err(e)) => {
                job.response.network = Some(json!({ "error": e.to_string() }));
            }
        }
    }
    if done < Step::Script {
        if let Some(script) = job.job.script.clone() {
            match driver.evaluate(job.tab, script) {
                Ok(rid) => return Some(Stage::Script { rid }),
                Err(err) => job.response.script_error = Some(err.to_string()),
            }
        }
    }
    if job.job.screenshot {
        match driver.screenshot(job.tab) {
            Ok(rid) => return Some(Stage::Screenshot { rid }),
            Err(err) => job.response.screenshot_error = Some(err.to_string()),
        }
    }
    finish(job, Instant::now());
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PageFetchRequest, RenderRequest};
    use crate::deepseek::DeepSeekChatRequest;
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};

    /// Answers scripts with a closure; replies are available on the next tick.
    struct FakeDriver {
        next_rid: u64,
        replies: HashMap<u64, ContentToBrowser>,
        events: Vec<ContentToBrowser>,
        tabs: Vec<TabId>,
        closed: Vec<TabId>,
        connected: bool,
        responder: Box<dyn FnMut(&str) -> Option<Result<Value, String>>>,
    }

    impl FakeDriver {
        fn new(responder: impl FnMut(&str) -> Option<Result<Value, String>> + 'static) -> Self {
            Self {
                next_rid: 1,
                replies: HashMap::new(),
                events: Vec::new(),
                tabs: Vec::new(),
                closed: Vec::new(),
                connected: true,
                responder: Box::new(responder),
            }
        }
    }

    impl ContentDriver for FakeDriver {
        fn create_tab(&mut self, tab: TabId, _url: Url) -> BrowserResult<()> {
            self.tabs.push(tab);
            Ok(())
        }
        fn close_tab(&mut self, tab: TabId) {
            self.closed.push(tab);
        }
        fn navigate(&mut self, _tab: TabId, _url: Url) -> BrowserResult<()> {
            Ok(())
        }
        fn get_cookies(&mut self, _url: Url) -> BrowserResult<u64> {
            let rid = self.next_rid;
            self.next_rid += 1;
            self.replies
                .insert(rid, ContentToBrowser::Cookies { cookies: Vec::new() });
            Ok(rid)
        }
        fn set_cookies(&mut self, _url: Url, _cookies: Vec<CookieInfo>) -> BrowserResult<u64> {
            let rid = self.next_rid;
            self.next_rid += 1;
            Ok(rid)
        }
        fn evaluate(&mut self, tab: TabId, script: String) -> BrowserResult<u64> {
            let rid = self.next_rid;
            self.next_rid += 1;
            if let Some(result) = (self.responder)(&script) {
                self.replies
                    .insert(rid, ContentToBrowser::ScriptResult { tab_id: tab, result });
            }
            Ok(rid)
        }
        fn screenshot(&mut self, tab: TabId) -> BrowserResult<u64> {
            let rid = self.next_rid;
            self.next_rid += 1;
            self.replies.insert(
                rid,
                ContentToBrowser::Screenshot {
                    tab_id: tab,
                    result: Ok("iVBORw0KGgo=".into()),
                },
            );
            Ok(rid)
        }
        fn network_log(&mut self, tab: TabId) -> BrowserResult<u64> {
            let rid = self.next_rid;
            self.next_rid += 1;
            self.replies.insert(
                rid,
                ContentToBrowser::NetworkLog {
                    tab_id: tab,
                    requests: vec![browser_ipc::NetworkRequestMsg {
                        url: "https://example.com/api/items".into(),
                        method: "GET".into(),
                        destination: "empty".into(),
                        main_frame: false,
                        blocked: false,
                    }],
                },
            );
            Ok(rid)
        }
        fn take_reply(&mut self, rid: u64) -> Option<ContentToBrowser> {
            self.replies.remove(&rid)
        }
        fn forget_reply(&mut self, rid: u64) {
            self.replies.remove(&rid);
        }
        fn poll(&mut self) -> BrowserResult<Vec<ContentToBrowser>> {
            Ok(std::mem::take(&mut self.events))
        }
        fn connected(&self) -> bool {
            self.connected
        }
        fn content_pid(&self) -> Option<u32> {
            Some(42)
        }
    }

    fn snapshot() -> Value {
        // JS numbers arrive as floats over IPC.
        json!({ "url": "https://example.com/final", "title": "Example", "html": "<html><body>hi</body></html>", "html_length": 28.0 })
    }

    fn render_job(extra: impl FnOnce(&mut RenderRequest)) -> RenderJob {
        let mut req = RenderRequest {
            url: "https://example.com/".into(),
            wait_for: None,
            wait_ms: Some(0),
            timeout_ms: Some(10_000),
            script: None,
            screenshot: false,
            wait_text: None,
            wait_js: None,
            network_idle_ms: None,
            capture_network: false,
        };
        extra(&mut req);
        req.validate().unwrap()
    }

    fn submit<D: ContentDriver>(
        actor: &mut Actor<D>,
        job: RenderJob,
        now: Instant,
    ) -> oneshot::Receiver<Result<RenderResponse, RenderError>> {
        let (tx, rx) = oneshot::channel();
        actor.handle(Command::Render { job, reply: tx }, now);
        rx
    }

    fn loaded(actor: &mut Actor<FakeDriver>) {
        let tab = *actor.driver.tabs.last().unwrap();
        actor.driver.events.push(ContentToBrowser::LoadStatusChanged {
            tab_id: tab,
            loading: false,
        });
    }

    fn deepseek_job() -> DeepSeekJob {
        DeepSeekChatRequest {
            prompt: "hello".into(),
            chat_id: None,
            timeout_ms: Some(60_000),
            settle_ms: None,
            max_continues: None,
            thinking: None,
            search: None,
        }
        .validate()
        .map(DeepSeekJob::Chat)
        .unwrap()
    }

    #[test]
    fn deepseek_runs_setup_then_polls_to_completion() {
        let driver = FakeDriver::new(|script| {
            if script.contains("const CFG") {
                Some(Ok(json!(true)))
            } else if script.contains("__rbDeepSeek") {
                Some(Ok(json!({
                    "done": true,
                    "error": null,
                    "result": {
                        "chat_id": "c1",
                        "chat_url": "https://chat.deepseek.com/a/chat/s/c1",
                        "answer": "hi",
                        "continued": 1,
                        "messages": []
                    }
                })))
            } else {
                None
            }
        });
        let mut actor = Actor::new(driver, 4, 8);
        let t0 = Instant::now();
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::DeepSeek {
                job: deepseek_job(),
                reply: tx,
            },
            t0,
        );
        assert_eq!(actor.driver.tabs.len(), 1, "plugin tab is created");
        assert!(rx.try_recv().is_err());

        loaded(&mut actor);
        for i in 1..25 {
            actor.tick(t0 + Duration::from_millis(100 * i));
        }
        let result = rx.try_recv().unwrap().unwrap();
        assert_eq!(result["answer"], "hi");
        assert_eq!(result["continued"], 1);
        assert!(result["elapsed_ms"].as_u64().is_some());

        // The tab is reused for the next operation.
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::DeepSeek {
                job: deepseek_job(),
                reply: tx,
            },
            t0 + Duration::from_secs(1),
        );
        assert_eq!(actor.driver.tabs.len(), 1, "plugin tab is reused");
        for i in 0..25 {
            actor.tick(t0 + Duration::from_secs(1) + Duration::from_millis(100 * i));
        }
        assert_eq!(rx.try_recv().unwrap().unwrap()["answer"], "hi");
    }

    #[test]
    fn deepseek_reports_page_errors() {
        let driver = FakeDriver::new(|script| {
            if script.contains("const CFG") {
                Some(Ok(json!(true)))
            } else if script.contains("__rbDeepSeek") {
                Some(Ok(json!({
                    "done": true,
                    "error": "session is not signed in to DeepSeek",
                    "result": null
                })))
            } else {
                None
            }
        });
        let mut actor = Actor::new(driver, 4, 8);
        let t0 = Instant::now();
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::DeepSeek {
                job: deepseek_job(),
                reply: tx,
            },
            t0,
        );
        loaded(&mut actor);
        for i in 1..25 {
            actor.tick(t0 + Duration::from_millis(100 * i));
        }
        let err = rx.try_recv().unwrap().unwrap_err();
        assert!(err.message().contains("not signed in"), "{err:?}");
    }

    #[test]
    fn deepseek_retries_read_only_ops_after_a_reload() {
        let calls = std::cell::Cell::new(0);
        let driver = FakeDriver::new(move |script| {
            if script.contains("const CFG") {
                Some(Ok(json!(true)))
            } else if script.contains("__rbDeepSeek") {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Some(Ok(json!({
                        "done": true,
                        "error": "the DeepSeek page reloaded before the operation finished",
                        "result": null
                    })))
                } else {
                    Some(Ok(json!({
                        "done": true,
                        "error": null,
                        "result": { "chats": [] }
                    })))
                }
            } else {
                None
            }
        });
        let mut actor = Actor::new(driver, 4, 8);
        let t0 = Instant::now();
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::DeepSeek {
                job: DeepSeekJob::ListChats { limit: 10 },
                reply: tx,
            },
            t0,
        );
        loaded(&mut actor);
        for i in 1..30 {
            actor.tick(t0 + Duration::from_millis(100 * i));
        }
        let result = rx.try_recv().unwrap().unwrap();
        assert_eq!(result["chats"], json!([]));
    }

    #[test]
    fn deepseek_does_not_retry_a_chat_after_a_reload() {
        let driver = FakeDriver::new(|script| {
            if script.contains("const CFG") {
                Some(Ok(json!(true)))
            } else if script.contains("__rbDeepSeek") {
                Some(Ok(json!({
                    "done": true,
                    "error": "the DeepSeek page reloaded before the operation finished",
                    "result": null
                })))
            } else {
                None
            }
        });
        let mut actor = Actor::new(driver, 4, 8);
        let t0 = Instant::now();
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::DeepSeek {
                job: deepseek_job(),
                reply: tx,
            },
            t0,
        );
        loaded(&mut actor);
        for i in 1..30 {
            actor.tick(t0 + Duration::from_millis(100 * i));
        }
        let err = rx.try_recv().unwrap().unwrap_err();
        assert!(err.message().contains("page reloaded"), "{err:?}");
    }

    #[test]
    fn renders_snapshot_script_and_screenshot() {
        let driver = FakeDriver::new(|script| {
            Some(Ok(if script.contains("outerHTML") {
                snapshot()
            } else {
                json!(7)
            }))
        });
        let mut actor = Actor::new(driver, 2, 8);
        let t0 = Instant::now();
        let mut rx = submit(
            &mut actor,
            render_job(|r| {
                r.script = Some("document.links.length".into());
                r.screenshot = true;
            }),
            t0,
        );
        actor.tick(t0);
        assert!(rx.try_recv().is_err(), "must wait for load");
        loaded(&mut actor);
        for i in 1..5 {
            actor.tick(t0 + Duration::from_millis(10 * i));
        }
        let resp = rx.try_recv().unwrap().unwrap();
        assert_eq!(resp.final_url, "https://example.com/final");
        assert_eq!(resp.title, "Example");
        assert!(resp.html.contains("hi"));
        assert!(!resp.html_truncated);
        assert!(resp.load_complete);
        assert_eq!(resp.script_result, Some(json!(7)));
        assert_eq!(resp.screenshot_png_base64.as_deref(), Some("iVBORw0KGgo="));
        assert_eq!(actor.driver.closed.len(), 1, "tab closed after the job");
        assert!(!actor.busy());
    }

    #[test]
    fn waits_for_selector_until_it_appears() {
        let mut probes = 0;
        let driver = FakeDriver::new(move |script| {
            if script.contains("querySelector(\".feed\")") {
                probes += 1;
                Some(Ok(json!(probes >= 3)))
            } else {
                Some(Ok(snapshot()))
            }
        });
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(&mut actor, render_job(|r| r.wait_for = Some(".feed".into())), t0);
        loaded(&mut actor);
        let mut now = t0;
        for _ in 0..40 {
            now += Duration::from_millis(100);
            actor.tick(now);
        }
        let resp = rx.try_recv().unwrap().unwrap();
        assert_eq!(resp.wait_for_found, Some(true));
        assert!(now.duration_since(t0) >= Duration::from_millis(500));
    }

    #[test]
    fn selector_found_before_load_completes() {
        let driver = FakeDriver::new(|script| {
            Some(Ok(if script.contains("querySelector(\".feed\")") {
                json!(true)
            } else {
                snapshot()
            }))
        });
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(&mut actor, render_job(|r| r.wait_for = Some(".feed".into())), t0);
        let mut now = t0;
        for _ in 0..20 {
            now += Duration::from_millis(100);
            actor.tick(now);
        }
        let resp = rx.try_recv().expect("done well before the load deadline").unwrap();
        assert_eq!(resp.wait_for_found, Some(true));
        assert!(!resp.load_complete);
    }

    #[test]
    fn lost_probe_reply_is_resent() {
        let mut probes = 0;
        let driver = FakeDriver::new(move |script| {
            if script.contains("querySelector(\"h1\")") {
                probes += 1;
                // The first probe dies with the document it was sent to.
                (probes > 1).then(|| Ok(json!(true)))
            } else {
                Some(Ok(snapshot()))
            }
        });
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(&mut actor, render_job(|r| r.wait_for = Some("h1".into())), t0);
        let mut now = t0;
        for _ in 0..30 {
            now += Duration::from_millis(100);
            actor.tick(now);
        }
        let resp = rx.try_recv().expect("done after the probe is resent").unwrap();
        assert_eq!(resp.wait_for_found, Some(true));
    }

    #[test]
    fn captures_network_before_user_script() {
        let driver = FakeDriver::new(|script| {
            Some(Ok(if script.contains("outerHTML") {
                snapshot()
            } else if script.contains("__rbNet") {
                json!([{ "url": "https://example.com/api/items", "status": 200,
                         "content_type": "application/json", "body": "{\"items\":[1]}" }])
            } else {
                json!("user")
            }))
        });
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(
            &mut actor,
            render_job(|r| {
                r.capture_network = true;
                r.script = Some("1".into());
            }),
            t0,
        );
        loaded(&mut actor);
        for i in 1..6 {
            actor.tick(t0 + Duration::from_millis(10 * i));
        }
        let resp = rx.try_recv().unwrap().unwrap();
        let net = resp.network.expect("network captured");
        assert_eq!(net["hook"], true);
        assert_eq!(net["requests"][0]["destination"], "empty");
        assert_eq!(net["responses"][0]["status"], 200);
        assert_eq!(resp.script_result, Some(json!("user")));
    }

    #[test]
    fn snapshot_taken_even_if_load_never_completes() {
        let driver = FakeDriver::new(|_| Some(Ok(snapshot())));
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(&mut actor, render_job(|_| {}), t0);
        actor.tick(t0 + Duration::from_secs(1));
        assert!(rx.try_recv().is_err());
        // 70% of the 10 s budget elapsed without `load`.
        actor.tick(t0 + Duration::from_millis(7_100));
        actor.tick(t0 + Duration::from_millis(7_200));
        let resp = rx.try_recv().unwrap().unwrap();
        assert!(!resp.load_complete);
        assert_eq!(resp.title, "Example");
    }

    #[test]
    fn times_out_when_snapshot_never_answers() {
        let driver = FakeDriver::new(|_| None);
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();
        let mut rx = submit(&mut actor, render_job(|_| {}), t0);
        loaded(&mut actor);
        actor.tick(t0);
        actor.tick(t0 + Duration::from_secs(11));
        assert!(matches!(rx.try_recv().unwrap(), Err(RenderError::Timeout(_))));
        assert_eq!(actor.driver.closed.len(), 1);
    }

    fn fetch_job(url: &str) -> crate::api::PageFetchJob {
        PageFetchRequest {
            url: url.into(),
            page_url: None,
            method: None,
            headers: BTreeMap::new(),
            body: None,
            body_base64: None,
            timeout_ms: Some(5_000),
        }
        .validate()
        .unwrap()
    }

    #[test]
    fn js_numbers_parse_from_floats() {
        assert_eq!(json_u64(&json!(200)), Some(200));
        assert_eq!(json_u64(&json!(200.0)), Some(200));
        assert_eq!(json_u64(&json!(-1.0)), None);
        assert_eq!(json_u64(&json!("200")), None);
    }

    #[test]
    fn page_fetch_uses_a_persistent_tab() {
        let mut polled = false;
        let driver = FakeDriver::new(move |script| {
            if script.contains("__rbSession.done") {
                // First poll runs before the fetch settles; later ones return the result.
                if !polled {
                    polled = true;
                    Some(Ok(Value::Null))
                } else {
                    Some(Ok(json!({
                        "done": true,
                        "ok": true,
                        "status": 200.0,
                        "url": "https://example.com/api/me",
                        "headers": { "content-type": "application/json" },
                        "body": "{\"ok\":true}",
                        "body_length": 11.0,
                        "truncated": false
                    })))
                }
            } else if script.contains("window.__rbSession") {
                Some(Ok(json!(true)))
            } else {
                None
            }
        });
        let mut actor = Actor::new(driver, 1, 8);
        let t0 = Instant::now();

        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::Fetch {
                job: fetch_job("https://example.com/api/me"),
                reply: tx,
            },
            t0,
        );
        actor.tick(t0);
        let tab = actor.driver.tabs[0];
        actor.driver.events.push(ContentToBrowser::LoadStatusChanged {
            tab_id: tab,
            loading: false,
        });
        let mut now = t0;
        for _ in 0..10 {
            now += Duration::from_millis(150);
            actor.tick(now);
        }
        let resp = rx.try_recv().unwrap().unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.final_url, "https://example.com/api/me");
        assert_eq!(resp.body, "{\"ok\":true}");
        assert_eq!(actor.driver.tabs.len(), 1);

        // A second fetch on the same page reuses the tab instead of navigating a new one.
        let (tx, mut rx) = oneshot::channel();
        actor.handle(
            Command::Fetch {
                job: fetch_job("https://example.com/api/other"),
                reply: tx,
            },
            now,
        );
        for _ in 0..10 {
            now += Duration::from_millis(150);
            actor.tick(now);
        }
        let resp = rx.try_recv().unwrap().unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(actor.driver.tabs.len(), 1, "persistent session tab reused");
    }

    #[test]
    fn crash_fails_job_and_limits_apply() {
        let driver = FakeDriver::new(|_| None);
        let mut actor = Actor::new(driver, 1, 1);
        let t0 = Instant::now();
        let mut first = submit(&mut actor, render_job(|_| {}), t0);
        let mut second = submit(&mut actor, render_job(|_| {}), t0);
        let mut third = submit(&mut actor, render_job(|_| {}), t0);
        assert_eq!(actor.driver.tabs.len(), 1, "max_tabs = 1");
        assert!(matches!(third.try_recv().unwrap(), Err(RenderError::Busy)));

        let tab = actor.driver.tabs[0];
        actor.driver.events.push(ContentToBrowser::TabCrashed {
            tab_id: tab,
            reason: "oom".into(),
        });
        actor.tick(t0);
        assert!(matches!(first.try_recv().unwrap(), Err(RenderError::Failed(m)) if m.contains("oom")));
        assert_eq!(actor.driver.tabs.len(), 2, "queued job started after the crash");
        assert!(second.try_recv().is_err());

        actor.driver.connected = false;
        actor.tick(t0);
        assert!(matches!(second.try_recv().unwrap(), Err(RenderError::Failed(_))));
    }
}
