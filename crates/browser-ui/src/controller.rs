use browser_core::{
    normalize_url, Browser, NetworkRoute, ProxyConfig, RequestContext, Route, SettingsNetworkRouter,
    TabId,
};
use browser_engine::{EngineEvent, EngineViewId};
use crate::page_backend::PageBackend;
use browser_profile::{
    discover_browsers, import_from_detected, import_path, DetectedBrowser, NetworkMode,
    ProfileManager, ProfileStore, SessionState, SessionTab, StartupBehavior, WindowState,
};
use tracing::{error, info, warn};
use url::Url;

pub const SETTINGS_URL: &str = "about:settings";
pub const NEWTAB_URL: &str = "about:newtab";

pub fn is_settings_url(url: Option<&Url>) -> bool {
    url.map(|u| u.as_str() == SETTINGS_URL).unwrap_or(false)
}

pub fn is_new_tab_url(url: Option<&Url>) -> bool {
    match url.map(|u| u.as_str()) {
        None => true,
        Some(s) => matches!(s, "about:blank" | "about:newtab" | "about:home" | ""),
    }
}

/// Pages drawn by egui (Servo stays on about:blank).
pub fn is_chrome_ui_url(url: Option<&Url>) -> bool {
    is_settings_url(url)
        || matches!(
            url.map(|u| u.as_str()),
            Some("about:newtab" | "about:home")
        )
}

