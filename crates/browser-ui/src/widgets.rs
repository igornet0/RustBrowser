//! Shared chrome widgets built on design tokens.

use crate::icons::{self, Icon};
use crate::theme::{self, radius, size, space};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

pub fn primary_button(ui: &mut Ui, label: &str) -> egui::Response {
    let t = theme::current();
    ui.add(
        egui::Button::new(RichText::new(label).color(t.accent_fg).strong().size(13.0))
            .fill(t.accent)
            .corner_radius(CornerRadius::same(radius::LG as u8))
            .min_size(Vec2::new(0.0, size::ICON_BTN)),
    )
}

pub fn ghost_button(ui: &mut Ui, label: &str) -> egui::Response {
    let t = theme::current();
    ui.add(
        egui::Button::new(RichText::new(label).color(t.text_secondary).size(13.0))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, t.border))
            .corner_radius(CornerRadius::same(radius::LG as u8))
            .min_size(Vec2::new(0.0, size::ICON_BTN)),
    )
}

pub fn section_title(ui: &mut Ui, text: &str) {
    let t = theme::current();
    ui.label(RichText::new(text).size(22.0).strong().color(t.text));
    ui.add_space(space::SM);
}

pub fn caption(ui: &mut Ui, text: &str) {
    let t = theme::current();
    ui.label(RichText::new(text).size(12.0).color(t.text_secondary));
}

/// Compact rounded tab — active elevates, inactive stays quiet.
pub fn tab_chip(
    ui: &mut Ui,
    title: &str,
    active: bool,
    loading: bool,
) -> (egui::Response, bool /* close */) {
    let t = theme::current();
    let max_w = 168.0_f32;
    let label = if title.chars().count() > 18 {
        let s: String = title.chars().take(18).collect();
        format!("{s}…")
    } else {
        title.to_string()
    };

    let desired = Vec2::new(
        (label.len() as f32 * 6.8 + 46.0).clamp(84.0, max_w),
        size::TAB_H,
    );
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    let hovered = response.hovered();

    let fill = if active {
        t.tab_active
    } else if hovered {
        t.surface_hover
    } else {
        t.tab_inactive
    };
    // Soft pill — no hard border except a whisper on hover for inactive.
    let stroke = if active {
        Stroke::NONE
    } else if hovered {
        Stroke::new(1.0_f32, t.border)
    } else {
        Stroke::NONE
    };
    ui.painter()
        .rect(rect, radius::LG, fill, stroke, egui::StrokeKind::Inside);

    // Favicon / loading
    let fav = Rect::from_center_size(
        Pos2::new(rect.left() + 13.0, rect.center().y),
        Vec2::splat(11.0),
    );
    if loading {
        ui.painter()
            .circle_stroke(fav.center(), 4.5, Stroke::new(1.4_f32, t.accent));
    } else {
        icons::paint(ui, Icon::Globe, fav, t.text_tertiary);
    }

    let show_close = hovered || active;
    let close_w = if show_close { 16.0 } else { 0.0 };
    let text_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 24.0, rect.top()),
        Pos2::new(rect.right() - 4.0 - close_w, rect.bottom()),
    );
    ui.painter().text(
        text_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        &label,
        egui::FontId::proportional(12.0),
        if active { t.text } else { t.text_secondary },
    );

    let mut close_clicked = false;
    if show_close {
        let close_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 11.0, rect.center().y),
            Vec2::splat(14.0),
        );
        let close_id = response.id.with("close");
        let close_resp = ui.interact(close_rect, close_id, Sense::click());
        if close_resp.hovered() {
            ui.painter()
                .circle_filled(close_rect.center(), 7.0, t.surface_active);
        }
        let c_color = if close_resp.hovered() {
            t.text
        } else {
            t.text_tertiary
        };
        icons::paint(ui, Icon::Close, close_rect.shrink(2.5), c_color);
        if close_resp.clicked() {
            close_clicked = true;
        }
    }

    (response, close_clicked)
}

/// Soft pill address / search field with lock icon and focus ring.
pub fn address_bar(
    ui: &mut Ui,
    address: &mut String,
    focused_request: &mut bool,
    id_salt: impl std::hash::Hash,
) -> egui::Response {
    let t = theme::current();
    let height = size::ADDR_H;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    let edit_id = egui::Id::new(("address_edit", id_salt));
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let stroke = if focused {
        Stroke::new(1.5_f32, t.accent)
    } else {
        Stroke::new(1.0_f32, t.border)
    };
    ui.painter().rect(
        rect,
        radius::PILL.min(height * 0.5),
        t.address_bg,
        stroke,
        egui::StrokeKind::Inside,
    );

    let lock_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 16.0, rect.center().y),
        Vec2::splat(13.0),
    );
    let secure = address.starts_with("https://");
    icons::paint(
        ui,
        if secure { Icon::Lock } else { Icon::Globe },
        lock_rect,
        if secure { t.success } else { t.text_tertiary },
    );

    let mut edit_rect = rect.shrink2(Vec2::new(30.0, 5.0));
    edit_rect.max.x -= 6.0;

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(edit_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let display = display_url(address, focused);
    let mut edit_buf = if focused {
        address.clone()
    } else {
        display
    };
    let resp = child.add(
        egui::TextEdit::singleline(&mut edit_buf)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .desired_width(edit_rect.width())
            .text_color(t.text)
            .hint_text("Search or enter address"),
    );
    if *focused_request {
        resp.request_focus();
        *focused_request = false;
    }
    if resp.changed() || (focused && edit_buf != *address) {
        *address = edit_buf;
    }
    resp
}

fn display_url(address: &str, focused: bool) -> String {
    if focused {
        return address.to_string();
    }
    let trimmed = address.trim();
    // Keep the toolbar clean on the built-in home page.
    if matches!(
        trimmed,
        "" | "about:newtab" | "about:home" | "about:blank"
    ) {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix("https://") {
        rest.trim_end_matches('/').to_string()
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        rest.trim_end_matches('/').to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn card(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let t = theme::current();
    let fill = if selected { t.accent_muted } else { t.surface_raised };
    let stroke = if selected {
        Stroke::new(1.25_f32, t.accent)
    } else {
        Stroke::new(1.0_f32, t.border)
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(radius::LG as u8))
        .inner_margin(egui::Margin::same(space::MD as i8))
        .show(ui, add);
    // egui Frame only senses hover by default — upgrade so `.clicked()` works.
    ui.interact(inner.response.rect, inner.response.id, Sense::click())
}

/// Visible theme option chip (System / Light / Dark).
pub fn theme_option(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let t = theme::current();
    let desired = Vec2::new(104.0, 44.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    let fill = if selected {
        t.accent
    } else if resp.hovered() {
        t.surface_hover
    } else {
        t.surface_raised
    };
    let stroke = if selected {
        Stroke::new(1.5_f32, t.accent)
    } else {
        Stroke::new(1.0_f32, t.border_strong)
    };
    ui.painter().rect(
        rect,
        radius::LG,
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let text_color = if selected { t.accent_fg } else { t.text };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        text_color,
    );
    resp
}
