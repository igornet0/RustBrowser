//! Servo embedding backend using the official crates.io `servo` 0.5.0 API.
//!
//! Pattern follows `servo::examples::winit_minimal` and servoshell:
//! `WindowRenderingContext` + `OffscreenRenderingContext` for page content
//! under native chrome.

use browser_core::{BrowserError, BrowserResult, ProxyConfig, Route};
use crate::events::{EngineEvent, EngineViewId};
use crate::identity::build_preferences;
use crate::keyutils::keyboard_event_from_winit;
use crate::traits::BrowserEngine;
use euclid::Scale;
use servo::{
    CompositionEvent, CompositionState, ImeEvent, InputEvent, LoadStatus, MouseButton,
    MouseButtonAction, MouseButtonEvent, MouseMoveEvent, OffscreenRenderingContext, PrefValue,
    PermissionFeature, PermissionRequest, RenderingContext, Servo, ServoBuilder, WebView,
    WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};
use url::Url;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, Ime, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent,
};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::{DisplayHandle, WindowHandle};

/// Configuration captured when the window is ready.
pub struct ServoEngineConfig<'a> {
    pub display_handle: DisplayHandle<'a>,
    pub window_handle: WindowHandle<'a>,
    pub window_size: PhysicalSize<u32>,
    pub scale_factor: f64,
    pub event_loop_waker: Box<dyn servo::EventLoopWaker>,
    /// Servo engine storage (cookies / site data). Prefer the active profile `storage/` dir.
    pub config_dir: Option<std::path::PathBuf>,
    /// Optional hosts-format file for domain → IP overrides (`Opts.host_file`).
    pub host_file: Option<std::path::PathBuf>,
    /// BCP-47 locale for `Accept-Language` (e.g. `ru-RU`). Empty / None → system locale.
    pub locale: Option<String>,
}

struct ViewState {
    webview: WebView,
}

struct SharedState {
    events: Mutex<Vec<EngineEvent>>,
    view_ids: RefCell<HashMap<servo::WebViewId, EngineViewId>>,
    request_redraw: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// Set when Servo has a new composited frame that needs `paint()`.
    page_dirty: Mutex<bool>,
    /// Returns true if the origin+feature is already Allowed in PermissionManager.
    permission_allowed: Mutex<Option<Box<dyn Fn(String, String) -> bool + Send>>>,
}

impl SharedState {
    fn push(&self, event: EngineEvent) {
        if matches!(event, EngineEvent::NewFrameReady { .. }) {
            if let Ok(mut dirty) = self.page_dirty.lock() {
                *dirty = true;
            }
        }
        if let Ok(mut q) = self.events.lock() {
            q.push(event);
        }
    }

    fn view_id_for(&self, webview: &WebView) -> Option<EngineViewId> {
        self.view_ids.borrow().get(&webview.id()).copied()
    }

    fn mark_page_dirty(&self) {
        if let Ok(mut dirty) = self.page_dirty.lock() {
            *dirty = true;
        }
    }

    fn take_page_dirty(&self) -> bool {
        self.page_dirty
            .lock()
            .map(|mut dirty| std::mem::take(&mut *dirty))
            .unwrap_or(false)
    }
}

/// Holds Servo + surfaces. Lives on the UI thread (Rc / !Send).
pub struct ServoEngine {
    servo: Servo,
    window_context: Rc<WindowRenderingContext>,
    page_context: Rc<OffscreenRenderingContext>,
    views: HashMap<EngineViewId, ViewState>,
    focused: Option<EngineViewId>,
    shared: Rc<SharedState>,
    page_size: PhysicalSize<u32>,
    /// Physical-pixel origin of the page content inside the window (matches PaintCallback).
    content_origin_x: f64,
    content_origin_y: f64,
    scale_factor: f64,
    page_zoom: f32,
    last_mouse: PhysicalPosition<f64>,
    modifiers: ModifiersState,
}

