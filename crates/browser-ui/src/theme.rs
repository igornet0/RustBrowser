//! Design tokens + egui Visuals application.

use browser_profile::{ColorScheme, Theme as ThemeMode};
use egui::{Color32, CornerRadius, FontId, Margin, Stroke, Visuals};

/// Spacing scale (4px grid).
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

pub mod radius {
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const PILL: f32 = 999.0;
}

pub mod size {
    pub const ICON: f32 = 15.0;
    pub const ICON_BTN: f32 = 28.0;
    pub const TAB_H: f32 = 28.0;
    pub const TOOLBAR_H: f32 = 40.0;
    pub const ADDR_H: f32 = 32.0;
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_raised: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub accent: Color32,
    pub accent_muted: Color32,
    pub accent_fg: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub address_bg: Color32,
    pub tab_active: Color32,
    pub tab_inactive: Color32,
    pub overlay: Color32,
    pub shadow: Color32,
}

impl Tokens {
    pub fn resolve(mode: &ThemeMode, scheme: ColorScheme) -> Self {
        let dark = matches!(mode, ThemeMode::Dark | ThemeMode::System);
        let accent = accent_for(scheme, dark);
        if dark {
            Self {
                bg: Color32::from_rgb(18, 18, 20),
                surface: Color32::from_rgb(28, 28, 32),
                surface_raised: Color32::from_rgb(36, 36, 42),
                surface_hover: Color32::from_rgb(44, 44, 52),
                surface_active: Color32::from_rgb(52, 52, 62),
                border: Color32::from_rgba_unmultiplied(255, 255, 255, 10),
                border_strong: Color32::from_rgba_unmultiplied(255, 255, 255, 22),
                text: Color32::from_rgb(242, 242, 247),
                text_secondary: Color32::from_rgb(158, 158, 170),
                text_tertiary: Color32::from_rgb(112, 112, 124),
                accent,
                accent_muted: Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 36),
                accent_fg: Color32::WHITE,
                danger: Color32::from_rgb(255, 105, 105),
                success: Color32::from_rgb(70, 200, 140),
                warning: Color32::from_rgb(255, 186, 90),
                address_bg: Color32::from_rgb(22, 22, 26),
                tab_active: Color32::from_rgb(40, 40, 48),
                tab_inactive: Color32::TRANSPARENT,
                overlay: Color32::from_rgba_unmultiplied(0, 0, 0, 140),
                shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 80),
            }
        } else {
            Self {
                bg: Color32::from_rgb(236, 236, 240),
                surface: Color32::from_rgb(250, 250, 252),
                surface_raised: Color32::from_rgb(255, 255, 255),
                surface_hover: Color32::from_rgb(240, 240, 244),
                surface_active: Color32::from_rgb(230, 230, 236),
                border: Color32::from_rgb(226, 226, 232),
                border_strong: Color32::from_rgb(206, 206, 216),
                text: Color32::from_rgb(28, 28, 34),
                text_secondary: Color32::from_rgb(110, 110, 124),
                text_tertiary: Color32::from_rgb(150, 150, 162),
                accent,
                accent_muted: Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 24),
                accent_fg: Color32::WHITE,
                danger: Color32::from_rgb(210, 55, 55),
                success: Color32::from_rgb(28, 160, 100),
                warning: Color32::from_rgb(200, 130, 20),
                address_bg: Color32::from_rgb(244, 244, 248),
                tab_active: Color32::from_rgb(255, 255, 255),
                tab_inactive: Color32::TRANSPARENT,
                overlay: Color32::from_rgba_unmultiplied(20, 22, 30, 90),
                shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 28),
            }
        }
    }
}

fn accent_for(scheme: ColorScheme, dark: bool) -> Color32 {
    match (scheme, dark) {
        (ColorScheme::Violet, true) => Color32::from_rgb(124, 108, 255),
        (ColorScheme::Violet, false) => Color32::from_rgb(92, 76, 220),
        (ColorScheme::Ocean, true) => Color32::from_rgb(56, 168, 255),
        (ColorScheme::Ocean, false) => Color32::from_rgb(20, 120, 220),
        (ColorScheme::Forest, true) => Color32::from_rgb(64, 200, 140),
        (ColorScheme::Forest, false) => Color32::from_rgb(28, 150, 100),
        (ColorScheme::Sunset, true) => Color32::from_rgb(255, 138, 90),
        (ColorScheme::Sunset, false) => Color32::from_rgb(220, 96, 56),
        (ColorScheme::Midnight, true) => Color32::from_rgb(100, 140, 255),
        (ColorScheme::Midnight, false) => Color32::from_rgb(48, 80, 200),
        (ColorScheme::Sand, true) => Color32::from_rgb(220, 180, 120),
        (ColorScheme::Sand, false) => Color32::from_rgb(168, 120, 64),
    }
}

