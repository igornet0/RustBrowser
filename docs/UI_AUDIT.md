# UI Audit — Rust Browser

**Date:** 2026-09-14  
**Stack:** `winit` 0.30 + `egui` 0.34 / `egui_glow` + Servo 0.5 page compositor  
**Scope:** `crates/browser-ui` (chrome only; engine/profile logic unchanged)

## Current architecture (post-redesign)

| Area | Location | Notes |
|------|----------|--------|
| Event loop / compositing | `app.rs` | Servo blit via PaintCallback — unchanged |
| Chrome orchestrator | `chrome.rs` | Compact tabs + toolbar; **no bottom bar** |
| Design tokens | `theme.rs` | `Tokens`, spacing/radius/size, `apply()` |
| Icons | `icons.rs` | Stroke SVG-style painter + `icon_button` |
| Widgets | `widgets.rs` | Tabs, address bar, buttons, cards |
| Settings | `settings_panel.rs` | Settings app + import modal |
| Command palette | `command_palette.rs` | Cmd/Ctrl+K |
| i18n | `i18n.rs` | String table + EN fallback |
| Controller | `controller.rs` | Additive UI flags only |
| Splash | `splash.rs` | Brand overlay |

## Problems fixed (vs screenshot “before”)

1. ~~Bottom nav bar~~ → toast + menu / library windows / settings
2. ~~Primitive tabs~~ → compact chips with hover close + loading
3. ~~Emoji / text toolbar~~ → stroke icon buttons + tooltips
4. ~~Debug settings~~ → sidebar Settings application
5. ~~Long import form~~ → modal + browser cards
6. ~~Scattered colors~~ → centralized design tokens
7. ~~No palette / VPN / profile chrome~~ → Cmd+K, VPN popover, profile popover
8. ~~Blank new tab~~ → New Tab landing with search + shortcuts

## Design goals

- Dark-first premium chrome; max content area  
- Central Design System  
- Compact tab strip + icon-only nav + focusable address bar  
- Settings as application with category sidebar  
- Import as modal with browser cards + options  
- Cmd/Ctrl+K command palette  
- VPN + Profile toolbar popovers (VPN UI-only)  
- Stroke icons (no emoji as UI icons)

## Non-goals / remaining limits

- No Servo / SQLite / import backend rewrites  
- No real VPN tunnel implementation (UI states only)  
- No drag-and-drop tab reorder (browser API not present)  
- History / Bookmarks / Downloads still floating windows (not full in-content pages)  
- Context menus / rich error pages partially deferred  
- Extensions / Advanced network are placeholders  

## File map

```
docs/UI_AUDIT.md
crates/browser-ui/src/theme.rs
crates/browser-ui/src/icons.rs
crates/browser-ui/src/widgets.rs
crates/browser-ui/src/chrome.rs
crates/browser-ui/src/settings_panel.rs
crates/browser-ui/src/command_palette.rs
crates/browser-ui/src/controller.rs   (UI flags additive)
crates/browser-ui/src/i18n.rs
crates/browser-ui/src/lib.rs
```

## Verification

```bash
cd rust-browser
cargo check -p browser-ui
cargo test -p browser-ui -p browser-profile -p browser-core
cargo build -p browser-app
cargo run -p browser-app
```
