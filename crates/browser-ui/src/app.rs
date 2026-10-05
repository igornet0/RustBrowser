use browser_core::{Watchdog, WatchdogConfig};
use crate::branding;
use crate::chrome::draw_chrome;
use crate::controller::{is_chrome_ui_url, BrowserController};
use crate::page_backend::PageBackend;
use crate::splash::SplashScreen;
use crate::waker::{Waker, WakerEvent};
use browser_engine::{EngineEvent, ServoEngineConfig};
use browser_profile::{ProfileManager, WindowState};
use egui_glow::EguiGlow;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};
use std::time::{Duration, Instant};

struct Running {
    window: Arc<Window>,
    engine: PageBackend,
    egui: EguiGlow,
    controller: BrowserController,
    modifiers: ModifiersState,
    page_left_px: f64,
    toolbar_height_px: f64,
    page_right_px: f64,
    page_bottom_px: f64,
    last_pointer_px: Option<PhysicalPosition<f64>>,
    page_input_active: bool,
    waker: Waker,
    splash: SplashScreen,
    next_repaint_at: Option<Instant>,
    watchdog: Watchdog,
    last_checkpoint: Instant,
}

enum App {
    Boot {
        waker: Waker,
        controller: Option<BrowserController>,
    },
    Running(Box<Running>),
}

/// Launch the desktop browser UI.
pub fn run() -> browser_core::BrowserResult<()> {
    let oop = browser_core::content_isolation_enabled();
    info!(content_isolation = oop, "browser startup");

    let profiles_root = ProfileManager::default_root();
    let controller = BrowserController::bootstrap(profiles_root)?;

    let event_loop = EventLoop::with_user_event()
        .build()
        .map_err(|e| browser_core::BrowserError::ui(e.to_string()))?;
    let waker = Waker::new(&event_loop);
    let mut app = App::Boot {
        waker,
        controller: Some(controller),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| browser_core::BrowserError::ui(e.to_string()))?;
    Ok(())
}

