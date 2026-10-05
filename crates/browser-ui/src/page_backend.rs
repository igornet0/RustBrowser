//! Production page backend: Content Process via IPC (default) or legacy in-process Servo.
//!
//! After P1.1 the default path never constructs `Servo` / `WebView` in the Browser process.

use crate::remote_content::RemoteContentSession;
use browser_core::{BrowserError, BrowserResult, TabId};
use browser_engine::{
    BrowserEngine, EngineEvent, EngineViewId, ServoEngine, ServoEngineConfig, UiSurface,
};
use browser_ipc::{clamp_ipc_frame_size, ContentToBrowser, FrameBuffer, InputEventMsg};
use browser_core::Route;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use url::Url;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key as WinitKey, ModifiersState};
use winit::raw_window_handle::{DisplayHandle, WindowHandle};

pub enum PageBackend {
    /// P1.1 production: UiSurface for egui GL + remote Servo in Content Process.
    Isolated {
        surface: UiSurface,
        remote: RemoteContentSession,
        frames: HashMap<TabId, FrameBuffer>,
        nav: HashMap<TabId, (bool, bool)>,
        focused: Option<TabId>,
        content_origin_x: f64,
        content_origin_y: f64,
        /// Full content viewport in window pixels (hit-testing / layout sync).
        requested_size: PhysicalSize<u32>,
        /// Servo/IPC framebuffer (may be capped under [`clamp_ipc_frame_size`]).
        page_size: PhysicalSize<u32>,
        scale: f64,
        last_mouse: PhysicalPosition<f64>,
        modifiers: ModifiersState,
        events: Vec<EngineEvent>,
        needs_tab_restore: bool,
    },
    /// Legacy/debug only (`RUST_BROWSER_CONTENT=inprocess`).
    Legacy(ServoEngine),
}

impl PageBackend {
    pub fn start_isolated(
        display_handle: DisplayHandle<'_>,
        window_handle: WindowHandle<'_>,
        window_size: PhysicalSize<u32>,
        content_dir: PathBuf,
        socket_dir: PathBuf,
        exe: PathBuf,
        host_file: Option<PathBuf>,
    ) -> BrowserResult<Self> {
        let surface = UiSurface::new(display_handle, window_handle, window_size)?;
        let remote =
            RemoteContentSession::start_with_host_file(content_dir, socket_dir, exe, host_file)?;
        info!(
            pid = ?remote.content_pid(),
            "P1.1 isolated content process attached — Browser owns no Servo"
        );
        let toolbar = (96.0_f64).max(1.0);
        let requested = PhysicalSize::new(
            window_size.width.max(1),
            window_size.height.saturating_sub(toolbar as u32).max(1),
        );
        let (cw, ch) = clamp_ipc_frame_size(requested.width, requested.height);
        Ok(Self::Isolated {
            surface,
            remote,
            frames: HashMap::new(),
            nav: HashMap::new(),
            focused: None,
            content_origin_x: 0.0,
            content_origin_y: toolbar,
            requested_size: requested,
            page_size: PhysicalSize::new(cw, ch),
            scale: 1.0,
            last_mouse: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::default(),
            events: Vec::new(),
            needs_tab_restore: false,
        })
    }

    pub fn start_legacy(config: ServoEngineConfig<'_>) -> BrowserResult<Self> {
        warn!(
            "WARNING: RUST_BROWSER_CONTENT=inprocess is legacy/debug mode. It disables P1 process isolation."
        );
        Ok(Self::Legacy(ServoEngine::new(config)?))
    }

    pub fn is_isolated(&self) -> bool {
        matches!(self, Self::Isolated { .. })
    }

    pub fn glow_context(&self) -> Arc<glow::Context> {
        match self {
            Self::Isolated { surface, .. } => surface.glow_context(),
            Self::Legacy(engine) => engine.glow_context(),
        }
    }

    pub fn content_pid(&self) -> Option<u32> {
        match self {
            Self::Isolated { remote, .. } => remote.content_pid(),
            Self::Legacy(_) => None,
        }
    }

