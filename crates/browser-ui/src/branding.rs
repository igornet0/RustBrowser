//! Brand assets: app logo (window/dock) and UI splash icon.

use egui::{ColorImage, TextureHandle, TextureOptions};
use tracing::warn;
use winit::window::Icon;

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo-256.png");
const SPLASH_ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

/// Decode PNG bytes into RGBA8 (width, height, pixels).
fn decode_rgba(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(png)
        .map_err(|err| {
            warn!(?err, "failed to decode brand asset");
            err
        })
        .ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

/// Application / window icon from `logo.png`.
pub fn app_window_icon() -> Option<Icon> {
    let (w, h, rgba) = decode_rgba(LOGO_PNG)?;
    Icon::from_rgba(rgba, w, h)
        .map_err(|err| {
            warn!(?err, "failed to create window icon from logo");
            err
        })
        .ok()
}

/// Splash / loading icon texture from `icon.png` (egui).
pub fn load_splash_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let (w, h, rgba) = decode_rgba(SPLASH_ICON_PNG)?;
    let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture("splash_icon", color, TextureOptions::LINEAR))
}

/// Maximum splash lifetime even if load-complete is never observed.
pub const SPLASH_MAX_SECS: f32 = 4.0;

/// Minimum time the splash stays visible after first paint.
pub const SPLASH_MIN_SECS: f32 = 1.25;