impl ServoEngine {
    pub fn new(config: ServoEngineConfig<'_>) -> BrowserResult<Self> {
        let window_context = Rc::new(
            WindowRenderingContext::new(
                config.display_handle,
                config.window_handle,
                config.window_size,
            )
            .map_err(|e| BrowserError::engine(format!("WindowRenderingContext: {e:?}")))?,
        );
        let _ = window_context.make_current();

        // Reserve space for chrome; page fills the remainder (min 1x1).
        // Match browser-ui initial toolbar estimate (~96 CSS px).
        let toolbar_px = (96.0 * config.scale_factor).round() as u32;
        let page_h = config.window_size.height.saturating_sub(toolbar_px).max(1);
        let page_size = PhysicalSize::new(config.window_size.width.max(1), page_h);
        let page_context = Rc::new(window_context.offscreen_context(page_size));

        let mut opts = servo::Opts::default();
        if let Some(dir) = config.config_dir {
            if let Err(err) = std::fs::create_dir_all(&dir) {
                warn!(?err, "could not create servo config dir");
            } else {
                opts.config_dir = Some(dir);
            }
        }
        if let Some(host_file) = config.host_file {
            if host_file.is_file() {
                info!(path = %host_file.display(), "servo host_file enabled");
                opts.host_file = Some(host_file);
            } else {
                warn!(
                    path = %host_file.display(),
                    "host_file path set but file missing — using system DNS"
                );
            }
        }

        let preferences = build_preferences(config.locale.as_deref());
        let locale_label = if preferences.intl_locale_override.is_empty() {
            "(system)".to_string()
        } else {
            preferences.intl_locale_override.clone()
        };
        info!(
            user_agent = %preferences.user_agent,
            locale = %locale_label,
            "network identity configured"
        );

        let servo = ServoBuilder::default()
            .opts(opts)
            .preferences(preferences)
            .event_loop_waker(config.event_loop_waker)
            .build();
        // Do not call servo.setup_logging(): the app already installed tracing-subscriber.
        info!("servo engine initialized");

        let shared = Rc::new(SharedState {
            events: Mutex::new(Vec::new()),
            view_ids: RefCell::new(HashMap::new()),
            request_redraw: Mutex::new(None),
            page_dirty: Mutex::new(true),
            permission_allowed: Mutex::new(None),
        });

        Ok(Self {
            servo,
            window_context,
            page_context,
            views: HashMap::new(),
            focused: None,
            shared,
            page_size,
            content_origin_x: 0.0,
            content_origin_y: toolbar_px as f64,
            scale_factor: config.scale_factor,
            page_zoom: 1.0,
            last_mouse: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::default(),
        })
    }

    pub fn set_redraw_callback(&self, cb: impl Fn() + Send + 'static) {
        if let Ok(mut slot) = self.shared.request_redraw.lock() {
            *slot = Some(Box::new(cb));
        }
    }

    /// Install a permission lookup: `(origin, feature) -> already_allowed`.
    /// Default remains deny when this returns false / is unset.
    pub fn set_permission_checker(&self, checker: impl Fn(String, String) -> bool + Send + 'static) {
        if let Ok(mut slot) = self.shared.permission_allowed.lock() {
            *slot = Some(Box::new(checker));
        }
    }

    pub fn clear_redraw_callback(&self) {
        if let Ok(mut slot) = self.shared.request_redraw.lock() {
            *slot = None;
        }
    }

    pub fn glow_context(&self) -> Arc<glow::Context> {
        self.window_context.glow_gl_api()
    }

    pub fn set_content_origin_y(&mut self, y: f64) {
        self.content_origin_y = y;
    }

    /// Set the page content rect in physical window pixels (must match the blit rect).
    pub fn set_content_rect(&mut self, origin_x: f64, origin_y: f64) {
        self.content_origin_x = origin_x.max(0.0);
        self.content_origin_y = origin_y.max(0.0);
    }

    pub fn content_origin_y(&self) -> f64 {
        self.content_origin_y
    }

    pub fn content_origin_x(&self) -> f64 {
        self.content_origin_x
    }

    pub fn page_size(&self) -> PhysicalSize<u32> {
        self.page_size
    }

    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    pub fn page_zoom(&self) -> f32 {
        self.page_zoom
    }