pub fn scheme_swatch(scheme: ColorScheme) -> Color32 {
    accent_for(scheme, true)
}

thread_local! {
    static TOKENS: std::cell::RefCell<Tokens> =
        std::cell::RefCell::new(Tokens::resolve(&ThemeMode::Dark, ColorScheme::Ocean));
    static APPLIED: std::cell::RefCell<Option<(ThemeMode, ColorScheme)>> =
        const { std::cell::RefCell::new(None) };
}

pub fn current() -> Tokens {
    TOKENS.with(|t| *t.borrow())
}

/// Force the next `apply` call to rebuild visuals (after changing theme in settings).
pub fn invalidate_applied() {
    APPLIED.with(|cell| *cell.borrow_mut() = None);
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode, scheme: ColorScheme) {
    let skip = APPLIED.with(|cell| cell.borrow().as_ref() == Some(&(mode.clone(), scheme)));
    if skip {
        return;
    }
    APPLIED.with(|cell| *cell.borrow_mut() = Some((mode.clone(), scheme)));

    let t = Tokens::resolve(&mode, scheme);
    TOKENS.with(|cell| *cell.borrow_mut() = t);

    let dark = matches!(mode, ThemeMode::Dark | ThemeMode::System);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    visuals.window_fill = t.surface_raised;
    visuals.panel_fill = t.surface;
    visuals.extreme_bg_color = t.address_bg;
    visuals.faint_bg_color = t.surface_hover;
    visuals.window_stroke = Stroke::new(1.0_f32, t.border);
    visuals.widgets.noninteractive.bg_fill = t.surface;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text_secondary);
    visuals.widgets.inactive.bg_fill = t.surface_hover;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    visuals.widgets.hovered.bg_fill = t.surface_active;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t.text);
    visuals.widgets.active.bg_fill = t.accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, t.accent_fg);
    visuals.widgets.open.bg_fill = t.surface_active;
    visuals.selection.bg_fill = t.accent_muted;
    visuals.selection.stroke = Stroke::new(1.0_f32, t.accent);
    visuals.hyperlink_color = t.accent;
    visuals.warn_fg_color = t.warning;
    visuals.error_fg_color = t.danger;
    visuals.override_text_color = Some(t.text);
    visuals.window_corner_radius = CornerRadius::same(radius::LG as u8);
    visuals.menu_corner_radius = CornerRadius::same(radius::MD as u8);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: t.shadow,
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: t.shadow,
    };

    ctx.set_visuals(visuals);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(space::SM, space::SM);
    style.spacing.button_padding = egui::vec2(space::MD, space::SM);
    style.spacing.window_margin = Margin::same(space::LG as i8);
    style.interaction.show_tooltips_only_when_still = true;
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::proportional(13.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::proportional(13.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::proportional(18.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        FontId::proportional(11.5),
    );
    ctx.set_global_style(style);
}

pub fn chrome_frame() -> egui::Frame {
    let t = current();
    egui::Frame::new()
        .fill(t.surface)
        .inner_margin(Margin::symmetric(10, 4))
}

pub fn tab_strip_frame() -> egui::Frame {
    let t = current();
    egui::Frame::new()
        .fill(t.surface)
        .inner_margin(Margin::symmetric(8, 4))
}

pub fn rounding_md() -> CornerRadius {
    CornerRadius::same(radius::MD as u8)
}

pub fn rounding_lg() -> CornerRadius {
    CornerRadius::same(radius::LG as u8)
}

/// Color scheme swatch for settings.
pub fn color_swatch(
    ui: &mut egui::Ui,
    scheme: ColorScheme,
    selected: bool,
) -> egui::Response {
    let color = scheme_swatch(scheme);
    let size = egui::vec2(28.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().circle_filled(rect.center(), 12.0, color);
    if selected {
        ui.painter().circle_stroke(
            rect.center(),
            13.5,
            Stroke::new(2.0_f32, current().text),
        );
    }
    response
}
