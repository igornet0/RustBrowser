# P1 Report — Process Isolation

**Date:** 2026-09-14  
**Status:** P1 landed; **P1.1 completed** dual-path removal on the default architecture. See `docs/P1.1_REPORT.md`.

## Implemented (P1)

| Item | Status |
|------|--------|
| `docs/P1_AUDIT.md` | Done |
| Separate Content Process | Done (`--content-process`) |
| Servo inside Content Process | Done |
| Framed IPC + version + request ids | Done (`browser-ipc`, now v2) |
| ContentProcessManager | Done |
| Heartbeat + hang terminate | Done |
| Browser survives content death | Done |
| Content storage dir (no password DB) | Done |
| Failure tests | Done |

## P1.1 follow-through

| Item | Status |
|------|--------|
| Remove production Browser `ServoEngine` | Done — `PageBackend::Isolated` default |
| CPU frames in egui | Done |
| Input/nav via IPC only (default) | Done |
| Legacy inprocess warning | Done |

## P1.2 hardening

See `docs/P1.2_REPORT.md` — fail-closed IPC v3, generation/sequence, lifecycle, frame benchmarks, keyboard map. **Sandbox still NOT IMPLEMENTED.**

## Still not claimed

1. **Sandbox:** NOT IMPLEMENTED  
2. **Site isolation:** NOT shipped  
3. Smooth FHD frames — JSON IPC too costly (see `P1_BENCHMARK.md`)

## Acceptance

```bash
./target/debug/rust-browser
# log: P1.1 isolated content process attached — Browser owns no Servo
kill -9 <content-pid>
# Browser stays up; content restarts; tabs restore
```