    /// Apply page zoom to every open WebView (and remember for new views).
    pub fn set_page_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(0.5, 2.0);
        self.page_zoom = zoom;
        for state in self.views.values() {
            state.webview.set_page_zoom(zoom);
        }
        self.shared.mark_page_dirty();
    }

    pub fn last_mouse(&self) -> PhysicalPosition<f64> {
        self.last_mouse
    }

    pub fn set_last_mouse(&mut self, position: PhysicalPosition<f64>) {
        self.last_mouse = position;
    }

    fn apply_proxy_prefs(&self, cfg: &ProxyConfig) {
        self.servo.set_preference(
            "network_http_proxy_uri",
            PrefValue::Str(cfg.http_proxy_uri.clone()),
        );
        self.servo.set_preference(
            "network_https_proxy_uri",
            PrefValue::Str(cfg.https_proxy_uri.clone()),
        );
        self.servo.set_preference(
            "network_http_no_proxy",
            PrefValue::Str(cfg.no_proxy.clone()),
        );
        info!(
            http = %cfg.http_proxy_uri,
            https = %cfg.https_proxy_uri,
            "network proxy preferences applied"
        );
    }

    pub fn site_data(&self) -> crate::site_data::SiteDataManager<'_> {
        crate::site_data::SiteDataManager::new(&self.servo)
    }

    /// Resize the parent window GL surface (full window, including chrome).
    pub fn resize_window(&mut self, size: PhysicalSize<u32>) {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        self.window_context.resize(size);
    }

    /// Update HiDPI scale for all webviews (window moved between displays, etc.).
    pub fn set_scale_factor(&mut self, scale: f64) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        if (self.scale_factor - scale).abs() < f64::EPSILON {
            return;
        }
        self.scale_factor = scale;
        let hidpi = Scale::new(scale as f32);
        for state in self.views.values() {
            state.webview.set_hidpi_scale_factor(hidpi);
        }
        self.servo.spin_event_loop();
    }

    /// Bind the window framebuffer before egui paints (servoshell pattern).
    pub fn prepare_parent_for_rendering(&self) {
        let _ = self.window_context.make_current();
        self.window_context.prepare_for_rendering();
    }

    /// Paint the focused WebView into the offscreen page framebuffer (no blit).
    /// Skips work when Servo has not produced a new frame since the last paint.
    pub fn paint_webview(&mut self) -> BrowserResult<()> {
        let dirty = self
            .shared
            .page_dirty
            .lock()
            .map(|d| *d)
            .unwrap_or(false);
        if !dirty {
            return Ok(());
        }
        let Some(view) = self.focused_view().cloned() else {
            return Ok(());
        };
        view.paint();
        let _ = self.shared.take_page_dirty();
        Ok(())
    }

    /// Force the next `paint_webview` to run (resize / tab focus / etc.).
    pub fn mark_page_dirty(&self) {
        self.shared.mark_page_dirty();
    }

    /// Callback that blits the offscreen page into a parent-window GL rect.
    /// Coordinates must use OpenGL bottom-left origin (`from_bottom_px`).
    pub fn render_to_parent_callback(
        &self,
    ) -> Option<Box<dyn Fn(&glow::Context, euclid::default::Rect<i32>) + Send + Sync>> {
        self.page_context.render_to_parent_callback()
    }

    fn focused_view(&self) -> Option<&WebView> {
        self.focused
            .and_then(|id| self.views.get(&id))
            .map(|v| &v.webview)
    }

    /// Give keyboard focus to the focused WebView (call after clicking the page).
    pub fn focus_page_input(&mut self) {
        if let Some(view) = self.focused_view().cloned() {
            view.focus();
            self.servo.spin_event_loop();
        }
    }

    fn device_point(x: f64, y: f64) -> servo::DevicePoint {
        servo::DevicePoint::new(x as f32, y as f32)
    }

    /// Convert a window-physical mouse position into WebView device coordinates.
    fn webview_point(&self, position: PhysicalPosition<f64>) -> Option<servo::DevicePoint> {
        let x = position.x - self.content_origin_x;
        let y = position.y - self.content_origin_y;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let w = self.page_size.width as f64;
        let h = self.page_size.height as f64;
        if x > w || y > h {
            return None;
        }
        Some(Self::device_point(x, y))
    }
}