    pub fn set_content_rect(&mut self, x: f64, y: f64) {
        match self {
            Self::Isolated {
                content_origin_x,
                content_origin_y,
                ..
            } => {
                *content_origin_x = x.max(0.0);
                *content_origin_y = y.max(0.0);
            }
            Self::Legacy(engine) => engine.set_content_rect(x, y),
        }
    }

    pub fn set_redraw_callback(&self, cb: impl Fn() + Send + 'static) {
        match self {
            Self::Legacy(engine) => engine.set_redraw_callback(cb),
            Self::Isolated { .. } => {
                // Frames arrive via IPC poll; Browser requests redraw itself.
                drop(cb);
            }
        }
    }

    pub fn clear_redraw_callback(&self) {
        if let Self::Legacy(engine) = self {
            engine.clear_redraw_callback();
        }
    }

    pub fn take_needs_restore(&mut self) -> bool {
        match self {
            Self::Isolated {
                needs_tab_restore, ..
            } => std::mem::take(needs_tab_restore),
            Self::Legacy(_) => false,
        }
    }

    pub fn scale_factor(&self) -> f64 {
        match self {
            Self::Isolated { scale, .. } => *scale,
            Self::Legacy(engine) => engine.scale_factor(),
        }
    }

    pub fn last_mouse(&self) -> PhysicalPosition<f64> {
        match self {
            Self::Isolated { last_mouse, .. } => *last_mouse,
            Self::Legacy(engine) => engine.last_mouse(),
        }
    }

    pub fn set_last_mouse(&mut self, position: PhysicalPosition<f64>) {
        match self {
            Self::Isolated { last_mouse, .. } => *last_mouse = position,
            Self::Legacy(engine) => engine.set_last_mouse(position),
        }
    }

    pub fn set_page_zoom(&mut self, zoom: f32) {
        if let Self::Legacy(engine) = self {
            engine.set_page_zoom(zoom);
        }
        let _ = zoom;
    }

    pub fn resize_window(&mut self, size: PhysicalSize<u32>) {
        match self {
            Self::Isolated { surface, .. } => surface.resize_window(size),
            Self::Legacy(engine) => engine.resize_window(size),
        }
    }

    pub fn set_scale_factor(&mut self, scale: f64) {
        match self {
            Self::Isolated {
                scale: s,
                remote,
                page_size,
                requested_size,
                ..
            } => {
                *s = scale;
                let (w, h) = clamp_ipc_frame_size(requested_size.width, requested_size.height);
                *page_size = PhysicalSize::new(w, h);
                let _ = remote.resize(w, h, scale);
            }
            Self::Legacy(engine) => engine.set_scale_factor(scale),
        }
    }

    pub fn prepare_parent_for_rendering(&self) {
        match self {
            Self::Isolated { surface, .. } => surface.prepare_for_rendering(),
            Self::Legacy(engine) => engine.prepare_parent_for_rendering(),
        }
    }

    pub fn present(&mut self) -> BrowserResult<()> {
        match self {
            Self::Isolated { surface, .. } => {
                surface.present();
                Ok(())
            }
            Self::Legacy(engine) => engine.present(),
        }
    }

    pub fn frame_for(&self, tab: TabId) -> Option<&FrameBuffer> {
        match self {
            Self::Isolated { frames, .. } => frames.get(&tab),
            Self::Legacy(_) => None,
        }
    }

    pub fn page_size(&self) -> PhysicalSize<u32> {
        match self {
            // Layout sync compares against the full viewport, not the capped framebuffer.
            Self::Isolated { requested_size, .. } => *requested_size,
            Self::Legacy(engine) => engine.page_size(),
        }
    }

