# Rust Browser

Rust desktop browser powered by Servo.

## Features

- Servo-backed HTML/CSS/JS rendering (no custom engine)
- Address bar and HTTPS navigation
- Back / Forward / Reload
- Multiple tabs with keyboard shortcuts
- Persistent history, bookmarks, settings, and session restore
- Local user profiles with isolated data directories
- Basic download manager
- VPN-ready network policy abstraction (`Direct` today)

## Architecture

```
Browser UI (winit + egui)
    ↓
Browser Controller / Core
    ↓
Browser Engine abstraction
    ↓
Servo 0.5.0
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Requirements

- Rust ≥ 1.88 (tested with 1.98)
- macOS / Linux / Windows (desktop targets supported by Servo)
- Xcode (macOS), cmake, pkg-config
- See [docs/BUILD.md](docs/BUILD.md) for researched host details

## Build

```bash
cd rust-browser
cargo build -p browser-app
```

First build compiles Servo and takes a long time.

## Run

```bash
cargo run -p browser-app
```

Default homepage: `https://example.com`

## Development

```bash
cargo check --workspace
cargo test --workspace
RUST_LOG=info,browser=debug cargo run -p browser-app
```

## Testing

```bash
cargo test --workspace
```

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md).

## Branding

Assets live in `assets/`:

| File | Use |
|------|-----|
| `logo.png` / `logo-256.png` | Application / window icon |
| `icon.png` | Splash icon shown while the first page loads |

Source masters: `../logo.png` and `../icon.png` at the monorepo root.
