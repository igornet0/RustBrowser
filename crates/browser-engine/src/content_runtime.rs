//! Headless Content Process runtime: Servo + SoftwareRenderingContext + IPC.
//!
//! Runs in a **separate OS process**. Does not load passwords, browser SQLite,
//! or UI. Browser Process is the source of truth for session/tabs.

use crate::events::EngineViewId;
use crate::identity::build_preferences;
use browser_core::{BrowserError, BrowserResult, TabId};
use crate::ipc_keymap::{self, parse_ipc_key};
use browser_ipc::{
    connect_unix, validate_browser_to_content, BrowserToContent, ContentToBrowser, Envelope,
    FrameBuffer, InputEventMsg, IpcReader, IpcWriter, NetworkRouteMsg, PROTOCOL_VERSION,
};
use euclid::Scale;
use servo::{
    CompositionEvent, CompositionState, ImeEvent, InputEvent, KeyState, KeyboardEvent, LoadStatus,
    Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, RenderingContext,
    Servo, ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
    WheelDelta, WheelEvent, WheelMode,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use url::Url;
use winit::dpi::PhysicalSize;

struct WakeFlag(Arc<AtomicBool>);

impl servo::EventLoopWaker for WakeFlag {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(WakeFlag(self.0.clone()))
    }
    fn wake(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Shared {
    events: Mutex<Vec<ContentToBrowser>>,
    view_ids: RefCell<HashMap<servo::WebViewId, TabId>>,
    dirty: Mutex<bool>,
}

impl Shared {
    fn push(&self, msg: ContentToBrowser) {
        if let Ok(mut q) = self.events.lock() {
            q.push(msg);
        }
    }
    fn tab_for(&self, webview: &WebView) -> Option<TabId> {
        self.view_ids.borrow().get(&webview.id()).copied()
    }
}

struct Delegate {
    shared: Rc<Shared>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        if let Ok(mut d) = self.shared.dirty.lock() {
            *d = true;
        }
        let _ = webview;
    }

    fn notify_url_changed(&self, webview: WebView, url: Url) {
        if let Some(tab_id) = self.shared.tab_for(&webview) {
            info!(%tab_id, %url, "content navigation");
            self.shared.push(ContentToBrowser::UrlChanged { tab_id, url });
        }
    }

    fn notify_page_title_changed(&self, webview: WebView, title: Option<String>) {
        if let Some(tab_id) = self.shared.tab_for(&webview) {
            self.shared.push(ContentToBrowser::TitleChanged {
                tab_id,
                title: title.unwrap_or_default(),
            });
        }
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if let Some(tab_id) = self.shared.tab_for(&webview) {
            let loading = !matches!(status, LoadStatus::Complete);
            self.shared
                .push(ContentToBrowser::LoadStatusChanged { tab_id, loading });
            if !loading {
                if let Some(url) = webview.url() {
                    self.shared
                        .push(ContentToBrowser::NavigationFinished { tab_id, url });
                }
            }
        }
    }

    fn notify_history_changed(&self, webview: WebView, _entries: Vec<Url>, _current: usize) {
        if let Some(tab_id) = self.shared.tab_for(&webview) {
            self.shared.push(ContentToBrowser::HistoryChanged {
                tab_id,
                can_go_back: webview.can_go_back(),
                can_go_forward: webview.can_go_forward(),
            });
        }
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        error!(%reason, "content webview crashed");
        if let Some(tab_id) = self.shared.tab_for(&webview) {
            self.shared
                .push(ContentToBrowser::TabCrashed { tab_id, reason });
        }
    }
}

struct ContentRuntime {
    servo: Servo,
    context: Rc<SoftwareRenderingContext>,
    views: HashMap<TabId, WebView>,
    focused: Option<TabId>,
    shared: Rc<Shared>,
    page_size: PhysicalSize<u32>,
    scale: f64,
    wake: Arc<AtomicBool>,
    last_frame: Instant,
}

impl ContentRuntime {
    fn new(storage: Option<PathBuf>) -> BrowserResult<Self> {
        let page_size = PhysicalSize::new(1280, 720);
        let context = Rc::new(
            SoftwareRenderingContext::new(page_size)
                .map_err(|e| BrowserError::engine(format!("SoftwareRenderingContext: {e:?}")))?,
        );
        let _ = context.make_current();

        let mut opts = servo::Opts::default();
        if let Some(dir) = storage {
            let _ = std::fs::create_dir_all(&dir);
            opts.config_dir = Some(dir);
        }

        let wake = Arc::new(AtomicBool::new(false));
        let preferences = build_preferences(None);
        let servo = ServoBuilder::default()
            .opts(opts)
            .preferences(preferences)
            .event_loop_waker(Box::new(WakeFlag(wake.clone())))
            .build();

        let shared = Rc::new(Shared {
            events: Mutex::new(Vec::new()),
            view_ids: RefCell::new(HashMap::new()),
            dirty: Mutex::new(true),
        });

        Ok(Self {
            servo,
            context,
            views: HashMap::new(),
            focused: None,
            shared,
            page_size,
            scale: 1.0,
            wake,
            last_frame: Instant::now(),
        })
    }

    fn create_tab(&mut self, tab_id: TabId, url: Option<Url>) -> BrowserResult<()> {
        if self.views.contains_key(&tab_id) {
            return Ok(());
        }
        let delegate = Rc::new(Delegate {
            shared: self.shared.clone(),
        });
        let mut builder = WebViewBuilder::new(
            &self.servo,
            self.context.clone() as Rc<dyn RenderingContext>,
        )
        .hidpi_scale_factor(Scale::new(self.scale as f32))
        .delegate(delegate);
        if let Some(url) = url {
            builder = builder.url(url);
        }
        let webview = builder.build();
        self.shared
            .view_ids
            .borrow_mut()
            .insert(webview.id(), tab_id);
        webview.resize(self.page_size);
        webview.show();
        webview.focus();
        self.views.insert(tab_id, webview);
        self.focused = Some(tab_id);
        self.servo.spin_event_loop();
        info!(%tab_id, "content tab created");
        Ok(())
    }

    fn close_tab(&mut self, tab_id: TabId) {
        if let Some(wv) = self.views.remove(&tab_id) {
            self.shared.view_ids.borrow_mut().remove(&wv.id());
        }
        if self.focused == Some(tab_id) {
            self.focused = self.views.keys().next().copied();
        }
        self.servo.spin_event_loop();
    }

    fn navigate(&mut self, tab_id: TabId, url: Url) -> BrowserResult<()> {
        let wv = self
            .views
            .get(&tab_id)
            .ok_or_else(|| BrowserError::engine(format!("unknown tab {tab_id}")))?;
        self.shared.push(ContentToBrowser::NavigationStarted {
            tab_id,
            url: url.clone(),
        });
        wv.load(url);
        self.servo.spin_event_loop();
        Ok(())
    }

    fn reload(&mut self, tab_id: TabId) -> BrowserResult<()> {
        let wv = self
            .views
            .get(&tab_id)
            .ok_or_else(|| BrowserError::engine(format!("unknown tab {tab_id}")))?;
        wv.reload();
        self.servo.spin_event_loop();
        Ok(())
    }

    fn go_back(&mut self, tab_id: TabId) -> BrowserResult<()> {
        let wv = self
            .views
            .get(&tab_id)
            .ok_or_else(|| BrowserError::engine(format!("unknown tab {tab_id}")))?;
        if wv.can_go_back() {
            let _ = wv.go_back(1);
            self.servo.spin_event_loop();
        }
        Ok(())
    }

    fn go_forward(&mut self, tab_id: TabId) -> BrowserResult<()> {
        let wv = self
            .views
            .get(&tab_id)
            .ok_or_else(|| BrowserError::engine(format!("unknown tab {tab_id}")))?;
        if wv.can_go_forward() {
            let _ = wv.go_forward(1);
            self.servo.spin_event_loop();
        }
        Ok(())
    }

    fn suspend_tab(&mut self, tab_id: TabId) {
        if let Some(wv) = self.views.get(&tab_id) {
            wv.hide();
        }
        self.servo.spin_event_loop();
    }

    fn resume_tab(&mut self, tab_id: TabId) {
        self.focus(tab_id);
    }

    fn focus(&mut self, tab_id: TabId) {
        for (id, wv) in &self.views {
            if *id == tab_id {
                wv.show();
                wv.focus();
            } else {
                wv.hide();
            }
        }
        self.focused = Some(tab_id);
        self.servo.spin_event_loop();
    }

    fn resize(&mut self, width: u32, height: u32, scale: f64) {
        let size = PhysicalSize::new(width.max(1).min(8192), height.max(1).min(8192));
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.page_size = size;
        let _ = self.context.resize(size);
        for wv in self.views.values() {
            wv.resize(size);
            wv.set_hidpi_scale_factor(Scale::new(self.scale as f32));
        }
        if let Ok(mut d) = self.shared.dirty.lock() {
            *d = true;
        }
        self.servo.spin_event_loop();
    }

    fn handle_input(&mut self, tab_id: TabId, event: InputEventMsg) {
        let Some(wv) = self.views.get(&tab_id).cloned() else {
            return;
        };
        match event {
            InputEventMsg::MouseMove { x, y } => {
                let p = servo::DevicePoint::new(x as f32, y as f32);
                wv.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(p.into())));
            }
            InputEventMsg::MouseButton {
                x,
                y,
                button,
                pressed,
            } => {
                let p = servo::DevicePoint::new(x as f32, y as f32);
                let btn = match button {
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    _ => MouseButton::Left,
                };
                let action = if pressed {
                    MouseButtonAction::Down
                } else {
                    MouseButtonAction::Up
                };
                if pressed {
                    wv.focus();
                }
                wv.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                    action,
                    btn,
                    p.into(),
                )));
            }
            InputEventMsg::MouseWheel { x, y, dx, dy } => {
                let p = servo::DevicePoint::new(x as f32, y as f32);
                wv.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta {
                        x: dx,
                        y: dy,
                        z: 0.0,
                        mode: WheelMode::DeltaPixel,
                    },
                    p.into(),
                )));
            }
            InputEventMsg::Key {
                key,
                pressed,
                modifiers,
            } => {
                let state = if pressed {
                    KeyState::Down
                } else {
                    KeyState::Up
                };
                let servo_key = parse_ipc_key(&key);
                let mut mods = Modifiers::empty();
                if modifiers & ipc_keymap::mods::SHIFT != 0 {
                    mods.insert(Modifiers::SHIFT);
                }
                if modifiers & ipc_keymap::mods::CONTROL != 0 {
                    mods.insert(Modifiers::CONTROL);
                }
                if modifiers & ipc_keymap::mods::ALT != 0 {
                    mods.insert(Modifiers::ALT);
                }
                if modifiers & ipc_keymap::mods::META != 0 {
                    mods.insert(Modifiers::META);
                }
                let keyboard = KeyboardEvent::new_without_event(
                    state,
                    servo_key,
                    servo::Code::Unidentified,
                    servo::Location::Standard,
                    mods,
                    false,
                    false,
                );
                wv.notify_input_event(InputEvent::Keyboard(keyboard));
            }
            InputEventMsg::Text { text } => {
                wv.notify_input_event(InputEvent::Ime(ImeEvent::Composition(CompositionEvent {
                    state: CompositionState::End,
                    data: text,
                })));
            }
        }
        self.servo.spin_event_loop();
    }

    fn capture_frame(&mut self, tab_id: TabId) -> Option<FrameBuffer> {
        let wv = self.views.get(&tab_id)?;
        wv.paint();
        let w = self.page_size.width as i32;
        let h = self.page_size.height as i32;
        let rect = euclid::Box2D::new(euclid::Point2D::new(0, 0), euclid::Point2D::new(w, h));
        let img = self.context.read_to_image(rect)?;
        Some(FrameBuffer {
            width: img.width(),
            height: img.height(),
            rgba: img.into_raw(),
        })
    }

    fn drain_events(&self) -> Vec<ContentToBrowser> {
        self.shared
            .events
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    fn spin(&mut self) {
        self.wake.store(false, Ordering::SeqCst);
        self.servo.spin_event_loop();
    }
}

