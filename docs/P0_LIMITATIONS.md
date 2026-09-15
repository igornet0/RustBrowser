# P0 Limitations — Honest boundaries

**Status:** current after P0 foundation work  
**Engine:** Servo 0.5.0 embedded in-process

## What P0 does **not** provide

### Process isolation

```
CURRENT
Browser Process
 └── UI + Servo + all WebViews   (one address space)

NOT YET
Browser Process
 ├── Content Process(es)
 ├── Network Process
 └── GPU Process
```

WebView recreate after a soft crash improves **resilience**, not **security**. A compromised or malicious page that escapes Servo still shares the browser process.

### Watchdog vs true hang containment

Watchdog heartbeat + background monitor can detect “no progress” while the main thread is stuck.

Limitations:

- If the **entire process** is frozen (kernel / GPU driver hang), no userland watchdog helps.
- Recovery of a blocked Servo event loop from another thread is **not** a safe Servo API — confirmed hang may force process exit and rely on session checkpoint restore.
- Slow pages ≠ hang. Thresholds are configurable and intentionally conservative.

Do **not** claim: “watchdog = process isolation.”

### Site isolation

All tabs share one `Servo` instance and one profile `config_dir`. There is no per-site renderer process and no app-enforced storage partition beyond profile directories.

### Sandbox

No OS sandbox (Seatbelt / AppContainer / seccomp) wraps content. Privileged APIs must go through PermissionManager prompts; that is policy, not a sandbox.

### VPN

`Route::Vpn` is an integration point. Selecting VPN without a tunnel backend yields **Unavailable**, not a silent Direct fallback.

### Web platform / storage partitioning

Servo capabilities (Service Workers, CHIPS, full storage partitioning, WebRTC, …) are engine-limited. See `STORAGE_ARCHITECTURE.md` and Servo release notes — do not invent fake partitioning.

## What P0 **does** provide

- Explicit tab crash lifecycle + WebView recreate
- Crash-safe session checkpoints + dirty-exit restore prompt
- Network routing decisions on every navigation (Direct / Proxy / Vpn-unavailable)
- Credentials out of plaintext SQLite (OS keyring or encrypted fallback)
- Permission decisions persisted; no auto-allow
- Documented security model and upgrade path to out-of-process content
