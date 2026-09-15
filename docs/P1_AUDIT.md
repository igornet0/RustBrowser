# P1 Audit — Process Isolation Feasibility (Servo 0.5.0)

**Date:** 2026-09-14  
**Engine:** `servo = "=0.5.0"` (crates.io)  
**Rule:** nothing invented — claims below are from registry sources + rust-browser embedding.

## Verdict

| Goal | Feasibility |
|------|-------------|
| Separate OS **Content Process** owning full `Servo` + `WebView` | **Feasible** (custom spawn; not Servo’s built-in multiprocess) |
| Browser UI survives `kill <content-pid>` | **Feasible** if UI never holds `Servo` |
| Live page pixels in egui at GL quality via `render_to_parent` | **Not feasible** across processes (same-process FBO blit only) |
| Live page pixels via CPU readback (`SoftwareRenderingContext` / `read_to_image`) | **Feasible** (degraded FPS/bandwidth) |
| Enable Servo `opts.multiprocess` alone | **Wrong boundary** — script/layout children; Paint/WebRender stay in embedder |

**P1 choice:** custom Content Process + Unix IPC + `SoftwareRenderingContext` frames (or placeholders). No fake sandbox / site isolation claims.

## 1. Send / Sync / thread affinity

| Type | Evidence | Implication |
|------|----------|-------------|
| `Servo` | `pub struct Servo(Rc<ServoInner>)` — `servo-0.5.0/servo.rs` | `!Send` — cannot cross threads/processes as a handle |
| `WebView` | `Rc<RefCell<WebViewInner>>` — `webview.rs` | Same |
| Delegates | `Rc<dyn WebViewDelegate>` | Must live in Content Process; proxy events over IPC |
| `EventLoopWaker` | `Send + Sync` — `servo-embedder-traits` | Wake may come from other threads **inside** the Servo process; `spin_event_loop` stays on Servo’s thread |

## 2. Window-less Servo

| Context | Needs OS window? |
|---------|------------------|
| `WindowRenderingContext` | Yes |
| `OffscreenRenderingContext` | Indirectly yes (parent window context) |
| `SoftwareRenderingContext::new(size)` | **No** — CPU path; `read_to_image` |

Servo tests construct Servo with only `SoftwareRenderingContext` (`servo-0.5.0/tests/common/mod.rs`).

## 3. Current rust-browser embedding (pre-P1)

```
ONE PROCESS
 └── winit + egui
      └── ServoEngine (UI thread, Rc)
           ├── WindowRenderingContext
           ├── OffscreenRenderingContext
           └── WebViews
      └── egui PaintCallback → render_to_parent (same-process GL)
```

## 4. Frame export

| API | Cross-process? |
|-----|----------------|
| `RenderingContext::read_to_image` | Yes (RGBA bytes) |
| `WebView::take_screenshot` | Yes (same-process callback → forward) |
| `OffscreenRenderingContext::render_to_parent_callback` | **No** — FBO blit in one GL context |
| Official DMA-BUF / IOSurface embedder API | **Not found** in Servo 0.5 public surface |

## 5. Servo built-in multiprocess

- `Opts::multiprocess`, `run_content_process(token)` re-exec child for **script/layout**
- Parent still owns Paint / WebRender / `WebView` API
- **Does not** equal “Servo in Content Process, UI in Browser Process”

## 6. Hard blockers vs workable path

**Blockers for GL OOP compositing:** `Rc` Servo, `render_to_parent` same-process, no public GPU frame IPC.

**Workable P1 path:**

```
Browser Process (winit + egui + core + profile)
      │  Unix IPC (framed, versioned)
      ▼
Content Process
      └── Servo + SoftwareRenderingContext + WebViews
           └── events + throttled RGBA frames
```

## 7. Explicit non-goals for P1

- OS sandbox (Seatbelt / AppContainer / seccomp)
- Site isolation (multiple content processes per site)
- Network / GPU processes
- Fake “secure” claims beyond process isolation

## 8. Acceptance criterion

```bash
kill <content-process-pid>
```

Expected: Browser Process stays alive → detects disconnect → respawns Content Process → restores tabs from Browser-owned session state.

## 9. P1.1 note

Default Browser path no longer owns Servo (see `docs/P1.1_SERVO_AUDIT.md` / `docs/P1.1_REPORT.md`). This audit’s “UI never holds Servo” criterion is satisfied by `PageBackend::Isolated`.