/// Entry point for `--content-process <socket> --content-storage <dir>`.
pub fn run_content_process(socket: &Path, storage: Option<PathBuf>) -> BrowserResult<()> {
    info!(
        pid = std::process::id(),
        socket = %socket.display(),
        "content process starting"
    );

    // Browser listens; we connect as client after a short wait for the socket.
    let mut stream = None;
    for _ in 0..100 {
        match connect_unix(socket) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let stream = stream.ok_or_else(|| BrowserError::engine("content ipc connect timeout"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .ok();
    let mut writer = IpcWriter::new(stream.try_clone().map_err(|e| BrowserError::engine(e.to_string()))?);
    let mut reader = IpcReader::new(stream);

    let mut runtime = ContentRuntime::new(storage)?;
    let mut next_id = 1u64;

    // Wait for Hello
    let mut generation = 0u64;
    let mut out_seq = 1u64;
    loop {
        match reader.try_recv::<BrowserToContent>() {
            Ok(Some(env)) => {
                if let Err(err) = validate_browser_to_content(&env.payload) {
                    warn!(%err, "invalid hello payload");
                    continue;
                }
                if let BrowserToContent::Hello { protocol_version } = env.payload {
                    generation = env.generation.max(1);
                    writer.generation = generation;
                    if protocol_version != PROTOCOL_VERSION {
                        writer
                            .send(&Envelope::with_generation(
                                env.request_id,
                                generation,
                                ContentToBrowser::ProtocolMismatch {
                                    expected: PROTOCOL_VERSION,
                                    got: protocol_version,
                                },
                            ))
                            .map_err(|e| BrowserError::engine(e.to_string()))?;
                        return Err(BrowserError::engine("protocol mismatch"));
                    }
                    writer
                        .send(&Envelope::with_generation(
                            env.request_id,
                            generation,
                            ContentToBrowser::HelloAck {
                                protocol_version: PROTOCOL_VERSION,
                                pid: std::process::id(),
                            },
                        ))
                        .map_err(|e| BrowserError::engine(e.to_string()))?;
                    writer
                        .send(&Envelope::with_generation(
                            0,
                            generation,
                            ContentToBrowser::Ready,
                        ))
                        .map_err(|e| BrowserError::engine(e.to_string()))?;
                    break;
                }
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(BrowserError::engine(e.to_string())),
        }
    }

    reader.expected_generation = Some(generation);
    info!(generation, "content process ready");
    let mut running = true;

    let mut send_out = |writer: &mut IpcWriter, rid: u64, seq: u64, msg: ContentToBrowser| {
        writer.generation = generation;
        writer.send(&Envelope::full(rid, generation, seq, msg))
    };

    while running {
        runtime.spin();

        match reader.try_recv::<BrowserToContent>() {
            Ok(Some(env)) => {
                if env.generation != 0 && env.generation != generation {
                    warn!(
                        expected = generation,
                        got = env.generation,
                        "stale browser generation — ignore"
                    );
                    continue;
                }
                if let Err(err) = validate_browser_to_content(&env.payload) {
                    warn!(%err, "invalid browser→content payload — ignore");
                    continue;
                }
                let rid = env.request_id;
                match env.payload {
                    BrowserToContent::Hello { .. } => {}
                    BrowserToContent::CreateTab { tab_id, url } => {
                        if let Err(err) = runtime.create_tab(tab_id, url) {
                            let seq = out_seq;
                            out_seq += 1;
                            let _ = send_out(
                                &mut writer,
                                rid,
                                seq,
                                ContentToBrowser::Error {
                                    tab_id: Some(tab_id),
                                    message: err.to_string(),
                                },
                            );
                        }
                    }
                    BrowserToContent::Navigate { tab_id, url } => {
                        if let Err(err) = runtime.navigate(tab_id, url) {
                            let seq = out_seq;
                            out_seq += 1;
                            let _ = send_out(
                                &mut writer,
                                rid,
                                seq,
                                ContentToBrowser::Error {
                                    tab_id: Some(tab_id),
                                    message: err.to_string(),
                                },
                            );
                        }
                    }
                    BrowserToContent::Reload { tab_id } => {
                        let _ = runtime.reload(tab_id);
                    }
                    BrowserToContent::GoBack { tab_id } => {
                        let _ = runtime.go_back(tab_id);
                    }
                    BrowserToContent::GoForward { tab_id } => {
                        let _ = runtime.go_forward(tab_id);
                    }
                    BrowserToContent::Stop { .. } => {}
                    BrowserToContent::CloseTab { tab_id } => runtime.close_tab(tab_id),
                    BrowserToContent::FocusTab { tab_id } => runtime.focus(tab_id),
                    BrowserToContent::SuspendTab { tab_id } => runtime.suspend_tab(tab_id),
                    BrowserToContent::ResumeTab { tab_id } => runtime.resume_tab(tab_id),
                    BrowserToContent::Resize {
                        width,
                        height,
                        scale_factor,
                    } => runtime.resize(width, height, scale_factor),
                    BrowserToContent::SetViewport {
                        tab_id,
                        width,
                        height,
                        scale_factor,
                    } => {
                        runtime.focus(tab_id);
                        runtime.resize(width, height, scale_factor);
                    }
                    BrowserToContent::Input { tab_id, event } => {
                        runtime.handle_input(tab_id, event);
                    }
                    BrowserToContent::RequestFrame { tab_id } => {
                        if let Some(frame) = runtime.capture_frame(tab_id) {
                            let seq = out_seq;
                            out_seq += 1;
                            let _ = send_out(
                                &mut writer,
                                rid,
                                seq,
                                ContentToBrowser::Frame { tab_id, frame },
                            );
                        }
                    }
                    BrowserToContent::SetNetworkRoute { mode } => {
                        // Boundary prep only — full Network Core is P2.
                        match mode {
                            NetworkRouteMsg::Direct => {
                                info!("content network route: direct");
                            }
                            NetworkRouteMsg::Proxy { scheme, host, port } => {
                                info!(%scheme, %host, port, "content network route: proxy (not applied yet)");
                            }
                            NetworkRouteMsg::Unavailable { reason } => {
                                warn!(%reason, "content network route unavailable");
                            }
                        }
                    }
                    BrowserToContent::Heartbeat => {
                        let _ = send_out(&mut writer, rid, 0, ContentToBrowser::HeartbeatAck);
                    }
                    BrowserToContent::Shutdown => {
                        let _ = send_out(&mut writer, rid, 0, ContentToBrowser::ShutdownAck);
                        running = false;
                    }
                }
            }
            Ok(None) => {}
            Err(browser_ipc::IpcError::Disconnected) => {
                warn!("browser disconnected — content exit");
                running = false;
            }
            Err(e) => {
                error!(?e, "ipc recv");
                running = false;
            }
        }

        for msg in runtime.drain_events() {
            next_id += 1;
            let seq = out_seq;
            out_seq += 1;
            if send_out(&mut writer, next_id, seq, msg).is_err() {
                running = false;
                break;
            }
        }

        // Throttled automatic frame for focused tab.
        let dirty = runtime
            .shared
            .dirty
            .lock()
            .map(|d| *d)
            .unwrap_or(false);
        if dirty && runtime.last_frame.elapsed() > Duration::from_millis(100) {
            if let Some(tab_id) = runtime.focused {
                if let Some(frame) = runtime.capture_frame(tab_id) {
                    next_id += 1;
                    let seq = out_seq;
                    out_seq += 1;
                    let _ = send_out(
                        &mut writer,
                        next_id,
                        seq,
                        ContentToBrowser::Frame { tab_id, frame },
                    );
                    if let Ok(mut d) = runtime.shared.dirty.lock() {
                        *d = false;
                    }
                    runtime.last_frame = Instant::now();
                }
            }
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    info!("content process shutdown");
    Ok(())
}

/// Unused import hush for EngineViewId in this module (kept for future mapping helpers).
#[allow(dead_code)]
fn _engine_view(tab: TabId) -> EngineViewId {
    EngineViewId(tab)
}
