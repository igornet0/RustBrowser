//! Single-threaded job runner that owns the Content Process session.
//!
//! Each render job is a small state machine advanced by [`Actor::tick`]:
//! load → wait for selector → settle → snapshot → user script → screenshot → done.

use crate::api::{
    network_collect_script, snapshot_script, wait_probe_script, Health, RenderError, RenderJob,
    RenderResponse,
};
use browser_core::{BrowserResult, TabId};
use browser_ipc::{ContentToBrowser, RemoteContentSession, PROTOCOL_VERSION};
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

/// What the actor needs from the Content Process (a seam for tests).
pub trait ContentDriver {
    fn create_tab(&mut self, tab: TabId, url: Url) -> BrowserResult<()>;
    fn close_tab(&mut self, tab: TabId);
    fn evaluate(&mut self, tab: TabId, script: String) -> BrowserResult<u64>;
    fn screenshot(&mut self, tab: TabId) -> BrowserResult<u64>;
    fn network_log(&mut self, tab: TabId) -> BrowserResult<u64>;
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
    fn evaluate(&mut self, tab: TabId, script: String) -> BrowserResult<u64> {
        self.evaluate_script(tab, script)
    }
    fn screenshot(&mut self, tab: TabId) -> BrowserResult<u64> {
        self.capture_screenshot(tab)
    }
    fn network_log(&mut self, tab: TabId) -> BrowserResult<u64> {
        RemoteContentSession::network_log(self, tab)
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

pub enum Command {
    Render { job: RenderJob, reply: RenderReply },
    Health { reply: oneshot::Sender<Health> },
}

#[derive(Debug)]
enum Stage {
    Loading,
    WaitCondition { next_poll: Instant, pending: Option<u64> },
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
            Stage::WaitCondition { pending, .. } => pending,
            Stage::Snapshot { rid } | Stage::Script { rid } | Stage::Screenshot { rid } => Some(rid),
            Stage::Network { hook_rid, .. } => Some(hook_rid),
            _ => None,
        }
    }
}

pub struct Actor<D: ContentDriver> {
    driver: D,
    max_tabs: usize,
    max_queue: usize,
    active: Vec<Running>,
    queue: VecDeque<(RenderJob, RenderReply)>,
}

impl<D: ContentDriver> Actor<D> {
    pub fn new(driver: D, max_tabs: usize, max_queue: usize) -> Self {
        Self {
            driver,
            max_tabs: max_tabs.max(1),
            max_queue,
            active: Vec::new(),
            queue: VecDeque::new(),
        }
    }

    pub fn busy(&self) -> bool {
        !self.active.is_empty() || !self.queue.is_empty()
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
        let events = match self.driver.poll() {
            Ok(events) => events,
            Err(err) => {
                warn!(%err, "content poll failed");
                Vec::new()
            }
        };
        for event in events {
            self.route_event(event);
        }
        if !self.driver.connected() {
            // Tabs do not survive a content restart: fail what was running.
            for job in &mut self.active {
                fail(job, RenderError::Failed("content process crashed".into()));
            }
        }

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
        self.start_queued(now);
    }

    fn route_event(&mut self, event: ContentToBrowser) {
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
                if !(job.load_complete || now >= job.load_deadline) {
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
                if let Some(rid) = *pending {
                    let Some(reply) = driver.take_reply(rid) else {
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
                            *pending = Some(rid);
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
                            .and_then(Value::as_u64)
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
    use crate::api::RenderRequest;
    use serde_json::json;
    use std::collections::HashMap;

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
        json!({ "url": "https://example.com/final", "title": "Example", "html": "<html><body>hi</body></html>", "html_length": 28 })
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
