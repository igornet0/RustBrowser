# P1 / P1.1 / P1.2 Benchmarks

**Date:** 2026-09-14  
**Build:** debug `cargo test` / `cargo build -p browser-app`  
**Rule:** measured numbers only; GUI process CPU/RAM still partly manual.

## IPC Heartbeat

| Metric | Value |
|--------|-------|
| RTT avg | ~43 µs (earlier) / re-check via test |
| n | 50 |

`cargo test -p browser-ipc heartbeat_roundtrip_latency_sample -- --nocapture`

## Frame transport (JSON RGBA over Unix socket) — **measured**

Encoding: `serde_json` of `FrameBuffer.rgba: Vec<u8>` expands ~**4×** vs raw.

| Resolution | Raw bytes/frame | JSON bytes | IPC result | fps (approx) | MB/s (raw) | avg latency | p95 |
|------------|-----------------|------------|------------|--------------|------------|-------------|-----|
| 800×600 | 1 920 000 | ~7.7 MiB | sent | **~3.7** | ~6.8 | ~271 ms | ~304 ms |
| 1280×720 | 3 686 400 | ~14.1 MiB | sent | **~2.1** | ~7.2 | ~486 ms | ~493 ms |
| 1920×1080 | 8 294 400 | ~31.6 MiB | **SKIP** — exceeds `MAX_MESSAGE_SIZE` (16 MiB) | — | — | — | — |

Copies per frame path today: Servo readback → Vec → JSON serialize → socket → JSON deserialize → egui texture upload (**not zero-copy**).

### Conclusion (do not optimize yet — decision input)

Current frame IPC is **not viable for FHD** under the 16 MiB fail-closed limit, and is **too slow** for smooth 60 fps even at 800×600 in debug JSON mode.

**Candidate follow-ups (not implemented in P1.2):** shared memory / mmap ring, dirty rects, compressed frames, binary frame channel.

## GUI / process (manual checklist)

| Metric | Status |
|--------|--------|
| Browser cold start | fill locally |
| Content startup (Hello→Ready) | fill locally |
| First frame / nav→first frame | fill locally (expect dominated by frame IPC above) |
| CPU Browser / Content | fill via Activity Monitor |
| RAM Browser / Content | fill via Activity Monitor |
| Recovery after `kill -9` | integration test passes; wall-clock fill locally |

## How to re-run

```bash
cargo test -p browser-ipc frame_ipc_throughput_sample -- --nocapture
cargo test -p browser-core --test p1_process -- --ignored --nocapture
```