struct Delegate {
    shared: Rc<SharedState>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        if let Some(view) = self.shared.view_id_for(&webview) {
            self.shared.push(EngineEvent::NewFrameReady { view });
        }
        if let Ok(guard) = self.shared.request_redraw.lock() {
            if let Some(cb) = guard.as_ref() {
                cb();
            }
        }
    }

    fn notify_url_changed(&self, webview: WebView, url: Url) {
        if let Some(view) = self.shared.view_id_for(&webview) {
            info!(%url, "navigation");
            self.shared
                .push(EngineEvent::UrlChanged { view, url });
        }
    }

    fn notify_page_title_changed(&self, webview: WebView, title: Option<String>) {
        if let Some(view) = self.shared.view_id_for(&webview) {
            self.shared
                .push(EngineEvent::TitleChanged { view, title });
        }
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if let Some(view) = self.shared.view_id_for(&webview) {
            let loading = !matches!(status, LoadStatus::Complete);
            self.shared
                .push(EngineEvent::LoadStatusChanged { view, loading });
        }
    }

    fn notify_history_changed(&self, webview: WebView, _entries: Vec<Url>, _current: usize) {
        if let Some(view) = self.shared.view_id_for(&webview) {
            self.shared.push(EngineEvent::HistoryChanged {
                view,
                can_go_back: webview.can_go_back(),
                can_go_forward: webview.can_go_forward(),
            });
        }
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        error!(%reason, "engine crash");
        if let Some(view) = self.shared.view_id_for(&webview) {
            self.shared
                .push(EngineEvent::Crashed { view, reason });
        }
    }

    fn request_permission(&self, webview: WebView, request: PermissionRequest) {
        let feature = request.feature();
        let feature_name = permission_feature_name(feature).to_string();
        let origin = webview
            .url()
            .map(|u| {
                let scheme = u.scheme();
                let host = u.host_str().unwrap_or("");
                if host.is_empty() {
                    u.as_str().to_string()
                } else {
                    format!("{scheme}://{host}")
                }
            })
            .unwrap_or_else(|| "null".into());
        let view = self.shared.view_id_for(&webview);

        let allowed = self
            .shared
            .permission_allowed
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|f| f(origin.clone(), feature_name.clone())))
            .unwrap_or(false);

        if allowed {
            info!(%origin, %feature_name, "permission allow (stored decision)");
            request.allow();
            return;
        }

        warn!(%origin, %feature_name, ?view, "permission deny (Ask/Deny — no auto-allow)");
        if let Some(view) = view {
            self.shared.push(EngineEvent::PermissionRequested {
                view,
                feature: feature_name,
            });
        }
        request.deny();
    }

    fn notify_cursor_changed(&self, _webview: WebView, cursor: servo::Cursor) {
        self.shared.push(EngineEvent::CursorChanged { cursor });
        if let Ok(guard) = self.shared.request_redraw.lock() {
            if let Some(cb) = guard.as_ref() {
                cb();
            }
        }
    }
}

impl BrowserEngine for ServoEngine {
    fn create_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()> {
        if self.views.contains_key(&id) {
            return Ok(());
        }

        let delegate = Rc::new(Delegate {
            shared: self.shared.clone(),
        });

        let mut builder =
            WebViewBuilder::new(&self.servo, self.page_context.clone() as Rc<dyn RenderingContext>)
                .hidpi_scale_factor(Scale::new(self.scale_factor as f32))
                .delegate(delegate);

        if let Some(url) = url.clone() {
            builder = builder.url(url);
        }

        let webview = builder.build();
        self.shared
            .view_ids
            .borrow_mut()
            .insert(webview.id(), id);

        // Size to current page area.
        webview.resize(self.page_size);
        webview.set_page_zoom(self.page_zoom);
        webview.show();
        webview.focus();

        info!(tab = %id.0, "engine view created");
        self.views.insert(id, ViewState { webview });
        self.focused = Some(id);
        self.shared.mark_page_dirty();
        self.servo.spin_event_loop();
        Ok(())
    }

    fn destroy_view(&mut self, id: EngineViewId) -> BrowserResult<()> {
        if let Some(state) = self.views.remove(&id) {
            self.shared.view_ids.borrow_mut().remove(&state.webview.id());
            // Dropping WebView closes it — must not retain stale references.
            info!(tab = %id.0, "engine view destroyed");
        }
        if self.focused == Some(id) {
            self.focused = self.views.keys().next().copied();
            if let Some(fid) = self.focused {
                if let Some(v) = self.views.get(&fid) {
                    v.webview.show();
                }
            }
            self.shared.mark_page_dirty();
        }
        self.servo.spin_event_loop();
        Ok(())
    }

