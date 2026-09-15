# P0 Completion Report

**Date:** 2026-09-14  
**Status:** P0 foundation landed in-tree. **Not** process-isolated. **Not** a security claim of “secure browser.”

## 1. What was implemented

| Item | Result |
|------|--------|
| TabState lifecycle | `Creating → Loading → Ready → Crashed → Recovering → …` |
| Crash detection | `notify_crashed` → `TabState::Crashed` + overlay + toast |
| WebView recreate | `destroy_view` + `create_view` / `recreate_view`; URL restored |
| Watchdog | Background heartbeat; suspected/confirmed hang; last-resort exit |
| Session checkpoint | Debounced + periodic (5s); atomic tmp→fsync→rename |
| Dirty exit | `running.lock`; restore prompt on unclean startup |
| NetworkRouter | All address-bar navigations routed; Direct/Proxy; VPN→Unavailable |
| Credentials | OS keyring preferred; AES-GCM file fallback; plaintext migration |
| SiteDataManager | Thin Servo wrapper (cookies/cache/storage clear) |
| PermissionManager | SQLite decisions; default Ask/Deny; stored Allow honored |
| ContentProcess seam | `LocalContentProcess` trait stub for P1 |
| Docs | Audit, limitations, security, crash, network, storage, roadmap |

## 2. Remaining security gaps (honest)

- Single process: UI + Servo + all WebViews
- No OS sandbox around content
- No site isolation / CHIPS
- Watchdog cannot recover a fully blocked event loop without process exit
- Permission Ask does not yet show a blocking modal (deny + toast; stored Allow works)
- Encrypted-file credential fallback ≠ Keychain strength
- Proxy is global Servo prefs, not per-tab sockets

## 3. Servo 0.5 limitations

- In-process embedding model
- Storage partitioning / SW / IndexedDB app control: Partial / Not Supported (see `STORAGE_ARCHITECTURE.md`)
- Soft crash via `notify_crashed` only when Servo surfaces Panic to embedder

## 4. Ready for out-of-process

- `ContentProcess` / `LocalContentProcess` seam
- `BrowserEngine` + `NetworkRouter` boundaries
- Session/permission/credential stores are process-agnostic (filesystem/IPC-ready)

## 5. Needs refactor for P1

- `ServoEngine` owns one `Servo` + many `WebView`s — must split per content process
- Delegate/`SharedState` assume UI-thread `Rc`
- Proxy prefs are process-global

## 6–8. Benchmarks / memory / stress

- Unit/integration: tab crash path, 50-tab session checkpoint, 100 create/close, password migration, network/permissions tests — **pass**
- Full GUI memory @ 10/30/50 tabs: **not measured in this run** (requires interactive `cargo run`)
- Recommend next: instrument RSS while opening N tabs manually

## 9. NetworkPolicy results

- Direct → engine Direct
- Proxy without URI → Unavailable (no silent Direct)
- Vpn → Unavailable (no fake tunnel)

## 10. Password migration results

- Legacy plaintext rows migrate into secret backend; SQLite `password` wiped
- Tests: `migrates_legacy_plaintext`, `upsert_and_list_with_memory_backend`

## Next decision

**Do not auto-start VPN.** Choose:

1. **P1 Process Isolation** (recommended next for production architecture), or  
2. **P2 Network Core** (DNS/proxy depth) if isolation research needs more Servo investigation time.

Principle unchanged: resilient → isolated → then VPN/privacy differentiation.
