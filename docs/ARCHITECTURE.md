# Architecture

## Layering

```
Browser UI (winit + egui)
    ↓
Browser Controller
    ↓
Browser Core (tabs, URL helpers, errors, network policy)
    ↓
Browser Engine abstraction (`BrowserEngine` trait)
    ↓
Servo 0.5.0 (`WebView` / `ServoBuilder` / `RenderingContext`)
```

UI never imports Servo types except in the thin `browser-engine` / `browser-ui` glue that implements embedding. Navigation and tab logic go through `BrowserEngine`.

## Browser Core modules

```
Browser Core
 ├── Tabs (`Browser`, `Tab`, `TabId`)
 ├── History          → browser-profile / SQLite
 ├── Bookmarks        → browser-profile / SQLite
 ├── Profiles         → browser-profile
 ├── Downloads        → browser-profile (app-level HTTP)
 ├── Settings         → preferences.json
 └── Session         → session.json
```

History, bookmarks, profiles, downloads, settings, and session live in `browser-profile` so persistence stays in one place without a crate explosion.

## Networking

```
Browser
    ↓
NetworkPolicy (trait)
    ↓
NetworkRoute::{ Direct | Proxy | Vpn }
    ↓
Engine networking (Servo / rustls)
```

MVP policy: `DirectNetworkPolicy` → `NetworkRoute::Direct` for every URL.

Certificate validation is **not** disabled. There is no `accept_invalid_certs`.

VPN is intentionally not implemented. The route enum and per-tab policy hook exist so later versions can do:

- Tab 1 → Direct  
- Tab 2 → VPN  
- Tab 3 → Direct  

without rewriting UI/core.

## Engine embedding

Chosen stack (researched 2026-09-14):

| Piece | Choice |
|-------|--------|
| Engine | `servo = "=0.5.0"` (crates.io) |
| Window | `winit` 0.30.x |
| Chrome | `egui` + `egui_glow` |
| Page surface | `WindowRenderingContext` + `OffscreenRenderingContext` |
| TLS | Servo/rustls + `aws-lc-rs` provider |

Flow:

1. Create winit window and `EventLoopWaker`.
2. Build `WindowRenderingContext` for the window.
3. Create page-sized `OffscreenRenderingContext` via `offscreen_context`.
4. `ServoBuilder` → `WebViewBuilder` per tab.
5. On redraw: `WebView::paint`, blit page under chrome, paint egui, `present`.

## Crates

| Crate | Role |
|-------|------|
| `browser-core` | Models, errors, network policy, URL normalize |
| `browser-engine` | `BrowserEngine` trait + Servo backend |
| `browser-profile` | SQLite + JSON persistence |
| `browser-ui` | Event loop, chrome, controller |
| `browser-app` | Binary entrypoint (`rust-browser`) |

## Security stance (after P0)

- Use engine TLS; do not bypass certificate checks.
- Do not log passwords, cookies, tokens, or auth headers.
- Credentials: OS keyring (or encrypted-file fallback) — not plaintext SQLite.
- Permissions: default deny/Ask; no auto-allow.
- **Current Servo 0.5 integration is NOT process isolated.** See `SECURITY_MODEL.md` and `P0_LIMITATIONS.md`.
