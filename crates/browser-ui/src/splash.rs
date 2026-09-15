//! Splash overlay shown while the browser engine finishes the first page load.

use crate::branding::{self, SPLASH_MAX_SECS, SPLASH_MIN_SECS};
use egui::{Color32, FontId, Order, Pos2, Rect, Sense, Vec2};
use std::time::{Duration, Instant};

/// Target splash animation / timeout polling rate (avoids full display-rate spin).
const SPLASH_FRAME_MS: u64 = 33;

pub struct SplashScreen {
    texture: Option<egui::TextureHandle>,
    started: Instant,
    /// Set when the active tab reports load complete at least once.
    first_page_ready: bool,
    visible: bool,
}

impl SplashScreen {
    pub fn new() -> Self {
        Self {
            texture: None,
            started: Instant::now(),
            first_page_ready: false,
            visible: true,
        }
    }

    pub fn notify_page_ready(&mut self) {
        self.first_page_ready = true;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Draw full-window splash. Returns whether it is still visible.
    pub fn draw(&mut self, ctx: &egui::Context) -> bool {
        if !self.visible {
            return false;
        }

        if self.texture.is_none() {
            self.texture = branding::load_splash_texture(ctx);
        }

        let elapsed = self.started.elapsed().as_secs_f32();
        // Fallback: never leave the user stuck on splash if Complete is missed.
        if elapsed >= SPLASH_MAX_SECS {
            self.first_page_ready = true;
        }
        let can_hide = self.first_page_ready && elapsed >= SPLASH_MIN_SECS;

        let alpha = if can_hide {
            let fade = 1.0 - ((elapsed - SPLASH_MIN_SECS) / 0.35).clamp(0.0, 1.0);
            if fade <= 0.02 {
                self.visible = false;
                ctx.request_repaint();
                return false;
            }
            fade
        } else {
            1.0
        };

        // ~30fps while waiting / fading — not every display refresh.
        ctx.request_repaint_after(Duration::from_millis(SPLASH_FRAME_MS));

        let screen = ctx.viewport_rect();

        // Full-screen interactive area blocks clicks to chrome/page underneath.
        egui::Area::new(egui::Id::new("splash_overlay"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .sense(Sense::click_and_drag())
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let painter = ui.painter();

                let bg = Color32::from_rgba_unmultiplied(8, 6, 14, (255.0 * alpha) as u8);
                painter.rect_filled(screen, 0.0, bg);

                let center = screen.center();
                if let Some(tex) = &self.texture {
                    let size = Vec2::splat(180.0);
                    let rect = Rect::from_center_size(center - Vec2::new(0.0, 28.0), size);
                    let tint =
                        Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * alpha) as u8);
                    painter.image(
                        tex.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                }

                let title_color =
                    Color32::from_rgba_unmultiplied(240, 235, 255, (255.0 * alpha) as u8);
                let subtitle_color =
                    Color32::from_rgba_unmultiplied(180, 170, 210, (220.0 * alpha) as u8);

                painter.text(
                    center + Vec2::new(0.0, 92.0),
                    egui::Align2::CENTER_CENTER,
                    "Rust Browser",
                    FontId::proportional(22.0),
                    title_color,
                );
                painter.text(
                    center + Vec2::new(0.0, 118.0),
                    egui::Align2::CENTER_CENTER,
                    "Loading…",
                    FontId::proportional(14.0),
                    subtitle_color,
                );
            });

        true
    }
}

impl Default for SplashScreen {
    fn default() -> Self {
        Self::new()
    }
}