    fn recreate_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()> {
        self.destroy_view(id)?;
        self.create_view(id, url)
    }

    fn focus_view(&mut self, id: EngineViewId) -> BrowserResult<()> {
        if !self.views.contains_key(&id) {
            return Err(BrowserError::engine(format!("unknown view {id:?}")));
        }
        for (vid, state) in &self.views {
            if *vid == id {
                state.webview.show();
            } else {
                state.webview.hide();
            }
        }
        self.focused = Some(id);
        if let Some(state) = self.views.get(&id) {
            // Servo ignores keyboard until the WebView is focused.
            state.webview.focus();
        }
        self.shared.mark_page_dirty();
        self.servo.spin_event_loop();
        Ok(())
    }

    fn navigate(&mut self, id: EngineViewId, url: Url) -> BrowserResult<()> {
        let view = self
            .views
            .get(&id)
            .ok_or_else(|| BrowserError::engine(format!("unknown view {id:?}")))?;
        info!(tab = %id.0, %url, "navigation");
        view.webview.load(url);
        self.servo.spin_event_loop();
        Ok(())
    }

    fn reload(&mut self, id: EngineViewId) -> BrowserResult<()> {
        let view = self
            .views
            .get(&id)
            .ok_or_else(|| BrowserError::engine(format!("unknown view {id:?}")))?;
        view.webview.reload();
        self.servo.spin_event_loop();
        Ok(())
    }

    fn go_back(&mut self, id: EngineViewId) -> BrowserResult<()> {
        let view = self
            .views
            .get(&id)
            .ok_or_else(|| BrowserError::engine(format!("unknown view {id:?}")))?;
        if view.webview.can_go_back() {
            let _ = view.webview.go_back(1);
            self.servo.spin_event_loop();
        }
        Ok(())
    }

    fn go_forward(&mut self, id: EngineViewId) -> BrowserResult<()> {
        let view = self
            .views
            .get(&id)
            .ok_or_else(|| BrowserError::engine(format!("unknown view {id:?}")))?;
        if view.webview.can_go_forward() {
            let _ = view.webview.go_forward(1);
            self.servo.spin_event_loop();
        }
        Ok(())
    }

    fn can_go_back(&self, id: EngineViewId) -> bool {
        self.views
            .get(&id)
            .map(|v| v.webview.can_go_back())
            .unwrap_or(false)
    }

    fn can_go_forward(&self, id: EngineViewId) -> bool {
        self.views
            .get(&id)
            .map(|v| v.webview.can_go_forward())
            .unwrap_or(false)
    }

    fn apply_route(&mut self, route: &Route) -> BrowserResult<()> {
        match route {
            Route::Direct => {
                self.apply_proxy_prefs(&ProxyConfig {
                    http_proxy_uri: String::new(),
                    https_proxy_uri: String::new(),
                    no_proxy: String::new(),
                });
            }
            Route::Proxy(cfg) => {
                self.apply_proxy_prefs(cfg);
            }
            Route::Vpn(_) => {
                return Err(BrowserError::engine(
                    "VPN route reached engine without a tunnel backend".to_string(),
                ));
            }
            Route::Unavailable { reason } | Route::Block { reason } => {
                return Err(BrowserError::engine(reason.clone()));
            }
        }
        Ok(())
    }

    fn resize(&mut self, size: PhysicalSize<u32>) -> BrowserResult<()> {
        // WebRender panics above MAX_RENDER_TASK_SIZE (16384).
        const MAX: u32 = 8192;
        let new_size = PhysicalSize::new(
            size.width.clamp(1, MAX),
            size.height.clamp(1, MAX),
        );
        if self.page_size == new_size {
            return Ok(());
        }
        self.page_size = new_size;

        // Only resize through WebView so Servo updates both the offscreen
        // RenderingContext and the WebRender document view together.
        // Calling `page_context.resize` first makes `webview.resize` early-out
        // and skips the document-view update.
        if self.views.is_empty() {
            self.page_context.resize(self.page_size);
        } else {
            for state in self.views.values() {
                state.webview.resize(self.page_size);
            }
        }
        self.shared.mark_page_dirty();
        self.servo.spin_event_loop();
        Ok(())
    }

