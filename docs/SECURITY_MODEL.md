# Security Model

## After P1.2 — Hardened process boundary (still no OS sandbox)

```
Browser Process = privileged coordinator
 ├── UI / egui / UiSurface
 ├── tabs / session / NetworkRouter policy
 ├── Profiles / Credentials / Permissions / Keychain
 └── ContentProcessManager + fail-closed IPC client
          │ Unix IPC  PROTOCOL_VERSION = 3
          │ generation + sequence + validation
          ▼
Content Process = untrusted renderer
 └── Servo + SoftwareRenderingContext + WebViews
      └── profile/content/ only
```

| Property | Status |
|----------|--------|
| **Process isolation** | **DONE** |
| **Servo isolation** | **DONE** — production Browser does not own Servo |
| **IPC boundary** | **DONE** — versioned, sized, validated, generation-scoped |
| **Crash recovery** | **DONE** — death/hang/backoff/lockout; shutdown blocks respawn |
| **OS sandbox** | **NOT IMPLEMENTED** |
| **Site isolation** | **NOT IMPLEMENTED** |
| **Network isolation** | **NOT IMPLEMENTED** |
| **GPU sandbox** | **NOT IMPLEMENTED** |

Do **not** describe this as a “secure sandbox”, “fully isolated browser”, or “secure against compromised content”. Compromised Content can still abuse its own process until an OS sandbox exists.

## Privilege split

| Asset | Browser | Content |
|-------|---------|---------|
| Password DB / Keychain | Yes | **No** |
| Session / tab truth | Yes | Disposable |
| `profile/content/` | Creates | Engine storage only |
| Proxy policy | Owns | Receives non-secret `SetNetworkRoute` |
| Proxy passwords | Browser only | Not on IPC |

## IPC fail-closed

- Unknown / mismatched `PROTOCOL_VERSION` → reject  
- Oversized messages / frames → reject or drop  
- Invalid frame dimensions / buffer length / overflow → reject  
- Stale `generation` after Content restart → drop  
- Duplicate/stale `sequence` → drop  

## Legacy

`RUST_BROWSER_CONTENT=inprocess` → legacy/debug only + WARNING.

## See also

- `docs/P1.2_BOUNDARY_AUDIT.md`
- `docs/P1.2_LIFECYCLE.md`
- `docs/P1.2_NETWORK_BOUNDARY.md`
- `docs/P1.2_REPORT.md`
