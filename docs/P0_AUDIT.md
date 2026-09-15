# P0 Audit — Browser Core Foundation

**Date:** 2026-09-14  
**Scope:** factual state of `rust-browser` before P0 implementation  
**Engine:** Servo `=0.5.0` (crates.io)

## 1. Process / embedding topology

```
ONE OS PROCESS (browser-app)
 └── winit EventLoop
      └── browser-ui (egui chrome)
           └── BrowserController
                ├── browser-core (tabs, NetworkPolicy*)
                ├── browser-profile (SQLite / JSON)
                └── ServoEngine
                     ├── ONE Servo instance
                     ├── shared OffscreenRenderingContext
                     └── HashMap<EngineViewId, WebView>  (one WebView per tab)
```

\* `NetworkPolicy` exists but is **not** consulted on navigate.

**Verdict:** Not process-isolated. Not sandboxed at the application layer. Not site-isolated.

## 2. Where things live

| Concern | Location | Notes |
|---------|----------|--------|
| Servo create | `browser-engine/src/servo_backend.rs` → `ServoEngine::new` | `ServoBuilder` + prefs + `opts.config_dir` |
| WebView create | `ServoEngine::create_view` | `WebViewBuilder::new(&self.servo, page_context)` |
| WebView destroy | `ServoEngine::destroy_view` | Removes from map; drop closes WebView |
| Tab model | `browser-core/src/tab.rs` | `loading: bool` only — no crash lifecycle |
| Tab create/close | `browser-core/src/browser.rs` + `controller.rs` | close → `destroy_view`; new → `create_view` |
| Engine events | `browser-engine/src/events.rs` | Includes `EngineEvent::Crashed` |
| Crash notify | `WebViewDelegate::notify_crashed` → `EngineEvent::Crashed` | Controller: toast only |
| Navigation | `controller.navigate_address` → `engine.navigate` | Bypasses NetworkPolicy |
| NetworkPolicy | `browser-core/src/network.rs` | `DirectNetworkPolicy` always Direct; unused by UI |
| Session | `browser-profile/src/session.rs` | Direct `fs::write` to `session.json` (not atomic) |
| Session when | `controller.save_session` on window close | No periodic checkpoint |
| Profile paths | `browser-profile/src/paths.rs` | Per-profile root under profiles dir |
| Passwords | `passwords.db` SQLite | **plaintext** `password TEXT` |
| config_dir | profile `storage/` | Servo cookies / site data |
| cache/ | created on disk | Not wired to app HTTP cache controls |

## 3. Servo 0.5.0 embedder APIs (verified in registry source)

| API | Available |
|-----|-----------|
| `WebViewDelegate::notify_crashed` | Yes |
| `WebViewDelegate::request_permission` + `PermissionRequest` | Yes |
| `Preferences` HTTP(S) proxy fields + `Servo::set_preference` | Yes |
| `Servo::site_data_manager()` | Yes (`clear_cookies`, `clear_site_data`, …) |
| `Servo::network_manager().clear_cache()` | Yes |
| Out-of-process content / sandbox control from our crate | **No** |

`PermissionFeature` includes Camera, Microphone, Geolocation, Notifications, etc.

## 4. Crash / hang behaviour today

| Scenario | Behaviour |
|----------|-----------|
| Soft crash (`notify_crashed`) | Status toast; WebView left as-is; no recreate |
| Hard panic in process | Whole browser dies |
| Infinite JS / blocked event loop | Whole UI freezes; no watchdog |
| Session after kill | Only if `session.json` was written earlier (normally clean close) |

## 5. Storage ownership (current)

| Data | Owner | Scope |
|------|-------|-------|
| History / bookmarks / downloads | `browser-profile` SQLite | Profile |
| Settings | `preferences.json` | Profile |
| Session | `session.json` | Profile |
| Credentials | `passwords.db` plaintext | Profile |
| Cookies / local/session storage | Servo via `config_dir` | Profile (shared across tabs) |
| HTTP cache | Servo internal (+ unused `cache/` dir) | Profile-ish |

No CHIPS / storage-key partitioning controls in app code.

## 6. Gaps vs P0 Definition of Done

| Requirement | Pre-P0 |
|-------------|--------|
| Tab lifecycle states | Missing (`loading` bool only) |
| WebView recreate on crash | Missing |
| Watchdog | Missing |
| Periodic + atomic session | Missing |
| Dirty-exit detection | Missing |
| NetworkPolicy on navigate | Missing |
| Credential secure storage | Missing |
| SiteDataManager abstraction | Missing |
| PermissionManager | Missing (Servo delegate unused) |
| Honest security docs | Partial (ARCHITECTURE only) |

## 7. Implementation order (this phase)

1. P0.1 TabState  
2. P0.2 WebView recreate  
3. P0.3 Watchdog (in-process limits documented)  
4. P0.4 Periodic atomic session checkpoint  
5. P0.5 Crash startup recovery (`running.lock`)  
6. P0.6 NetworkRouter wiring (Direct + Proxy; VPN = unavailable)  
7. P0.7 CredentialStore + plaintext migration  
8. P0.8 SiteDataManager + PermissionManager + remaining docs  

P1 process isolation is **out of scope** for this phase; APIs should leave a seam (`ContentProcess` / local vs remote).
