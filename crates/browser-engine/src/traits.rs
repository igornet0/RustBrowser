use crate::events::{EngineEvent, EngineViewId};
use browser_core::{BrowserResult, ProxyConfig, Route};
use url::Url;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;

/// Abstraction over a browser engine backend (Servo today).
///
/// UI and core must depend on this trait for navigation — not on Servo types.
pub trait BrowserEngine {
    fn create_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()>;
    fn destroy_view(&mut self, id: EngineViewId) -> BrowserResult<()>;
    /// Destroy + create the same id, clearing stale WebView references.
    fn recreate_view(&mut self, id: EngineViewId, url: Option<Url>) -> BrowserResult<()> {
        self.destroy_view(id)?;
        self.create_view(id, url)
    }
    fn focus_view(&mut self, id: EngineViewId) -> BrowserResult<()>;

    fn navigate(&mut self, id: EngineViewId, url: Url) -> BrowserResult<()>;
    fn reload(&mut self, id: EngineViewId) -> BrowserResult<()>;
    fn go_back(&mut self, id: EngineViewId) -> BrowserResult<()>;
    fn go_forward(&mut self, id: EngineViewId) -> BrowserResult<()>;

    fn can_go_back(&self, id: EngineViewId) -> bool;
    fn can_go_forward(&self, id: EngineViewId) -> bool;

    /// Apply a network route (proxy prefs). VPN Unavailable must be handled before this.
    fn apply_route(&mut self, route: &Route) -> BrowserResult<()>;

    fn resize(&mut self, size: PhysicalSize<u32>) -> BrowserResult<()>;
    fn paint(&mut self) -> BrowserResult<()>;
    fn present(&mut self) -> BrowserResult<()>;
    fn spin(&mut self);

    /// Forward input to the focused web view. `content_origin_y` is the physical
    /// Y offset of page content below the chrome toolbar.
    fn handle_window_event(
        &mut self,
        event: &WindowEvent,
        content_origin_y: f64,
        scale_factor: f64,
    );

    fn poll_events(&mut self) -> Vec<EngineEvent>;
}

/// Helper used by backends when clearing proxy prefs for Direct.
pub fn clear_proxy_config() -> ProxyConfig {
    ProxyConfig {
        http_proxy_uri: String::new(),
        https_proxy_uri: String::new(),
        no_proxy: String::new(),
    }
}
