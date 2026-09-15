# Build Environment & Engine Research

Research date: 2026-09-14  
Host: `webs.local` (macOS 26.2, Darwin 25.2.0)

## 1. Host platform

| Item | Value |
|------|--------|
| OS | macOS 26.2 (Build 25C56) |
| Kernel | Darwin 25.2.0 |
| Architecture | `aarch64` / `arm64` (Apple M4) |
| Xcode | `/Applications/Xcode.app/Contents/Developer` |

## 2. Rust toolchain

| Tool | Version |
|------|---------|
| rustc | 1.98.0 (`88d9e12ae`, 2026-08-18) |
| cargo | 1.98.0 (`797e8a9bc`, 2026-08-05) |
| rustup | 1.29.0 |
| Active toolchain | `stable-aarch64-apple-darwin` |
| Host triple | `aarch64-apple-darwin` |
| LLVM (rustc) | 22.1.8 |

Servo crate `rust-version`: **1.88.0** (satisfied by 1.98.0).

## 3. System tools & libraries

| Dependency | Status |
|------------|--------|
| git | 2.48.1 |
| clang/LLVM (Homebrew) | 22.1.3 |
| cmake | 4.1.1 |
| pkg-config | 2.5.1 |
| uv | present (`~/.local/bin/uv`) |
| OpenSSL (pkg-config) | available |
| FreeType | 26.5.20 (Homebrew) |
| HarfBuzz | 11.5.0 (Homebrew) |
| OpenGL.framework | present |
| Metal.framework | present |
| GStreamer.framework | **not installed** (optional; needed only for `media-gstreamer`) |

Additional Homebrew packages already present: autoconf, automake, libtool, icu4c, llvm, python@3.12+.

## 4. Browser engine choice

### Decision: **Servo via crates.io** (primary)

Servo is suitable for desktop embedding as of 2026:

- Published on crates.io as `servo` (current: **0.5.0**, MPL-2.0).
- Official embedding API: `ServoBuilder`, `WebView` / `WebViewBuilder`, `RenderingContext`, `EventLoopWaker`, `WebViewDelegate`.
- Official minimal example: `components/servo/examples/winit_minimal.rs` (also shipped as crate example).
- Documentation: https://doc.servo.org/servo/ and https://book.servo.org/embedding/overview.html
- Blog: https://servo.org/blog/2026/04/13/servo-0.1.0-release/

**Gecko/Firefox was not required** — Servo embedding is viable for a desktop app.

Pinned for this project: `servo = "=0.5.0"`.

### Embedding workflow (authoritative API)

1. Install a rustls crypto provider (`aws-lc-rs`).
2. Create a `winit` `EventLoop` with a user event used as `EventLoopWaker`.
3. On resume: create `Window`, then `WindowRenderingContext` (or `OffscreenRenderingContext` when compositing chrome).
4. `ServoBuilder::default().event_loop_waker(...).build()`.
5. `WebViewBuilder::new(&servo, rendering_context).url(...).delegate(...).build()`.
6. On `notify_new_frame_ready` → `request_redraw`; on redraw → `WebView::paint()` + `RenderingContext::present()`.
7. After any `WebView` API call, call `Servo::spin_event_loop()`.

### Navigation API verified on 0.5.0

| Capability | API |
|------------|-----|
| Load URL | `WebView::load(Url)` |
| Reload | `WebView::reload()` |
| Back / Forward | `go_back(n)` / `go_forward(n)`, `can_go_back()` / `can_go_forward()` |
| History notify | `WebViewDelegate::notify_history_changed` |
| URL / title | `notify_url_changed`, `notify_page_title_changed` |
| Load status | `notify_load_status_changed` |
| Input | `notify_input_event` |
| Resize | `WebView::resize(PhysicalSize)` |

## 5. Windowing / UI stack decision

| Option | Fit with Servo |
|--------|----------------|
| **winit** | Required by official embedding example |
| **egui** (+ egui_glow) | Used by servoshell for browser chrome |
| iced | No first-party Servo compositing path |

**Chosen stack:** `winit` 0.30.x + `egui` / `egui_glow` for chrome (toolbar, tabs, address bar), Servo for page content.

Rationale: matches Servo’s own headed shell; stable path for combining native chrome with engine frames.

### Rendering backends

| Backend | Role |
|---------|------|
| `WindowRenderingContext` | Full-window page (minimal / debug) |
| `OffscreenRenderingContext` | Page surface composited under egui chrome (production UI) |
| WebRender (inside Servo) | HTML/CSS paint |
| Surfman | GL/Metal context management |

Default features of `servo` 0.5.0: `baked-in-resources`, `bundled_freetype`, `clipboard`, `js_jit`.

Media (`media-gstreamer`) is **optional** and disabled for MVP to avoid GStreamer install. Video/audio playback is limited until GStreamer is installed.

## 6. Minimal working combination

| Component | Version / choice |
|-----------|------------------|
| Rust | ≥ 1.88 (CI/local: 1.98.0) |
| Servo | `=0.5.0` (crates.io) |
| winit | `0.30.13` (matches Servo example) |
| egui / egui-winit / egui_glow | `0.34` (glow **0.17**, matches Servo) |
| rustls | 0.23 + `aws-lc-rs` crypto provider |
| euclid | 0.22 |
| SQLite | `rusqlite` 0.38 (bundled; aligned with servo-storage) |
| Logging | `tracing` + `tracing-subscriber` (do **not** call `Servo::setup_logging` after installing a global logger) |

## 7. Known engine limitations (as of 0.5.0)

1. **API still pre-1.0** — pin exactly `=0.5.0`; expect breaking changes on monthly releases.
2. **Web compatibility incomplete** vs Chromium/Gecko (WPT gaps).
3. **No dedicated download-manager embedder API** — no Content-Disposition → file-save callback found in public API. Application-level downloads use our own HTTP client; automatic “Save link as…” from engine events is limited.
4. **Per-tab VPN routing** is application-level (`NetworkPolicy`); Servo networking is direct unless proxy prefs/handlers are configured later.
5. **media-gstreamer** requires platform GStreamer packages; not enabled in default MVP build.

## 8. Build & run (this project)

```bash
cd rust-browser
cargo build -p browser-app
cargo run -p browser-app
cargo test --workspace
```

First Servo build is long (full browser engine). Subsequent incremental builds are much faster.

Optional media support:

```bash
# Install GStreamer for macOS from Servo build-deps, then:
cargo build -p browser-app --features media
```

## 9. References

- https://crates.io/crates/servo
- https://doc.servo.org/servo/
- https://book.servo.org/embedding/overview.html
- https://book.servo.org/building/macos.html
- https://github.com/servo/servo/blob/master/components/servo/examples/winit_minimal.rs
- https://servo.org/blog/2026/04/13/servo-0.1.0-release/
