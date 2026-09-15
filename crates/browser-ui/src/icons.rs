//! Stroke-based icons (no emoji). All icons share the same visual weight.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Back,
    Forward,
    Reload,
    Stop,
    Lock,
    Unlock,
    Star,
    StarFilled,
    Download,
    Shield,
    ShieldOff,
    User,
    Menu,
    Close,
    Plus,
    Search,
    Settings,
    History,
    Bookmark,
    Extension,
    Globe,
    Check,
    ChevronRight,
    ChevronDown,
    External,
    Copy,
    Trash,
    Folder,
    Info,
    Warning,
    Home,
    Palette,
    Network,
    Privacy,
    Advanced,
    Import,
    Password,
    Profile,
}

/// Paint a 16×16-ish icon centered in `rect`.
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5_f32, color);
    let p = ui.painter_at(rect);
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.42;

    match icon {
        Icon::Back => {
            let a = Pos2::new(c.x + s * 0.35, c.y - s * 0.7);
            let b = Pos2::new(c.x - s * 0.45, c.y);
            let d = Pos2::new(c.x + s * 0.35, c.y + s * 0.7);
            p.line_segment([a, b], stroke);
            p.line_segment([b, d], stroke);
        }
        Icon::Forward => {
            let a = Pos2::new(c.x - s * 0.35, c.y - s * 0.7);
            let b = Pos2::new(c.x + s * 0.45, c.y);
            let d = Pos2::new(c.x - s * 0.35, c.y + s * 0.7);
            p.line_segment([a, b], stroke);
            p.line_segment([b, d], stroke);
        }
        Icon::Reload => {
            p.circle_stroke(c, s * 0.75, stroke);
            // arrow tip
            let tip = Pos2::new(c.x + s * 0.75, c.y - s * 0.35);
            p.line_segment([Pos2::new(c.x + s * 0.25, c.y - s * 0.55), tip], stroke);
            p.line_segment([Pos2::new(c.x + s * 0.85, c.y + s * 0.05), tip], stroke);
        }
        Icon::Stop => {
            let r = Rect::from_center_size(c, Vec2::splat(s * 1.1));
            p.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Middle);
        }
        Icon::Lock | Icon::Unlock => {
            p.rect_stroke(
                Rect::from_center_size(Pos2::new(c.x, c.y + s * 0.25), Vec2::new(s * 1.2, s * 0.95)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.circle_stroke(Pos2::new(c.x, c.y - s * 0.35), s * 0.45, stroke);
            if matches!(icon, Icon::Unlock) {
                // open shackle gap — already looks open enough with stroke
            }
        }
        Icon::Star | Icon::StarFilled => {
            let pts = star_points(c, s);
            if matches!(icon, Icon::StarFilled) {
                p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            } else {
                for i in 0..pts.len() {
                    p.line_segment([pts[i], pts[(i + 1) % pts.len()]], stroke);
                }
            }
        }
        Icon::Download => {
            p.line_segment(
                [Pos2::new(c.x, c.y - s * 0.7), Pos2::new(c.x, c.y + s * 0.35)],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.45, c.y - s * 0.05),
                    Pos2::new(c.x, c.y + s * 0.4),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + s * 0.45, c.y - s * 0.05),
                    Pos2::new(c.x, c.y + s * 0.4),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.7, c.y + s * 0.7),
                    Pos2::new(c.x + s * 0.7, c.y + s * 0.7),
                ],
                stroke,
            );
        }
        Icon::Shield | Icon::ShieldOff => {
            let top = Pos2::new(c.x, c.y - s * 0.85);
            let left = Pos2::new(c.x - s * 0.7, c.y - s * 0.35);
            let bottom = Pos2::new(c.x, c.y + s * 0.9);
            let right = Pos2::new(c.x + s * 0.7, c.y - s * 0.35);
            p.line_segment([top, right], stroke);
            p.line_segment([right, bottom], stroke);
            p.line_segment([bottom, left], stroke);
            p.line_segment([left, top], stroke);
            if matches!(icon, Icon::ShieldOff) {
                p.line_segment(
                    [
                        Pos2::new(c.x - s * 0.55, c.y + s * 0.55),
                        Pos2::new(c.x + s * 0.55, c.y - s * 0.55),
                    ],
                    stroke,
                );
            }
        }
        Icon::User | Icon::Profile => {
            p.circle_stroke(Pos2::new(c.x, c.y - s * 0.35), s * 0.4, stroke);
            p.circle_stroke(Pos2::new(c.x, c.y + s * 0.75), s * 0.85, stroke);
        }
        Icon::Menu => {
            for dy in [-0.55, 0.0, 0.55] {
                p.line_segment(
                    [
                        Pos2::new(c.x - s * 0.7, c.y + s * dy as f32),
                        Pos2::new(c.x + s * 0.7, c.y + s * dy as f32),
                    ],
                    stroke,
                );
            }
        }
        Icon::Close => {
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.55, c.y - s * 0.55),
                    Pos2::new(c.x + s * 0.55, c.y + s * 0.55),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + s * 0.55, c.y - s * 0.55),
                    Pos2::new(c.x - s * 0.55, c.y + s * 0.55),
                ],
                stroke,
            );
        }
        Icon::Plus => {
            p.line_segment(
                [Pos2::new(c.x - s * 0.6, c.y), Pos2::new(c.x + s * 0.6, c.y)],
                stroke,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y - s * 0.6), Pos2::new(c.x, c.y + s * 0.6)],
                stroke,
            );
        }
        Icon::Search => {
            p.circle_stroke(Pos2::new(c.x - s * 0.15, c.y - s * 0.15), s * 0.55, stroke);
            p.line_segment(
                [
                    Pos2::new(c.x + s * 0.25, c.y + s * 0.25),
                    Pos2::new(c.x + s * 0.7, c.y + s * 0.7),
                ],
                stroke,
            );
        }
        Icon::Settings => {
            p.circle_stroke(c, s * 0.35, stroke);
            p.circle_stroke(c, s * 0.8, stroke);
        }
        Icon::History => {
            p.circle_stroke(c, s * 0.8, stroke);
            p.line_segment([c, Pos2::new(c.x, c.y - s * 0.45)], stroke);
            p.line_segment([c, Pos2::new(c.x + s * 0.4, c.y + s * 0.15)], stroke);
        }
        Icon::Bookmark => {
            let top = c.y - s * 0.85;
            let pts = [
                Pos2::new(c.x - s * 0.55, top),
                Pos2::new(c.x + s * 0.55, top),
                Pos2::new(c.x + s * 0.55, c.y + s * 0.85),
                Pos2::new(c.x, c.y + s * 0.35),
                Pos2::new(c.x - s * 0.55, c.y + s * 0.85),
            ];
            for i in 0..pts.len() {
                p.line_segment([pts[i], pts[(i + 1) % pts.len()]], stroke);
            }
        }
        Icon::Globe | Icon::Home => {
            p.circle_stroke(c, s * 0.8, stroke);
            p.line_segment(
                [Pos2::new(c.x - s * 0.8, c.y), Pos2::new(c.x + s * 0.8, c.y)],
                stroke,
            );
            p.add(egui::Shape::ellipse_stroke(
                c,
                Vec2::new(s * 0.4, s * 0.8),
                stroke,
            ));
        }
        Icon::Check => {
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.55, c.y),
                    Pos2::new(c.x - s * 0.1, c.y + s * 0.5),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.1, c.y + s * 0.5),
                    Pos2::new(c.x + s * 0.6, c.y - s * 0.5),
                ],
                stroke,
            );
        }
        Icon::ChevronRight => {
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.25, c.y - s * 0.55),
                    Pos2::new(c.x + s * 0.35, c.y),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + s * 0.35, c.y),
                    Pos2::new(c.x - s * 0.25, c.y + s * 0.55),
                ],
                stroke,
            );
        }
        Icon::ChevronDown => {
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.55, c.y - s * 0.2),
                    Pos2::new(c.x, c.y + s * 0.4),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x, c.y + s * 0.4),
                    Pos2::new(c.x + s * 0.55, c.y - s * 0.2),
                ],
                stroke,
            );
        }
        Icon::External => {
            p.rect_stroke(
                Rect::from_center_size(c + Vec2::new(-s * 0.15, s * 0.15), Vec2::splat(s * 1.0)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.1, c.y + s * 0.1),
                    Pos2::new(c.x + s * 0.65, c.y - s * 0.65),
                ],
                stroke,
            );
        }
        Icon::Copy => {
            p.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(c.x - s * 0.55, c.y - s * 0.35),
                    Vec2::new(s * 0.95, s * 1.1),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(c.x - s * 0.25, c.y - s * 0.65),
                    Vec2::new(s * 0.95, s * 1.1),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Trash => {
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.55, c.y - s * 0.35),
                    Pos2::new(c.x + s * 0.55, c.y - s * 0.35),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - s * 0.25, c.y - s * 0.55),
                    Pos2::new(c.x + s * 0.25, c.y - s * 0.55),
                ],
                stroke,
            );
            p.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(c.x - s * 0.45, c.y - s * 0.25),
                    Vec2::new(s * 0.9, s * 1.05),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Folder | Icon::Import => {
            p.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(c.x - s * 0.7, c.y - s * 0.25),
                    Vec2::new(s * 1.4, s * 1.0),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(c.x - s * 0.7, c.y - s * 0.55),
                    Vec2::new(s * 0.65, s * 0.35),
                ),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Info => {
            p.circle_stroke(c, s * 0.8, stroke);
            p.circle_filled(Pos2::new(c.x, c.y - s * 0.35), 1.4, color);
            p.line_segment(
                [Pos2::new(c.x, c.y - s * 0.05), Pos2::new(c.x, c.y + s * 0.45)],
                stroke,
            );
        }
        Icon::Warning => {
            let pts = [
                Pos2::new(c.x, c.y - s * 0.85),
                Pos2::new(c.x + s * 0.8, c.y + s * 0.7),
                Pos2::new(c.x - s * 0.8, c.y + s * 0.7),
            ];
            p.line_segment([pts[0], pts[1]], stroke);
            p.line_segment([pts[1], pts[2]], stroke);
            p.line_segment([pts[2], pts[0]], stroke);
            p.circle_filled(Pos2::new(c.x, c.y + s * 0.35), 1.4, color);
            p.line_segment(
                [Pos2::new(c.x, c.y - s * 0.25), Pos2::new(c.x, c.y + s * 0.1)],
                stroke,
            );
        }
        Icon::Extension => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(s * 1.1)),
                3.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.circle_filled(Pos2::new(c.x, c.y - s * 0.85), 2.0, color);
            p.circle_filled(Pos2::new(c.x + s * 0.85, c.y), 2.0, color);
        }
        Icon::Palette => {
            p.circle_stroke(c, s * 0.8, stroke);
            for (dx, dy) in [(-0.35, -0.2), (0.0, -0.4), (0.35, -0.15), (-0.2, 0.3)] {
                p.circle_filled(Pos2::new(c.x + s * dx, c.y + s * dy), 1.8, color);
            }
        }
        Icon::Network => {
            p.circle_stroke(c, s * 0.35, stroke);
            p.circle_stroke(c, s * 0.65, stroke);
            p.circle_stroke(c, s * 0.95, stroke);
        }
        Icon::Privacy => {
            paint(ui, Icon::Shield, rect, color);
        }
        Icon::Advanced => {
            paint(ui, Icon::Settings, rect, color);
        }
        Icon::Password => {
            p.circle_stroke(Pos2::new(c.x - s * 0.35, c.y), s * 0.4, stroke);
            p.line_segment(
                [
                    Pos2::new(c.x, c.y),
                    Pos2::new(c.x + s * 0.7, c.y),
                ],
                stroke,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + s * 0.45, c.y),
                    Pos2::new(c.x + s * 0.45, c.y + s * 0.35),
                ],
                stroke,
            );
        }
    }
}

fn star_points(c: Pos2, s: f32) -> Vec<Pos2> {
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let ang = std::f32::consts::FRAC_PI_2 * -1.0 + i as f32 * std::f32::consts::PI / 5.0;
        let r = if i % 2 == 0 { s } else { s * 0.45 };
        pts.push(Pos2::new(c.x + r * ang.cos(), c.y + r * ang.sin()));
    }
    pts
}

/// Icon-only button with tooltip.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let t = crate::theme::current();
    let size = Vec2::splat(crate::theme::size::ICON_BTN);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let bg = if !enabled {
        Color32::TRANSPARENT
    } else if active {
        t.accent_muted
    } else if hovered {
        t.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    // Circular hover — softer than square chips.
    ui.painter().circle_filled(rect.center(), size.x * 0.42, bg);
    let color = if !enabled {
        t.text_tertiary
    } else if active {
        t.accent
    } else if hovered {
        t.text
    } else {
        t.text_secondary
    };
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(crate::theme::size::ICON));
    paint(ui, icon, icon_rect, color);
    if enabled {
        response = response.on_hover_text(tooltip);
    }
    if !enabled {
        response = response.on_disabled_hover_text(tooltip);
    }
    response
}
