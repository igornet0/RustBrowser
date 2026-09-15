# Crash Recovery

## Levels

### A — Tab crash (soft)

```
notify_crashed
  → TabState::Crashed
  → keep TabId / URL / title
  → destroy WebView
  → (optional auto) recreate WebView + navigate
  → TabState::Recovering → Loading → Ready
```

UI: “This tab crashed” + **Reload**. Other tabs stay open. Browser stays open.

### B — Hang (watchdog)

Background thread monitors heartbeat from the UI/engine loop.

```
healthy → no progress → suspected → confirmed hang
  → mark session dirty / write hang note
  → attempt recovery if main thread returns
  → else process exit → startup restore from checkpoint
```

Not process isolation — see `P0_LIMITATIONS.md`.

### C — Browser process crash / kill

```
startup
  → running.lock present? → unclean exit
  → load session checkpoint
  → prompt or auto-restore per settings
```

Minimum restored: tab URLs, order, active tab, window geometry, profile.

## Session files

```
profile/
├── session.json          # committed checkpoint
├── session.json.tmp      # atomic write scratch
└── running.lock          # present while browser runs
```

Writes: encode → write tmp → fsync → rename over `session.json`.

Checkpoints (debounced): tab create/close, navigate, switch tab, periodic timer, clean shutdown.

**Never** store passwords or cookies in session JSON.

## Clean vs dirty shutdown

| Event | running.lock | session |
|-------|--------------|---------|
| Start | create / replace | load prior |
| Clean exit | remove | final checkpoint |
| Crash / kill | left behind | last checkpoint used |