impl ApplicationHandler<WakerEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let App::Boot { waker, controller } = self else {
            return;
        };
        let Some(mut controller) = controller.take() else {
            return;
        };

        let mut attrs = Window::default_attributes()
            .with_title("Rust Browser")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        if let Some(icon) = branding::app_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                error!(?err, "ui error creating window");
                event_loop.exit();
                return;
            }
        };
        // Needed so Servo can receive composition / password-manager IME input.
        window.set_ime_allowed(true);

        let display_handle = match event_loop.display_handle() {
            Ok(h) => h,
            Err(err) => {
                error!(?err, "display handle");
                event_loop.exit();
                return;
            }
        };
        let window_handle = match window.window_handle() {
            Ok(h) => h,
            Err(err) => {
                error!(?err, "window handle");
                event_loop.exit();
                return;
            }
        };

        let scale = window.scale_factor();
        let size = window.inner_size();

        let mut engine = if browser_core::content_isolation_enabled() {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rust-browser"));
            let content_dir = controller.store.paths.root.join("content");
            // Unix domain sockets have a ~104-byte path limit on macOS; profile
            // paths under Application Support are too long — use /tmp instead.
            let socket_dir = short_ipc_socket_dir();
            let host_file = host_file_for_engine(&controller);
            match PageBackend::start_isolated(
                display_handle,
                window_handle,
                size,
                content_dir,
                socket_dir,
                exe,
                host_file,
            ) {
                Ok(backend) => backend,
                Err(err) => {
                    error!(
                        ?err,
                        "isolated content failed — refusing in-process Servo fallback (set RUST_BROWSER_CONTENT=inprocess for legacy/debug only)"
                    );
                    event_loop.exit();
                    return;
                }
            }
        } else {
            match PageBackend::start_legacy(ServoEngineConfig {
                display_handle,
                window_handle,
                window_size: size,
                scale_factor: scale,
                event_loop_waker: Box::new(waker.clone()),
                config_dir: Some(controller.store.paths.storage.clone()),
                host_file: host_file_for_engine(&controller),
                locale: Some(controller.store.settings.language.locale_tag().to_string()),
            }) {
                Ok(b) => b,
                Err(err) => {
                    error!(?err, "engine errors");
                    event_loop.exit();
                    return;
                }
            }
        };

        let gl = engine.glow_context();
        let egui = EguiGlow::new(event_loop, gl, None, Some(scale as f32), false);

        let win = window.clone();
        engine.set_redraw_callback(move || win.request_redraw());

        // Permission checker only applies to legacy in-process Servo.
        if let PageBackend::Legacy(ref engine) = engine {
            use browser_profile::{PermissionDecision, PermissionKind};
            let perms_path = controller.store.paths.permissions_db.clone();
            engine.set_permission_checker(move |origin, feature| {
                let kind = match feature.as_str() {
                    "camera" => PermissionKind::Camera,
                    "microphone" => PermissionKind::Microphone,
                    "location" => PermissionKind::Location,
                    "notifications" => PermissionKind::Notifications,
                    "clipboard" => PermissionKind::Clipboard,
                    "screen_capture" => PermissionKind::ScreenCapture,
                    _ => PermissionKind::Other,
                };
                browser_profile::PermissionManager::open(&perms_path)
                    .ok()
                    .and_then(|m| m.decision(&origin, kind).ok())
                    .map(|d| d == PermissionDecision::Allow)
                    .unwrap_or(false)
            });
        }

        controller.sync_engine_views(&mut engine);

        let toolbar_height_px = 96.0 * scale;
        engine.set_content_rect(0.0, toolbar_height_px);
        let _ = engine.set_scale_factor(scale);

        *self = App::Running(Box::new(Running {
            window,
            engine,
            egui,
            controller,
            modifiers: ModifiersState::default(),
            page_left_px: 0.0,
            toolbar_height_px,
            page_right_px: size.width as f64,
            page_bottom_px: size.height as f64,
            last_pointer_px: None,
            page_input_active: true,
            waker: waker.clone(),
            splash: SplashScreen::new(),
            next_repaint_at: None,
            watchdog: Watchdog::start(WatchdogConfig::default()),
            last_checkpoint: Instant::now(),
        }));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let App::Running(state) = self {
            shutdown_running(state);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let App::Running(state) = self else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        state.watchdog.beat();
        state.controller.tick_recoveries(&mut state.engine);
        // Isolated path: poll IPC even when the Servo waker is absent.
        if state.engine.is_isolated() {
            let events = state.engine.poll_events();
            if !events.is_empty() {
                apply_events(state, events);
                state.window.request_redraw();
            }
        }
        if state.engine.take_needs_restore() {
            for tab in state.controller.browser.tabs.clone() {
                let url = tab.url.clone().filter(|u| !is_chrome_ui_url(Some(u)));
                let _ = state
                    .engine
                    .create_view(browser_engine::EngineViewId(tab.id), url.clone());
                if let Some(url) = url {
                    let _ = state
                        .engine
                        .navigate(browser_engine::EngineViewId(tab.id), url);
                }
            }
            state.controller.push_status("Content process restored tabs");
        }
        if state.last_checkpoint.elapsed() >= Duration::from_secs(5) {
            let size = state.window.inner_size();
            let pos = state.window.outer_position().unwrap_or_default();
            state.controller.last_window = WindowState {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
                maximized: false,
            };
            state.controller.schedule_session_checkpoint();
            state.controller.flush_session_checkpoint();
            state.last_checkpoint = Instant::now();
        }
        if let Some(at) = state.next_repaint_at {
            if Instant::now() >= at {
                state.next_repaint_at = None;
                state.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Poll);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(at));
            }
        } else if !state.controller.pending_tab_recoveries.is_empty() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
        } else if state.engine.is_isolated() {
            // Keep polling Content IPC / frames (~60 Hz).
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(16),
            ));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: WakerEvent) {
        if let App::Running(state) = self {
            state.watchdog.beat();
            state.engine.spin();
            let events = state.engine.poll_events();
            let needs_chrome = events.iter().any(|e| {
                !matches!(e, EngineEvent::NewFrameReady { .. })
            });
            let needs_frame = events
                .iter()
                .any(|e| matches!(e, EngineEvent::NewFrameReady { .. }));
            apply_events(state, events);
            // NewFrameReady already schedules redraw via engine callback; still
            // redraw for chrome-only events (title/url/loading).
            if needs_chrome || needs_frame {
                state.window.request_redraw();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let App::Running(state) = self else {
            return;
        };

        // Avoid spinning Servo on every CursorMoved — coalesce into redraw.
        let should_spin_now = matches!(
            event,
            WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::KeyboardInput { .. }
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
        );
        if should_spin_now {
            state.engine.spin();
        }

        if let WindowEvent::ModifiersChanged(mods) = &event {
            state.modifiers = mods.state();
        }

        // Chrome shortcuts (⌘T/W/L/…) are handled in draw_chrome via egui input.
        // Don't send those key combos to the page — except common editing shortcuts.
        let mut chrome_consumed_key = false;
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
        {
            if key_event.state.is_pressed() {
                let cmd = state.modifiers.super_key() || state.modifiers.control_key();
                if cmd && !is_page_editing_shortcut(key_event) {
                    chrome_consumed_key = true;
                }
                // Cmd+L focuses the address bar — page loses keyboard.
                if cmd {
                    if let Key::Character(c) = &key_event.logical_key {
                        if c.eq_ignore_ascii_case("l") {
                            state.page_input_active = false;
                        }
                    }
                }
            }
        }

        if let WindowEvent::CursorMoved { position, .. } = &event {
            state.last_pointer_px = Some(*position);
        }

        // When focus_address was requested (chrome shortcut), page loses keyboard.
        if state.controller.focus_address {
            state.page_input_active = false;
        }

        let over_page = pointer_in_page_viewport(state, &event);
        let page_receives_pointer = over_page && !chrome_ui_blocks_page(state) && !is_active_chrome_ui(state);

        // Servoshell pattern: when interacting with the page, don't let egui keep
        // text-field focus. Give events to egui first for chrome, then to Servo.
        let egui_event_response = state.egui.on_window_event(state.window.as_ref(), &event);

        if page_receives_pointer {
            if let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } = &event
            {
                state.page_input_active = true;
                state.egui.egui_ctx.memory_mut(|mem| {
                    if let Some(id) = mem.focused() {
                        mem.surrender_focus(id);
                    }
                });
                state.controller.show_vpn_popover = false;
                state.controller.show_profile_popover = false;
                state.controller.show_main_menu = false;
                state.engine.focus_page_input();
            }
            if let Some(pos) = state.last_pointer_px {
                state.engine.set_last_mouse(pos);
            }
            state.engine.handle_window_event(
                &event,
                state.toolbar_height_px,
                state.window.scale_factor(),
            );
        } else {
            if matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    ..
                }
            ) {
                // Clicked chrome / overlay — keyboard goes to egui.
                state.page_input_active = false;
            }
            if matches!(event, WindowEvent::CursorMoved { .. }) {
                state.window.set_cursor(winit::window::CursorIcon::Default);
            }
        }

        let forward_keyboard = !chrome_consumed_key
            && state.page_input_active
            && !chrome_ui_blocks_page(state)
            && !is_active_chrome_ui(state)
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)
            );

        if forward_keyboard || matches!(event, WindowEvent::ModifiersChanged(_)) {
            state.engine.handle_window_event(
                &event,
                state.toolbar_height_px,
                state.window.scale_factor(),
            );
        }

        match event {
            WindowEvent::CloseRequested => {
                let size = state.window.inner_size();
                let pos = state.window.outer_position().unwrap_or_default();
                state.controller.clean_shutdown(WindowState {
                    x: pos.x,
                    y: pos.y,
                    width: size.width,
                    height: size.height,
                    maximized: false,
                });
                shutdown_running(state);
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                redraw(state);
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    state.engine.resize_window(new_size);
                    state.page_bottom_px = new_size.height as f64;
                }
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged {
                scale_factor, ..
            } => {
                if scale_factor.is_finite() && scale_factor > 0.0 {
                    state.engine.set_scale_factor(scale_factor);
                    state
                        .egui
                        .egui_ctx
                        .set_pixels_per_point(scale_factor.clamp(0.5, 4.0) as f32);
                }
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
                if page_receives_pointer =>
            {
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => {
                state.window.request_redraw();
            }
            _ => {
                if egui_event_response.repaint {
                    state.window.request_redraw();
                }
            }
        }
    }
}