fn engine_url_for_tab(url: Option<Url>) -> Option<Url> {
    if is_chrome_ui_url(url.as_ref()) || url.is_none() {
        Some(Url::parse("about:blank").expect("about:blank"))
    } else {
        url
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Appearance,
    Privacy,
    Security,
    Network,
    Vpn,
    Profiles,
    Downloads,
    Bookmarks,
    History,
    Extensions,
    Advanced,
    About,
    Language,
    Import,
    Passwords,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VpnUiState {
    #[default]
    Off,
    Connecting,
    On,
    Error,
    Auto,
}

pub struct BrowserController {
    pub browser: Browser,
    pub store: ProfileStore,
    pub profiles: ProfileManager,
    pub address: String,
    pub address_dirty: bool,
    pub focus_address: bool,
    /// Separate query buffer for the home-page search field (not the tab URL).
    pub home_search: String,
    pub focus_home_search: bool,
    pub show_history: bool,
    pub show_bookmarks: bool,
    pub show_downloads: bool,
    pub show_settings: bool,
    pub show_restore_prompt: bool,
    pub show_command_palette: bool,
    pub show_vpn_popover: bool,
    pub show_profile_popover: bool,
    pub show_import_modal: bool,
    pub show_main_menu: bool,
    pub settings_tab: SettingsTab,
    /// Draft fields for adding a local host mapping in Network settings.
    pub local_host_domain: String,
    pub local_host_ip: String,
    pub import_path_buf: String,
    pub import_status: String,
    pub import_bookmarks: bool,
    pub import_history: bool,
    pub import_passwords: bool,
    pub detected_browsers: Vec<DetectedBrowser>,
    pub browsers_discovered: bool,
    pub selected_import_source: Option<String>,
    pub command_query: String,
    pub vpn_state: VpnUiState,
    pub pending_session: Option<SessionState>,
    pub status: String,
    /// When set, the status toast auto-hides after this instant.
    pub status_expires_at: Option<std::time::Instant>,
    /// Cached library panel rows — refreshed on open / mutation, not every frame.
    pub history_cache: Option<Vec<browser_profile::HistoryEntry>>,
    pub bookmarks_cache: Option<Vec<browser_profile::Bookmark>>,
    pub downloads_cache: Option<Vec<browser_profile::Download>>,
    /// Cached wallpaper texture for the home / new-tab page.
    pub home_bg_texture: Option<egui::TextureHandle>,
    pub home_bg_path_loaded: Option<std::path::PathBuf>,
    pub browser_icons: crate::browser_icons::BrowserIconCache,
    /// Debounced session writer lives on the store; last window geometry for checkpoints.
    pub last_window: WindowState,
    pub unclean_exit_detected: bool,
    /// Auto-recover crashed tabs after a short delay.
    pub pending_tab_recoveries: Vec<(TabId, std::time::Instant)>,
}

impl BrowserController {
    pub fn bootstrap(profiles_root: std::path::PathBuf) -> browser_core::BrowserResult<Self> {
        let profiles = ProfileManager::open(profiles_root)?;
        let store = profiles.open_active_store()?;
        let homepage = store.settings.homepage_url().unwrap_or_else(|_| {
            Url::parse("about:newtab").unwrap_or_else(|_| Url::parse("about:blank").unwrap())
        });

        let unclean = store.session.previous_session_unclean();
        let pending_session = store.session.load()?;
        let (browser, show_restore_prompt) = match (&store.settings.startup_behavior, &pending_session)
        {
            (StartupBehavior::RestorePreviousSession, Some(session)) if !session.tabs.is_empty() => {
                (browser_from_session(session, &homepage)?, false)
            }
            (StartupBehavior::AskToRestore, Some(session))
                if !session.tabs.is_empty()
                    && (store.settings.restore_previous_session || unclean) =>
            {
                (Browser::new(homepage.clone()), true)
            }
            _ if unclean && pending_session.as_ref().is_some_and(|s| !s.tabs.is_empty()) => {
                (Browser::new(homepage.clone()), true)
            }
            _ => (Browser::new(homepage), false),
        };

        let address = browser
            .active()
            .ok()
            .and_then(|t| t.url.as_ref().map(|u| u.to_string()))
            .unwrap_or_default();

        store.session.mark_running()?;

        info!(unclean_exit = unclean, "browser startup");
        Ok(Self {
            browser,
            store,
            profiles,
            address,
            address_dirty: false,
            focus_address: false,
            home_search: String::new(),
            focus_home_search: false,
            show_history: false,
            show_bookmarks: false,
            show_downloads: false,
            show_settings: false,
            show_restore_prompt,
            show_command_palette: false,
            show_vpn_popover: false,
            show_profile_popover: false,
            show_import_modal: false,
            show_main_menu: false,
            settings_tab: SettingsTab::General,
            local_host_domain: String::new(),
            local_host_ip: String::new(),
            import_path_buf: String::new(),
            import_status: String::new(),
            import_bookmarks: true,
            import_history: true,
            import_passwords: false,
            detected_browsers: Vec::new(),
            browsers_discovered: false,
            selected_import_source: None,
            command_query: String::new(),
            vpn_state: VpnUiState::Off,
            pending_session: if show_restore_prompt {
                pending_session
            } else {
                None
            },
            status: if unclean {
                "Previous session did not exit cleanly.".into()
            } else {
                String::new()
            },
            status_expires_at: if unclean {
                Some(std::time::Instant::now() + std::time::Duration::from_secs(6))
            } else {
                None
            },
            history_cache: None,
            bookmarks_cache: None,
            downloads_cache: None,
            home_bg_texture: None,
            home_bg_path_loaded: None,
            browser_icons: crate::browser_icons::BrowserIconCache::default(),
            last_window: WindowState::default(),
            unclean_exit_detected: unclean,
            pending_tab_recoveries: Vec::new(),
        })
    }

    pub fn sync_engine_views(&mut self, engine: &mut PageBackend) {
        let tab_ids: Vec<_> = self.browser.tabs.iter().map(|t| t.id).collect();
        for id in &tab_ids {
            let view = EngineViewId(*id);
            let url = self.browser.tab(*id).ok().and_then(|t| t.url.clone());
            if let Err(err) = engine.create_view(view, engine_url_for_tab(url)) {
                error!(?err, "engine errors");
            }
        }
        let _ = engine.focus_view(EngineViewId(self.browser.active_tab));
        engine.set_page_zoom(self.store.settings.default_zoom);
    }

    pub fn apply_engine_events(&mut self, events: Vec<EngineEvent>) {
        for event in events {
            match event {
                EngineEvent::UrlChanged { view, url } => {
                    // Never let Servo's about:blank overwrite chrome UI URLs.
                    if self
                        .browser
                        .tab(view.0)
                        .ok()
                        .map(|t| is_chrome_ui_url(t.url.as_ref()))
                        .unwrap_or(false)
                    {
                        if view.0 == self.browser.active_tab && !self.address_dirty {
                            if let Some(u) = self.browser.tab(view.0).ok().and_then(|t| t.url.clone())
                            {
                                self.address = u.to_string();
                            }
                        }
                        self.show_settings = self.settings_tab_active();
                        continue;
                    }
                    let _ = self.browser.update_tab_url(view.0, url.clone());
                    if view.0 == self.browser.active_tab && !self.address_dirty {
                        self.address = url.to_string();
                    }
                    self.show_settings = self.settings_tab_active();
                    if url.scheme() == "about" {
                        continue;
                    }
                    let title = self
                        .browser
                        .tab(view.0)
                        .map(|t| t.title.clone())
                        .unwrap_or_default();
                    if let Err(err) = self.store.history.record_visit(&url, &title) {
                        error!(?err, "database errors");
                    }
                }
                EngineEvent::TitleChanged { view, title } => {
                    if self
                        .browser
                        .tab(view.0)
                        .ok()
                        .map(|t| is_settings_url(t.url.as_ref()))
                        .unwrap_or(false)
                    {
                        let lang = self.store.settings.language;
                        let _ = self.browser.update_tab_title(
                            view.0,
                            crate::i18n::t(lang, "settings").to_string(),
                        );
                        continue;
                    }
                    if self
                        .browser
                        .tab(view.0)
                        .ok()
                        .map(|t| {
                            matches!(
                                t.url.as_ref().map(|u| u.as_str()),
                                Some("about:newtab" | "about:home")
                            )
                        })
                        .unwrap_or(false)
                    {
                        let lang = self.store.settings.language;
                        let _ = self.browser.update_tab_title(
                            view.0,
                            crate::i18n::t(lang, "new_tab").to_string(),
                        );
                        continue;
                    }
                    let title = title.unwrap_or_else(|| "New Tab".into());
                    let _ = self.browser.update_tab_title(view.0, title);
                }
                EngineEvent::LoadStatusChanged { view, loading } => {
                    let _ = self.browser.set_loading(view.0, loading);
                }
                EngineEvent::HistoryChanged {
                    view,
                    can_go_back,
                    can_go_forward,
                } => {
                    let _ = self
                        .browser
                        .set_nav_state(view.0, can_go_back, can_go_forward);
                }
                EngineEvent::NewFrameReady { .. } => {}
                EngineEvent::CursorChanged { .. } => {
                    // Applied in app.rs (window cursor icon).
                }
                EngineEvent::Crashed { view, reason } => {
                    error!(tab = %view.0, %reason, "engine crash");
                    let _ = self.browser.mark_tab_crashed(view.0, reason.clone());
                    self.push_status(format!("This tab crashed. Reloading… ({reason})"));
                    self.schedule_session_checkpoint();
                    // Auto-recover after a short delay so the user sees the crashed state.
                    self.pending_tab_recoveries.push((
                        view.0,
                        std::time::Instant::now() + std::time::Duration::from_millis(400),
                    ));
                }
                EngineEvent::PermissionRequested { view, feature } => {
                    warn!(tab = %view.0, %feature, "permission denied (Ask — change in Settings)");
                    self.push_status(format!(
                        "Permission '{feature}' denied — allow it in Settings → Privacy"
                    ));
                }
            }
        }
    }

    pub fn tick_recoveries(&mut self, engine: &mut PageBackend) {
        let now = std::time::Instant::now();
        let due: Vec<TabId> = self
            .pending_tab_recoveries
            .iter()
            .filter(|(_, at)| *at <= now)
            .map(|(id, _)| *id)
            .collect();
        self.pending_tab_recoveries
            .retain(|(id, at)| *at > now && !due.contains(id));
        for id in due {
            self.recover_tab(engine, id);
        }
    }

    /// Recreate the WebView for a crashed tab and restore its URL.
    pub fn recover_tab(&mut self, engine: &mut PageBackend, id: TabId) {
        let url = self.browser.tab(id).ok().and_then(|t| t.url.clone());
        let reason = self
            .browser
            .tab(id)
            .ok()
            .and_then(|t| t.last_crash_reason.clone())
            .unwrap_or_else(|| "unknown".into());
        if let Err(err) = self.browser.begin_tab_recovery(id) {
            warn!(?err, "begin recovery");
            return;
        }
        info!(tab = %id, %reason, "webview recreate after crash");
        if let Err(err) = engine.recreate_view(EngineViewId(id), engine_url_for_tab(url.clone())) {
            error!(?err, "recreate view failed");
            self.push_status(format!("Recovery failed: {err}"));
            return;
        }
        if let Some(url) = url {
            if !is_chrome_ui_url(Some(&url)) {
                if let Err(err) = self.navigate_engine(engine, id, url) {
                    error!(?err, "recovery navigate");
                }
            }
        }
        let _ = engine.focus_view(EngineViewId(id));
        self.schedule_session_checkpoint();
        self.push_status("Tab recovered");
    }

    pub fn network_router(&self) -> SettingsNetworkRouter {
        let preferred = match self.store.settings.network_mode {
            NetworkMode::Direct => NetworkRoute::Direct,
            NetworkMode::Proxy => NetworkRoute::Proxy,
            NetworkMode::Vpn => NetworkRoute::Vpn,
        };
        let proxy = if self.store.settings.proxy_uri.trim().is_empty() {
            None
        } else {
            Some(ProxyConfig {
                http_proxy_uri: self.store.settings.proxy_uri.clone(),
                https_proxy_uri: self.store.settings.proxy_uri.clone(),
                no_proxy: self.store.settings.proxy_no_proxy.clone(),
            })
        };
        SettingsNetworkRouter {
            preferred,
            proxy,
            // Honest: no tunnel backend in P0.
            vpn_available: false,
            vpn_profile: None,
        }
    }

    fn navigate_engine(
        &mut self,
        engine: &mut PageBackend,
        id: TabId,
        url: Url,
    ) -> browser_core::BrowserResult<()> {
        use browser_core::NetworkRouter;
        let ctx = RequestContext {
            url: url.clone(),
            tab_id: Some(id.to_string()),
            profile_id: None,
        };
        let route = self.network_router().route(&ctx);
        match &route {
            Route::Unavailable { reason } | Route::Block { reason } => {
                self.push_status(reason.clone());
                return Err(browser_core::BrowserError::Other(reason.clone()));
            }
            Route::Vpn(_) => {
                let reason = "VPN selected but no tunnel backend is available".to_string();
                self.push_status(reason.clone());
                return Err(browser_core::BrowserError::Other(reason));
            }
            Route::Direct | Route::Proxy(_) => {}
        }
        engine.apply_route(&route)?;
        engine.navigate(EngineViewId(id), url)?;
        self.schedule_session_checkpoint();
        Ok(())
    }

    pub fn navigate_address(&mut self, engine: &mut PageBackend) {
        let template = self.store.settings.search_engine.clone();
        let bare_host_input = !self.address.trim().contains("://");
        match normalize_url(&self.address, &template) {
            Ok(mut url) => {
                browser_profile::apply_local_host_overrides(
                    &mut url,
                    &self.store.settings.local_hosts,
                    bare_host_input,
                );
                if url.as_str() == SETTINGS_URL {
                    self.open_settings(engine);
                    return;
                }
                if matches!(url.as_str(), NEWTAB_URL | "about:home") {
                    let id = self.browser.active_tab;
                    let lang = self.store.settings.language;
                    let _ = self.browser.navigate_active(url.clone());
                    let _ = self
                        .browser
                        .update_tab_title(id, crate::i18n::t(lang, "new_tab").to_string());
                    self.address = url.to_string();
                    self.address_dirty = false;
                    self.show_settings = false;
                    return;
                }
                self.address = url.to_string();
                self.address_dirty = false;
                self.show_settings = false;
                info!(%url, "navigation");
                let id = self.browser.active_tab;
                if let Err(err) = self.browser.navigate_active(url.clone()) {
                    warn!(?err, "navigate state");
                }
                if let Err(err) = self.navigate_engine(engine, id, url) {
                    error!(?err, "engine errors");
                }
            }
            Err(err) => {
                self.push_status(err.to_string());
            }
        }
    }

    /// Open Settings as a top-level browser tab (`about:settings`).
    pub fn open_settings(&mut self, engine: &mut PageBackend) {
        let lang = self.store.settings.language;
        let title = crate::i18n::t(lang, "settings").to_string();
        let settings_url = Url::parse(SETTINGS_URL).expect("about:settings");

        if let Some(existing) = self
            .browser
            .tabs
            .iter()
            .find(|t| is_settings_url(t.url.as_ref()))
            .map(|t| t.id)
        {
            self.switch_tab(engine, existing);
            self.show_settings = true;
            self.address = SETTINGS_URL.into();
            return;
        }

        let id = self.browser.new_tab(Some(settings_url));
        let _ = self.browser.update_tab_title(id, title);
        // Keep Servo on a blank document — settings UI is drawn by egui.
        if let Err(err) = engine.create_view(
            EngineViewId(id),
            Some(Url::parse("about:blank").unwrap()),
        ) {
            error!(?err, "engine errors");
        }
        let _ = engine.focus_view(EngineViewId(id));
        self.show_settings = true;
        self.address = SETTINGS_URL.into();
        self.address_dirty = false;
    }

    pub fn settings_tab_active(&self) -> bool {
        self.browser
            .active()
            .ok()
            .map(|t| is_settings_url(t.url.as_ref()))
            .unwrap_or(false)
    }

    /// Close the settings tab if it is open (does nothing otherwise).
    pub fn close_settings_tab(&mut self, engine: &mut PageBackend) {
        let Some(id) = self
            .browser
            .tabs
            .iter()
            .find(|t| is_settings_url(t.url.as_ref()))
            .map(|t| t.id)
        else {
            self.show_settings = false;
            return;
        };
        if self.browser.active_tab == id {
            self.close_active_tab(engine);
        } else {
            let _ = self.browser.close_tab(id);
            let _ = engine.destroy_view(EngineViewId(id));
            self.show_settings = self.settings_tab_active();
        }
    }

    pub fn new_tab(&mut self, engine: &mut PageBackend) {
        self.show_settings = false;
        self.home_search.clear();
        let newtab = Url::parse(NEWTAB_URL).expect("about:newtab");
        let id = self.browser.new_tab(Some(newtab.clone()));
        let lang = self.store.settings.language;
        let _ = self
            .browser
            .update_tab_title(id, crate::i18n::t(lang, "new_tab").to_string());
        if let Err(err) = engine.create_view(
            EngineViewId(id),
            Some(Url::parse("about:blank").expect("about:blank")),
        ) {
            error!(?err, "engine errors");
        }
        engine.set_page_zoom(self.store.settings.default_zoom);
        self.sync_address_from_active();
        self.schedule_session_checkpoint();
    }

    pub fn apply_page_zoom(&mut self, engine: &mut PageBackend) {
        engine.set_page_zoom(self.store.settings.default_zoom);
    }

    pub fn set_home_background_from_path(&mut self, path: std::path::PathBuf) {
        let dest = self
            .store
            .paths
            .cache
            .join(format!(
                "home_background{}",
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!(".{e}"))
                    .unwrap_or_default()
            ));
        match std::fs::copy(&path, &dest) {
            Ok(_) => {
                self.store.settings.home_background = Some(dest);
                self.home_bg_texture = None;
                self.home_bg_path_loaded = None;
                let _ = self.store.save_settings();
                self.push_status("Background updated");
            }
            Err(err) => self.push_status(err.to_string()),
        }
    }

    pub fn clear_home_background(&mut self) {
        if let Some(path) = self.store.settings.home_background.take() {
            let _ = std::fs::remove_file(path);
        }
        self.home_bg_texture = None;
        self.home_bg_path_loaded = None;
        let _ = self.store.save_settings();
    }

    pub fn ensure_home_background_texture(&mut self, ctx: &egui::Context) {
        let Some(path) = self.store.settings.home_background.clone() else {
            self.home_bg_texture = None;
            self.home_bg_path_loaded = None;
            return;
        };
        if self.home_bg_path_loaded.as_ref() == Some(&path) && self.home_bg_texture.is_some() {
            return;
        }
        self.home_bg_texture = None;
        self.home_bg_path_loaded = None;
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Ok(img) = image::load_from_memory(&bytes) else {
            return;
        };
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        self.home_bg_texture =
            Some(ctx.load_texture("home_background", color, egui::TextureOptions::LINEAR));
        self.home_bg_path_loaded = Some(path);
    }

    pub fn switch_tab(&mut self, engine: &mut PageBackend, id: TabId) {
        if let Err(err) = self.browser.switch_tab(id) {
            warn!(?err, "switch tab");
            return;
        }
        let _ = engine.focus_view(EngineViewId(id));
        self.sync_address_from_active();
        self.show_settings = self.settings_tab_active();
        self.schedule_session_checkpoint();
    }

    pub fn close_active_tab(&mut self, engine: &mut PageBackend) {
        let id = self.browser.active_tab;
        if let Err(err) = self.browser.close_tab(id) {
            warn!(?err, "close tab");
            return;
        }
        let _ = engine.destroy_view(EngineViewId(id));
        // If last tab was reset, recreate engine view for new id.
        let active = self.browser.active_tab;
        let url = self.browser.active().ok().and_then(|t| t.url.clone());
        let _ = engine.create_view(EngineViewId(active), engine_url_for_tab(url.clone()));
        let _ = engine.focus_view(EngineViewId(active));
        engine.set_page_zoom(self.store.settings.default_zoom);
        // Chrome new-tab UI: don't leave TabState::Creating (spinner) forever waiting
        // for Servo Complete on an empty / about:blank document.
        if is_new_tab_url(url.as_ref()) {
            let _ = self.browser.set_loading(active, false);
            if url.is_none() {
                let newtab = Url::parse(NEWTAB_URL).expect("about:newtab");
                let _ = self.browser.navigate_active(newtab.clone());
                let _ = self.browser.set_loading(active, false);
                let lang = self.store.settings.language;
                let _ = self
                    .browser
                    .update_tab_title(active, crate::i18n::t(lang, "new_tab").to_string());
            }
        }
        self.sync_address_from_active();
        self.show_settings = self.settings_tab_active();
        self.schedule_session_checkpoint();
    }

    pub fn restore_closed(&mut self, engine: &mut PageBackend) {
        if let Some(id) = self.browser.restore_closed_tab() {
            let url = self.browser.tab(id).ok().and_then(|t| t.url.clone());
            let _ = engine.create_view(EngineViewId(id), engine_url_for_tab(url));
            let _ = engine.focus_view(EngineViewId(id));
            engine.set_page_zoom(self.store.settings.default_zoom);
            self.sync_address_from_active();
            self.show_settings = self.settings_tab_active();
        }
    }

    pub fn bookmark_active(&mut self) {
        let Ok(tab) = self.browser.active() else {
            return;
        };
        let Some(url) = tab.url.clone() else {
            return;
        };
        let title = tab.title.clone();
        match self.store.bookmarks.add(&url, &title, None, true) {
            Ok(_) => {
                self.bookmarks_cache = None;
                self.push_status("Bookmark added");
            }
            Err(err) => {
                error!(?err, "database errors");
                self.push_status(err.to_string());
            }
        }
    }

    pub fn download_active(&mut self) {
        let Ok(tab) = self.browser.active() else {
            return;
        };
        let Some(url) = tab.url.clone() else {
            return;
        };
        match self.store.downloads.start(url) {
            Ok(_) => {
                self.push_status("Download started");
                self.show_downloads = true;
            }
            Err(err) => {
                error!(?err, "download errors");
                self.push_status(err.to_string());
            }
        }
    }

    pub fn save_session(&self, window: WindowState) {
        let tabs: Vec<SessionTab> = self.browser.tabs.iter().map(SessionTab::from).collect();
        let state = SessionState {
            tabs,
            active_tab: self.browser.active_tab,
            window,
        };
        if let Err(err) = self.store.session.save(&state) {
            error!(?err, "session save");
        }
        if let Err(err) = self.store.save_settings() {
            error!(?err, "settings save");
        }
    }

    pub fn schedule_session_checkpoint(&mut self) {
        let tabs: Vec<SessionTab> = self.browser.tabs.iter().map(SessionTab::from).collect();
        let state = SessionState {
            tabs,
            active_tab: self.browser.active_tab,
            window: self.last_window.clone(),
        };
        self.store.session.schedule(state);
    }

    pub fn flush_session_checkpoint(&mut self) {
        let _ = self.store.session.flush();
    }

    pub fn clean_shutdown(&mut self, window: WindowState) {
        self.last_window = window.clone();
        self.save_session(window);
        if let Err(err) = self.store.session.mark_clean_shutdown() {
            error!(?err, "clear running.lock");
        }
    }

    pub fn accept_restore(&mut self, engine: &mut PageBackend) {
        let Some(session) = self.pending_session.take() else {
            self.show_restore_prompt = false;
            return;
        };
        self.show_restore_prompt = false;
        match browser_from_session(&session, &Url::parse("https://example.com").unwrap()) {
            Ok(browser) => {
                self.browser = browser;
                self.sync_engine_views(engine);
                self.sync_address_from_active();
            }
            Err(err) => warn!(?err, "restore session"),
        }
    }

    pub fn decline_restore(&mut self) {
        self.pending_session = None;
        self.show_restore_prompt = false;
    }

    pub fn refresh_detected_browsers(&mut self) {
        self.detected_browsers = discover_browsers();
        self.browsers_discovered = true;
    }

    /// Discover installed browsers once (or after an explicit refresh).
    pub fn ensure_detected_browsers(&mut self) {
        if !self.browsers_discovered {
            self.refresh_detected_browsers();
        }
    }

    pub fn invalidate_library_caches(&mut self) {
        self.history_cache = None;
        self.bookmarks_cache = None;
        self.downloads_cache = None;
    }

    pub fn ensure_history_cache(&mut self) {
        if self.history_cache.is_none() {
            self.history_cache = self.store.history.list_recent(100).ok();
        }
    }

    pub fn ensure_bookmarks_cache(&mut self) {
        if self.bookmarks_cache.is_none() {
            self.bookmarks_cache = self.store.bookmarks.list_all().ok();
        }
    }

    pub fn ensure_downloads_cache(&mut self) {
        if self.downloads_cache.is_none() {
            self.downloads_cache = self.store.downloads.list().ok();
        }
    }

    pub fn run_import_detected(&mut self, engine: &mut PageBackend, source_id: &str) {
        let Some(source) = self
            .detected_browsers
            .iter()
            .find(|b| b.id == source_id)
            .cloned()
        else {
            self.import_status = "Browser profile not found — try Refresh".into();
            return;
        };
        match import_from_detected(&source, &self.store.bookmarks, &self.store.credentials) {
            Ok((summary, tabs)) => self.apply_import_result(engine, summary, tabs),
            Err(err) => {
                self.import_status = err.to_string();
                self.push_status(err.to_string());
            }
        }
    }

    pub fn run_import(&mut self, engine: &mut PageBackend) {
        let path = std::path::PathBuf::from(self.import_path_buf.trim());
        if path.as_os_str().is_empty() {
            self.import_status = "Choose a file or folder first".into();
            return;
        }
        match import_path(&path, &self.store.bookmarks, &self.store.credentials) {
            Ok((summary, tabs)) => self.apply_import_result(engine, summary, tabs),
            Err(err) => {
                self.import_status = err.to_string();
                self.push_status(err.to_string());
            }
        }
    }

    fn apply_import_result(
        &mut self,
        engine: &mut PageBackend,
        summary: browser_profile::ImportSummary,
        tabs: Vec<browser_profile::ImportedTab>,
    ) {
        for tab in &tabs {
            let id = self.browser.new_tab(Some(tab.url.clone()));
            let _ = self.browser.update_tab_title(id, tab.title.clone());
            if let Err(err) = engine.create_view(EngineViewId(id), Some(tab.url.clone())) {
                error!(?err, "engine errors");
            }
        }
        if !tabs.is_empty() {
            let id = self.browser.active_tab;
            let _ = engine.focus_view(EngineViewId(id));
            self.sync_address_from_active();
        }
        let mut msg = summary.describe();
        for note in &summary.notes {
            msg.push_str("\n");
            msg.push_str(note);
        }
        self.import_status = msg.clone();
        self.push_status(summary.describe());
    }

    pub fn sync_address_from_active(&mut self) {
        self.address = self
            .browser
            .active()
            .ok()
            .and_then(|t| t.url.as_ref().map(|u| u.to_string()))
            .unwrap_or_default();
        self.address_dirty = false;
    }

    /// Show a toast that auto-dismisses after a few seconds.
    pub fn push_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_expires_at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
    }

    pub fn dismiss_status(&mut self) {
        self.status.clear();
        self.status_expires_at = None;
    }

    /// Hide expired toasts; returns true if a repaint should be scheduled.
    pub fn tick_status(&mut self) -> bool {
        if self.status.is_empty() {
            return false;
        }
        if let Some(until) = self.status_expires_at {
            if std::time::Instant::now() >= until {
                self.dismiss_status();
                return true;
            }
            return true; // still visible — keep ticking
        }
        // Legacy assignment without expiry — start the timer now.
        self.status_expires_at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
        true
    }
}

fn browser_from_session(
    session: &SessionState,
    fallback: &Url,
) -> browser_core::BrowserResult<Browser> {
    let mut tabs = Vec::new();
    for t in &session.tabs {
        let mut tab = match &t.url {
            Some(u) => browser_core::Tab::with_url(u.clone()),
            None => browser_core::Tab::new(None),
        };
        tab.id = t.id;
        tab.title = t.title.clone();
        let _ = tab.transition(browser_core::TabState::Ready);
        tabs.push(tab);
    }
    if tabs.is_empty() {
        tabs.push(browser_core::Tab::with_url(fallback.clone()));
    }
    let active = if tabs.iter().any(|t| t.id == session.active_tab) {
        session.active_tab
    } else {
        tabs[0].id
    };
    Browser::from_tabs(tabs, active)
}
