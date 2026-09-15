# Roadmap

## P0 — Browser Core Foundation

Resilience and security **foundation** before VPN / chrome expansion.

- [x] Audit (`docs/P0_AUDIT.md`)
- [x] TabState lifecycle + crash detection
- [x] WebView recreate on tab crash
- [x] Watchdog / hang detection (in-process limits documented)
- [x] Atomic + periodic session checkpoint
- [x] Dirty-exit crash recovery (`running.lock`)
- [x] NetworkRouter on all navigations (Direct / Proxy; Vpn = unavailable)
- [x] CredentialStore (no plaintext passwords) + migration
- [x] SiteDataManager + PermissionManager abstractions
- [x] Docs: limitations, security, crash, network, storage

## P1 — Process Isolation

- [x] Audit (`docs/P1_AUDIT.md`)
- [x] IPC protocol + version + Unix transport
- [x] Content Process binary entry (`--content-process`)
- [x] Servo in Content Process (`SoftwareRenderingContext`)
- [x] ContentProcessManager (spawn / death / backoff / crash-loop)
- [x] Browser survives content kill (manager + session restore path)
- [x] Heartbeat over IPC
- [x] SECURITY_MODEL updated (Process YES / Sandbox NO / Site Isolation NO)
- [x] Failure tests (`crates/browser-core/tests/p1_process.rs`)

## P1.1 — Complete Servo Content Process Isolation

- [x] Audit (`docs/P1.1_SERVO_AUDIT.md`)
- [x] IPC v2 messages (nav/input/viewport/suspend + load events)
- [x] Default Browser path: `PageBackend::Isolated` (UiSurface + IPC) — **no** production `ServoEngine`
- [x] Frames: Content RGBA → Browser egui texture
- [x] Input / navigation: Browser → IPC → Content → Servo
- [x] Legacy `RUST_BROWSER_CONTENT=inprocess` only (loud warning)
- [x] Docs + protocol/lifecycle regression tests
- [ ] Site isolation (deferred — seam only)
- [ ] OS sandbox (deferred — **NOT IMPLEMENTED**)

## P1.2 — Browser Hardening & Boundary Preparation

- [x] Boundary audit (`docs/P1.2_BOUNDARY_AUDIT.md`)
- [x] IPC fail-closed (limits, frame validation, version reject) — `PROTOCOL_VERSION = 3`
- [x] `generation` + sequence stale rejection
- [x] Content lifecycle state machine (`docs/P1.2_LIFECYCLE.md`)
- [x] Crash recovery edge cases (shutdown blocks respawn; stale pid/socket)
- [x] NamedKey keyboard mapping + tests
- [x] Frame transport benchmark (`docs/P1_BENCHMARK.md`)
- [x] App-level resource limits (OS memory/CPU policy still absent)
- [x] Proxy boundary design only (`docs/P1.2_NETWORK_BOUNDARY.md`)
- [x] Report (`docs/P1.2_REPORT.md`)

## P2 — next decision (do not auto-start)

Choose after P1.2 findings:

- **P2-A** Network Core  
- **P2-B** OS Sandbox  
- **P2-C** Site Isolation  
- **or** frame transport rewrite if rendering bottleneck dominates  

Historical Network Core checklist (if P2-A):

- DNS / DoH / DoT policy
- Proxy hardening
- HTTP/2 / HTTP/3 visibility & tuning
- TLS policy
- Routing observability

**Gate:** P1.2 complete. Prefer frame-transport or sandbox before Network Core if those gaps dominate.

## P3 — VPN

- Real tunnel backend (e.g. WireGuard)
- VPN manager + profiles
- Per-profile routing
- Per-tab routing
- No fake VPN→Direct

## P4 — Privacy Engine

- Tracker / cookie policy
- WebRTC / DNS policy
- Fingerprint protection (consistent profiles, not random noise)

## P5 — Product hardening

- DevTools, extensions, a11y, updater, opt-in crash reports, performance

---

## Legacy note

Earlier v0.2–v0.6 ordering (Privacy → Network → VPN first) is superseded. Differentiating VPN/privacy features land **after** P0–P1.1 foundation.