/// Short AF_UNIX socket directory. Profile cache paths under
/// `~/Library/Application Support/...` exceed macOS `sun_path` (~104 bytes).
fn short_ipc_socket_dir() -> PathBuf {
    PathBuf::from("/tmp").join(format!("rb-ipc-{}", std::process::id()))
}

/// Path to the profile hosts file when it exists (Servo domain → IP overrides).
fn host_file_for_engine(controller: &BrowserController) -> Option<PathBuf> {
    let path = controller.store.paths.local_hosts.clone();
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn shutdown_running(state: &mut Running) {
    state.waker.retire();
    state.engine.clear_redraw_callback();
    state.egui.destroy();
    state.engine.spin();
}

impl Drop for Running {
    fn drop(&mut self) {
        self.waker.retire();
        self.engine.clear_redraw_callback();
        self.egui.destroy();
    }
}

fn apply_events(state: &mut Running, events: Vec<EngineEvent>) {
    for event in &events {
        if matches!(
            event,
            EngineEvent::LoadStatusChanged { loading: false, .. }
                | EngineEvent::NewFrameReady { .. }
        ) {
            state.splash.notify_page_ready();
        }
        if let EngineEvent::CursorChanged { cursor } = event {
            state.window.set_cursor(servo_cursor_icon(*cursor));
        }
    }
    state.controller.apply_engine_events(events);
}

fn servo_cursor_icon(cursor: servo::Cursor) -> winit::window::CursorIcon {
    use servo::Cursor;
    use winit::window::CursorIcon;
    match cursor {
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Move => CursorIcon::Move,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
        _ => CursorIcon::Default,
    }
}

fn sync_page_layout(
    state: &mut Running,
    content_left_points: f32,
    content_top_points: f32,
    content_width_points: f32,
    content_height_points: f32,
    pixels_per_point: f32,
) {
    // Prefer the PPP egui used for this frame's content rect (same as PaintCallback).
    // Fall back to the window scale if egui reports something unusable.
    let window_scale = state.window.scale_factor();
    let ppp = {
        let from_egui = f64::from(pixels_per_point);
        if from_egui.is_finite() && from_egui > 0.0 {
            from_egui.clamp(0.5, 4.0)
        } else {
            window_scale
        }
    };

    // Servo hidpi must match the scale used for page size / hit-test conversion.
    if (state.engine.scale_factor() - ppp).abs() > f64::EPSILON {
        state.engine.set_scale_factor(ppp);
    }
    if (f64::from(state.egui.egui_ctx.pixels_per_point()) - ppp).abs() > f64::EPSILON {
        state.egui.egui_ctx.set_pixels_per_point(ppp as f32);
    }

    let left_px = (f64::from(content_left_points) * ppp).round();
    let top_px = (f64::from(content_top_points) * ppp).round();
    let width_px = (f64::from(content_width_points) * ppp).round().max(1.0);
    let height_px = (f64::from(content_height_points) * ppp).round().max(1.0);

    state.page_left_px = left_px;
    state.toolbar_height_px = top_px;
    state.page_right_px = left_px + width_px;
    state.page_bottom_px = top_px + height_px;
    state.engine.set_content_rect(left_px, top_px);

    let page_size = winit::dpi::PhysicalSize::new(width_px as u32, height_px as u32);
    if state.engine.page_size() != page_size {
        let _ = state.engine.resize(page_size);
    }
}

fn pointer_in_page_viewport(state: &Running, event: &WindowEvent) -> bool {
    let pos = match event {
        WindowEvent::CursorMoved { position, .. } => *position,
        WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => state
            .last_pointer_px
            .unwrap_or_else(|| state.engine.last_mouse()),
        _ => return false,
    };
    pos.x >= state.page_left_px
        && pos.x < state.page_right_px
        && pos.y >= state.toolbar_height_px
        && pos.y < state.page_bottom_px
}

fn chrome_ui_blocks_page(state: &Running) -> bool {
    state.splash.is_visible()
        || state.controller.show_command_palette
        || state.controller.show_import_modal
        || state.controller.show_restore_prompt
        || state.controller.show_vpn_popover
        || state.controller.show_profile_popover
        || state.controller.show_main_menu
        || state.controller.show_history
        || state.controller.show_bookmarks
        || state.controller.show_downloads
}

fn is_active_chrome_ui(state: &Running) -> bool {
    if state.controller.settings_tab_active() {
        return true;
    }
    let active_url = state
        .controller
        .browser
        .active()
        .ok()
        .and_then(|t| t.url.as_ref());
    is_chrome_ui_url(active_url)
}

fn is_page_editing_shortcut(key_event: &winit::event::KeyEvent) -> bool {
    matches!(
        &key_event.logical_key,
        Key::Character(c)
            if matches!(c.to_lowercase().as_str(), "a" | "c" | "v" | "x" | "z" | "y")
    )
}

fn redraw(state: &mut Running) {
    // Process deferred Servo work (e.g. coalesced mouse moves) once per frame.
    state.watchdog.beat();
    state.engine.spin();
    let events = state.engine.poll_events();
    apply_events(state, events);

    let mut content_left_points = 0.0_f32;
    let mut content_top_points = (state.toolbar_height_px / state.window.scale_factor()) as f32;
    let mut content_width_points = 800.0_f32;
    let mut content_height_points = 400.0_f32;
    let mut pixels_per_point = state.window.scale_factor() as f32;
    let mut request_repaint = false;
    let window = state.window.clone();

    let raw_input = state.egui.egui_winit.take_egui_input(window.as_ref());
    let full_output = state.egui.egui_ctx.run_ui(raw_input, |ui| {
        let out = draw_chrome(ui, &mut state.controller, &mut state.engine);
        content_left_points = out.content_left_points;
        content_top_points = out.content_top_points;
        content_width_points = out.content_width_points;
        content_height_points = out.content_height_points;
        pixels_per_point = out.pixels_per_point;
        request_repaint = out.request_repaint;
        let _ = state.splash.draw(ui.ctx());
    });

    sync_page_layout(
        state,
        content_left_points,
        content_top_points,
        content_width_points,
        content_height_points,
        pixels_per_point,
    );

    state.egui.egui_winit.handle_platform_output(window.as_ref(), full_output.platform_output);

    for (id, image_delta) in full_output.textures_delta.set {
        state.egui.painter.set_texture(id, &image_delta);
    }

    // Bind the window framebuffer once before chrome + page blit.
    state.engine.prepare_parent_for_rendering();

    // PaintCallback registered in draw_chrome blits the Servo page during this pass.
    let clipped = state
        .egui
        .egui_ctx
        .tessellate(full_output.shapes, full_output.pixels_per_point);
    let dimensions: [u32; 2] = window.inner_size().into();
    state
        .egui
        .painter
        .paint_primitives(dimensions, full_output.pixels_per_point, &clipped);

    for id in full_output.textures_delta.free {
        state.egui.painter.free_texture(id);
    }

    if let Err(err) = state.engine.present() {
        warn!(?err, "present");
    }

    let egui_wants_repaint = full_output
        .viewport_output
        .values()
        .any(|v| v.repaint_delay.is_zero());
    let min_delay = full_output
        .viewport_output
        .values()
        .map(|v| v.repaint_delay)
        .min()
        .unwrap_or(Duration::MAX);

    if request_repaint || egui_wants_repaint {
        state.next_repaint_at = None;
        state.window.request_redraw();
    } else if min_delay < Duration::from_secs(30) {
        state.next_repaint_at = Some(Instant::now() + min_delay);
    } else {
        state.next_repaint_at = None;
    }
}
