//! Embedded browser brand icons for the import UI (unified 128×128 PNG).

use egui::{ColorImage, TextureHandle, TextureOptions, Vec2};
use std::collections::HashMap;
use tracing::warn;

const CHROME: &[u8] = include_bytes!("../../../assets/browsers/chrome.png");
const FIREFOX: &[u8] = include_bytes!("../../../assets/browsers/firefox.png");
const EDGE: &[u8] = include_bytes!("../../../assets/browsers/edge.png");
const OPERA: &[u8] = include_bytes!("../../../assets/browsers/opera.png");
const SAFARI: &[u8] = include_bytes!("../../../assets/browsers/safari.png");
const BRAVE: &[u8] = include_bytes!("../../../assets/browsers/brave.png");
const YANDEX: &[u8] = include_bytes!("../../../assets/browsers/yandex.png");

/// Known browser brands shown in import settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserBrand {
    Chrome,
    Firefox,
    Edge,
    Opera,
    Safari,
    Brave,
    Yandex,
    Other,
}

impl BrowserBrand {
    pub const ALL: [Self; 7] = [
        Self::Chrome,
        Self::Firefox,
        Self::Edge,
        Self::Safari,
        Self::Brave,
        Self::Opera,
        Self::Yandex,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Firefox => "Firefox",
            Self::Edge => "Edge",
            Self::Opera => "Opera",
            Self::Safari => "Safari",
            Self::Brave => "Brave",
            Self::Yandex => "Yandex",
            Self::Other => "Other",
        }
    }

    /// Match against a detected browser's display name.
    pub fn from_browser_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.contains("yandex") {
            Self::Yandex
        } else if n.contains("brave") {
            Self::Brave
        } else if n.contains("edge") {
            Self::Edge
        } else if n.contains("opera") {
            Self::Opera
        } else if n.contains("firefox") {
            Self::Firefox
        } else if n.contains("safari") {
            Self::Safari
        } else if n.contains("chrome") {
            Self::Chrome
        } else {
            Self::Other
        }
    }

    fn png(self) -> Option<&'static [u8]> {
        Some(match self {
            Self::Chrome => CHROME,
            Self::Firefox => FIREFOX,
            Self::Edge => EDGE,
            Self::Opera => OPERA,
            Self::Safari => SAFARI,
            Self::Brave => BRAVE,
            Self::Yandex => YANDEX,
            Self::Other => return None,
        })
    }
}

#[derive(Default)]
pub struct BrowserIconCache {
    textures: HashMap<BrowserBrand, TextureHandle>,
}

impl BrowserIconCache {
    pub fn texture(&mut self, ctx: &egui::Context, brand: BrowserBrand) -> Option<&TextureHandle> {
        if !self.textures.contains_key(&brand) {
            if let Some(png) = brand.png() {
                if let Some(tex) = decode_texture(ctx, brand, png) {
                    self.textures.insert(brand, tex);
                }
            }
        }
        self.textures.get(&brand)
    }
}

fn decode_texture(
    ctx: &egui::Context,
    brand: BrowserBrand,
    png: &[u8],
) -> Option<TextureHandle> {
    let img = image::load_from_memory(png)
        .map_err(|err| {
            warn!(?err, brand = ?brand, "browser icon decode failed");
            err
        })
        .ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(
        format!("browser_icon_{brand:?}"),
        color,
        TextureOptions::LINEAR,
    ))
}

/// Draw a brand icon at `size` (falls back to a letter chip).
pub fn paint_brand_icon(
    ui: &mut egui::Ui,
    cache: &mut BrowserIconCache,
    brand: BrowserBrand,
    size: f32,
) {
    let size = Vec2::splat(size);
    if let Some(tex) = cache.texture(ui.ctx(), brand) {
        let id = tex.id();
        ui.add(egui::Image::new((id, size)));
        return;
    }
    // Fallback chip
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let t = crate::theme::current();
    ui.painter()
        .circle_filled(rect.center(), size.x * 0.42, t.surface_hover);
    let letter = brand.label().chars().next().unwrap_or('?');
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter.to_string(),
        egui::FontId::proportional(size.x * 0.45),
        t.text,
    );
}