    /// Draw page pixels into an egui rect (isolated path). Legacy uses GL blit instead.
    pub fn paint_page_egui(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        tab: TabId,
    ) {
        let Self::Isolated { frames, .. } = self else {
            return;
        };
        // Do not request_frame every egui paint — Content pushes dirty frames on its own.
        // Spamming RequestFrame on Retina stalls Content on JSON encode and freezes loads.
        if let Some(frame) = frames.get(&tab) {
            if frame.rgba.len()
                == (frame.width as usize).saturating_mul(frame.height as usize).saturating_mul(4)
            {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                );
                let tex = ui.ctx().load_texture(
                    format!("content-frame-{tab}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                ui.put(
                    rect,
                    egui::Image::new(&tex).fit_to_exact_size(rect.size()),
                );
                return;
            }
        }
        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_rgb(18, 20, 28));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Loading…",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(160, 165, 180),
        );
    }

    pub fn crash_loop_locked(&self) -> bool {
        match self {
            Self::Isolated { remote, .. } => remote.crash_loop_locked,
            Self::Legacy(_) => false,
        }
    }

    pub fn retry_crash_loop(&mut self) -> BrowserResult<()> {
        match self {
            Self::Isolated { remote, needs_tab_restore, .. } => {
                remote.retry_after_crash_loop()?;
                *needs_tab_restore = true;
                Ok(())
            }
            Self::Legacy(_) => Ok(()),
        }
    }

    // —— BrowserEngine-shaped API ——

    pub fn create_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()> {
        match self {
            Self::Isolated {
                remote, focused, ..
            } => {
                remote.create_tab(id.0, url)?;
                *focused = Some(id.0);
                remote.focus_tab(id.0)
            }
            Self::Legacy(engine) => engine.create_view(id, url),
        }
    }

    pub fn destroy_view(&mut self, id: EngineViewId) -> BrowserResult<()> {
        match self {
            Self::Isolated {
                remote,
                frames,
                nav,
                focused,
                ..
            } => {
                frames.remove(&id.0);
                nav.remove(&id.0);
                if *focused == Some(id.0) {
                    *focused = None;
                }
                remote.close_tab(id.0)
            }
            Self::Legacy(engine) => engine.destroy_view(id),
        }
    }

    pub fn recreate_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()> {
        self.destroy_view(id)?;
        self.create_view(id, url)
    }

    pub fn focus_view(&mut self, id: EngineViewId) -> BrowserResult<()> {
        match self {
            Self::Isolated {
                remote, focused, ..
            } => {
                *focused = Some(id.0);
                remote.focus_tab(id.0)
            }
            Self::Legacy(engine) => engine.focus_view(id),
        }
    }

    pub fn navigate(&mut self, id: EngineViewId, url: Url) -> BrowserResult<()> {
        match self {
            Self::Isolated { remote, .. } => remote.navigate(id.0, url),
            Self::Legacy(engine) => engine.navigate(id, url),
        }
    }

    pub fn reload(&mut self, id: EngineViewId) -> BrowserResult<()> {
        match self {
            Self::Isolated { remote, .. } => remote.reload(id.0),
            Self::Legacy(engine) => engine.reload(id),
        }
    }

    pub fn go_back(&mut self, id: EngineViewId) -> BrowserResult<()> {
        match self {
            Self::Isolated { remote, .. } => remote.go_back(id.0),
            Self::Legacy(engine) => engine.go_back(id),
        }
    }

    pub fn go_forward(&mut self, id: EngineViewId) -> BrowserResult<()> {
        match self {
            Self::Isolated { remote, .. } => remote.go_forward(id.0),
            Self::Legacy(engine) => engine.go_forward(id),
        }
    }

    pub fn can_go_back(&self, id: EngineViewId) -> bool {
        match self {
            Self::Isolated { nav, .. } => nav.get(&id.0).map(|(b, _)| *b).unwrap_or(false),
            Self::Legacy(engine) => engine.can_go_back(id),
        }
    }

    pub fn can_go_forward(&self, id: EngineViewId) -> bool {
        match self {
            Self::Isolated { nav, .. } => nav.get(&id.0).map(|(_, f)| *f).unwrap_or(false),
            Self::Legacy(engine) => engine.can_go_forward(id),
        }
    }

    pub fn apply_route(&mut self, route: &Route) -> BrowserResult<()> {
        match self {
            Self::Isolated { .. } => {
                // Networking runs inside Content Process Servo. Proxy prefs need an IPC
                // extension; Direct is the default. Do not silently treat Vpn as Direct.
                match route {
                    Route::Direct => Ok(()),
                    Route::Proxy(_) => {
                        warn!("proxy route not yet applied inside content process");
                        Ok(())
                    }
                    Route::Vpn(_) | Route::Unavailable { .. } | Route::Block { .. } => Err(
                        BrowserError::engine(
                            "route not available for content process".to_string(),
                        ),
                    ),
                }
            }
            Self::Legacy(engine) => engine.apply_route(route),
        }
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) -> BrowserResult<()> {
        match self {
            Self::Isolated {
                remote,
                requested_size,
                page_size,
                scale,
                ..
            } => {
                *requested_size = size;
                let (w, h) = clamp_ipc_frame_size(size.width.max(1), size.height.max(1));
                *page_size = PhysicalSize::new(w, h);
                remote.resize(w, h, *scale)
            }
            Self::Legacy(engine) => engine.resize(size),
        }
    }

    pub fn paint_webview(&mut self) -> BrowserResult<()> {
        match self {
            // Isolated frames are pushed by Content when dirty — no per-paint RequestFrame.
            Self::Isolated { .. } => Ok(()),
            Self::Legacy(engine) => engine.paint_webview(),
        }
    }

    pub fn render_to_parent_callback(
        &self,
    ) -> Option<Box<dyn Fn(&glow::Context, euclid::default::Rect<i32>) + Send + Sync>> {
        match self {
            Self::Isolated { .. } => None, // CPU frames via egui texture
            Self::Legacy(engine) => engine.render_to_parent_callback(),
        }
    }

    pub fn spin(&mut self) {
        if let Self::Legacy(engine) = self {
            engine.spin();
        }
    }

    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        match self {
            Self::Isolated {
                remote,
                frames,
                nav,
                events,
                needs_tab_restore,
                ..
            } => {
                match remote.poll() {
                    Ok(msgs) => {
                        for msg in msgs {
                            match msg {
                                ContentToBrowser::UrlChanged { tab_id, url } => {
                                    events.push(EngineEvent::UrlChanged {
                                        view: EngineViewId(tab_id),
                                        url,
                                    });
                                }
                                ContentToBrowser::TitleChanged { tab_id, title } => {
                                    events.push(EngineEvent::TitleChanged {
                                        view: EngineViewId(tab_id),
                                        title: Some(title),
                                    });
                                }
                                ContentToBrowser::LoadStatusChanged { tab_id, loading } => {
                                    events.push(EngineEvent::LoadStatusChanged {
                                        view: EngineViewId(tab_id),
                                        loading,
                                    });
                                }
                                ContentToBrowser::HistoryChanged {
                                    tab_id,
                                    can_go_back,
                                    can_go_forward,
                                } => {
                                    nav.insert(tab_id, (can_go_back, can_go_forward));
                                    events.push(EngineEvent::HistoryChanged {
                                        view: EngineViewId(tab_id),
                                        can_go_back,
                                        can_go_forward,
                                    });
                                }
                                ContentToBrowser::Frame { tab_id, frame } => {
                                    frames.insert(tab_id, frame);
                                    events.push(EngineEvent::NewFrameReady {
                                        view: EngineViewId(tab_id),
                                    });
                                }
                                ContentToBrowser::TabCrashed { tab_id, reason } => {
                                    events.push(EngineEvent::Crashed {
                                        view: EngineViewId(tab_id),
                                        reason,
                                    });
                                }
                                ContentToBrowser::Error { tab_id, message } => {
                                    if message.contains("restarted")
                                        || message.contains("died")
                                        || message.contains("hang")
                                    {
                                        *needs_tab_restore = true;
                                    }
                                    warn!(%message, "content process message");
                                    // Failed navigate / unknown tab must not leave the spinner spinning.
                                    if let Some(tab_id) = tab_id {
                                        events.push(EngineEvent::LoadStatusChanged {
                                            view: EngineViewId(tab_id),
                                            loading: false,
                                        });
                                    }
                                }
                                ContentToBrowser::NavigationFinished { tab_id, url } => {
                                    events.push(EngineEvent::UrlChanged {
                                        view: EngineViewId(tab_id),
                                        url,
                                    });
                                    events.push(EngineEvent::LoadStatusChanged {
                                        view: EngineViewId(tab_id),
                                        loading: false,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(err) => error!(?err, "remote poll"),
                }
                std::mem::take(events)
            }
            Self::Legacy(engine) => engine.poll_events(),
        }
    }

    pub fn handle_window_event(
        &mut self,
        event: &WindowEvent,
        content_origin_y: f64,
        scale_factor: f64,
    ) {
        match self {
            Self::Legacy(engine) => {
                engine.handle_window_event(event, content_origin_y, scale_factor);
            }
            Self::Isolated {
                remote,
                focused,
                content_origin_x,
                content_origin_y: origin_y,
                last_mouse,
                requested_size,
                page_size,
                modifiers,
                ..
            } => {
                *origin_y = content_origin_y;
                if let WindowEvent::ModifiersChanged(mods) = event {
                    *modifiers = mods.state();
                    return;
                }
                let Some(tab_id) = *focused else {
                    return;
                };
                let hit_w = requested_size.width.max(1) as f64;
                let hit_h = requested_size.height.max(1) as f64;
                let page_w = page_size.width.max(1) as f64;
                let page_h = page_size.height.max(1) as f64;
                let in_page = |pos: PhysicalPosition<f64>| -> Option<(f64, f64)> {
                    let x = pos.x - *content_origin_x;
                    let y = pos.y - *origin_y;
                    if x < 0.0 || y < 0.0 || x > hit_w || y > hit_h {
                        return None;
                    }
                    // Map viewport coords → capped Servo framebuffer.
                    Some((x * page_w / hit_w, y * page_h / hit_h))
                };
                let mods_byte = {
                    let mut m = 0u8;
                    if modifiers.shift_key() {
                        m |= 0x01;
                    }
                    if modifiers.control_key() {
                        m |= 0x02;
                    }
                    if modifiers.alt_key() {
                        m |= 0x04;
                    }
                    if modifiers.super_key() {
                        m |= 0x08;
                    }
                    m
                };
                match event {
                    WindowEvent::CursorMoved { position, .. } => {
                        *last_mouse = *position;
                        if let Some((x, y)) = in_page(*position) {
                            let _ = remote.input(
                                tab_id,
                                InputEventMsg::MouseMove { x, y },
                            );
                        }
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        let Some((x, y)) = in_page(*last_mouse) else {
                            return;
                        };
                        let button = match button {
                            WinitMouseButton::Right => 1,
                            WinitMouseButton::Middle => 2,
                            _ => 0,
                        };
                        let pressed = matches!(state, ElementState::Pressed);
                        let _ = remote.input(
                            tab_id,
                            InputEventMsg::MouseButton {
                                x,
                                y,
                                button,
                                pressed,
                            },
                        );
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let Some((x, y)) = in_page(*last_mouse) else {
                            return;
                        };
                        let (dx, dy) = match delta {
                            MouseScrollDelta::LineDelta(dx, dy) => {
                                ((*dx as f64) * 76.0, (*dy as f64) * 76.0)
                            }
                            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                        };
                        let _ = remote.input(
                            tab_id,
                            InputEventMsg::MouseWheel { x, y, dx, dy },
                        );
                    }
                    WindowEvent::KeyboardInput { event: key_event, .. } => {
                        let key = match &key_event.logical_key {
                            WinitKey::Character(c) => c.to_string(),
                            WinitKey::Named(named) => format!("Named:{named:?}"),
                            other => format!("{other:?}"),
                        };
                        let _ = remote.input(
                            tab_id,
                            InputEventMsg::Key {
                                key,
                                pressed: key_event.state.is_pressed(),
                                modifiers: mods_byte,
                            },
                        );
                    }
                    WindowEvent::Ime(Ime::Commit(text)) => {
                        let _ = remote.input(
                            tab_id,
                            InputEventMsg::Text {
                                text: text.clone(),
                            },
                        );
                    }
                    WindowEvent::Ime(Ime::Preedit(text, _)) => {
                        let _ = remote.input(
                            tab_id,
                            InputEventMsg::Text {
                                text: text.clone(),
                            },
                        );
                    }
                    WindowEvent::Focused(true) => {
                        // Content focuses the active tab on FocusTab; no-op here.
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn focus_page_input(&mut self) {
        if let Self::Legacy(engine) = self {
            engine.focus_page_input();
        }
    }
}