    fn paint(&mut self) -> BrowserResult<()> {
        // Paint into the offscreen page buffer only. Blitting into the window
        // happens via egui PaintCallback (see browser-ui), matching servoshell.
        self.paint_webview()
    }

    fn present(&mut self) -> BrowserResult<()> {
        self.window_context.present();
        Ok(())
    }

    fn spin(&mut self) {
        self.servo.spin_event_loop();
    }

    fn handle_window_event(
        &mut self,
        event: &WindowEvent,
        content_origin_y: f64,
        _scale_factor: f64,
    ) {
        // Keep Y in sync when callers still pass it; X comes from set_content_rect.
        self.content_origin_y = content_origin_y;

        if let WindowEvent::ModifiersChanged(mods) = event {
            self.modifiers = mods.state();
            return;
        }

        let Some(webview) = self.focused_view().cloned() else {
            return;
        };

        match event {
            WindowEvent::KeyboardInput { event: key_event, .. } => {
                let keyboard = keyboard_event_from_winit(key_event, self.modifiers);
                webview.notify_input_event(InputEvent::Keyboard(keyboard));
                self.servo.spin_event_loop();
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    Ime::Enabled => {
                        webview.notify_input_event(InputEvent::Ime(ImeEvent::Composition(
                            CompositionEvent {
                                state: CompositionState::Start,
                                data: String::new(),
                            },
                        )));
                    }
                    Ime::Preedit(text, _) => {
                        webview.notify_input_event(InputEvent::Ime(ImeEvent::Composition(
                            CompositionEvent {
                                state: CompositionState::Update,
                                data: text.clone(),
                            },
                        )));
                    }
                    Ime::Commit(text) => {
                        webview.notify_input_event(InputEvent::Ime(ImeEvent::Composition(
                            CompositionEvent {
                                state: CompositionState::End,
                                data: text.clone(),
                            },
                        )));
                    }
                    Ime::Disabled => {
                        webview.notify_input_event(InputEvent::Ime(ImeEvent::Dismissed));
                    }
                }
                self.servo.spin_event_loop();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_mouse = *position;
                let Some(point) = self.webview_point(*position) else {
                    webview.notify_input_event(InputEvent::MouseLeftViewport(
                        servo::MouseLeftViewportEvent::default(),
                    ));
                    return;
                };
                webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point.into())));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(point) = self.webview_point(self.last_mouse) else {
                    return;
                };
                let servo_button = match button {
                    WinitMouseButton::Left => MouseButton::Left,
                    WinitMouseButton::Right => MouseButton::Right,
                    WinitMouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                let action = match state {
                    ElementState::Pressed => MouseButtonAction::Down,
                    ElementState::Released => MouseButtonAction::Up,
                };
                if matches!(action, MouseButtonAction::Down) {
                    webview.focus();
                }
                webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                    action,
                    servo_button,
                    point.into(),
                )));
                self.servo.spin_event_loop();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(point) = self.webview_point(self.last_mouse) else {
                    return;
                };
                let (dx, dy, mode) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        ((*x as f64) * 76.0, (*y as f64) * 76.0, WheelMode::DeltaLine)
                    }
                    MouseScrollDelta::PixelDelta(p) => (p.x, p.y, WheelMode::DeltaPixel),
                };
                webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta {
                        x: dx,
                        y: dy,
                        z: 0.0,
                        mode,
                    },
                    point.into(),
                )));
                self.servo.spin_event_loop();
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    self.resize_window(*new_size);
                }
            }
            WindowEvent::ScaleFactorChanged {
                scale_factor: new_scale,
                ..
            } => {
                self.set_scale_factor(*new_scale);
            }
            _ => {}
        }
    }

    fn poll_events(&mut self) -> Vec<EngineEvent> {
        self.shared
            .events
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}

fn permission_feature_name(feature: PermissionFeature) -> &'static str {
    match feature {
        PermissionFeature::Camera => "camera",
        PermissionFeature::Microphone => "microphone",
        PermissionFeature::Geolocation => "location",
        PermissionFeature::Notifications => "notifications",
        PermissionFeature::Push => "notifications",
        _ => "other",
    }
}
