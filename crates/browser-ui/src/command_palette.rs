//! Cmd/Ctrl+K command palette.

use crate::controller::BrowserController;
use crate::icons::{self, Icon};
use crate::i18n;
use crate::theme::{self, radius, space};
use browser_engine::EngineViewId;
use crate::page_backend::PageBackend;
use egui::{CornerRadius, Key, RichText};

#[derive(Clone, Copy)]
struct Cmd {
    id: &'static str,
    icon: Icon,
    label_key: &'static str,
}

const COMMANDS: &[Cmd] = &[
    Cmd { id: "back", icon: Icon::Back, label_key: "cmd_back" },
    Cmd { id: "forward", icon: Icon::Forward, label_key: "cmd_forward" },
    Cmd { id: "reload", icon: Icon::Reload, label_key: "cmd_reload" },
    Cmd { id: "new_tab", icon: Icon::Plus, label_key: "cmd_new_tab" },
    Cmd { id: "close_tab", icon: Icon::Close, label_key: "cmd_close_tab" },
    Cmd { id: "bookmark", icon: Icon::Star, label_key: "cmd_bookmark" },
    Cmd { id: "settings", icon: Icon::Settings, label_key: "cmd_settings" },
    Cmd { id: "history", icon: Icon::History, label_key: "cmd_history" },
    Cmd { id: "bookmarks", icon: Icon::Bookmark, label_key: "cmd_bookmarks" },
    Cmd { id: "downloads", icon: Icon::Download, label_key: "cmd_downloads" },
    Cmd { id: "vpn", icon: Icon::Shield, label_key: "cmd_vpn" },
    Cmd { id: "focus_address", icon: Icon::Search, label_key: "cmd_address" },
];

pub fn draw(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    if !controller.show_command_palette {
        return;
    }

    let lang = controller.store.settings.language;
    let t = theme::current();
    let screen = ctx.content_rect();

    // Dim backdrop
    egui::Area::new(egui::Id::new("cmd_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .sense(egui::Sense::click())
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, t.overlay);
            if ui.interact(screen, ui.id().with("bg"), egui::Sense::click()).clicked() {
                controller.show_command_palette = false;
                controller.command_query.clear();
            }
        });

    let width = 480.0_f32.min(screen.width() - 40.0);
    let pos = egui::pos2(screen.center().x - width * 0.5, screen.top() + 80.0);

    egui::Window::new("commands")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_pos(pos)
        .fixed_size([width, 0.0])
        .frame(
            egui::Frame::new()
                .fill(t.surface_raised)
                .corner_radius(CornerRadius::same(radius::LG as u8))
                .stroke(egui::Stroke::new(1.0_f32, t.border))
                .inner_margin(egui::Margin::same(space::MD as i8))
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: t.shadow,
                }),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let icon_r = egui::Rect::from_center_size(
                    ui.cursor().left_center() + egui::vec2(8.0, 10.0),
                    egui::vec2(16.0, 16.0),
                );
                icons::paint(ui, Icon::Search, icon_r, t.text_tertiary);
                ui.add_space(22.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut controller.command_query)
                        .hint_text(i18n::t(lang, "cmd_hint"))
                        .desired_width(ui.available_width())
                        .frame(egui::Frame::NONE),
                );
                resp.request_focus();
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    controller.show_command_palette = false;
                    controller.command_query.clear();
                }
            });
            ui.add_space(space::SM);
            ui.separator();
            ui.add_space(space::SM);

            let q = controller.command_query.to_ascii_lowercase();
            let mut ran = None::<&'static str>;
            for cmd in COMMANDS {
                let label = i18n::t(lang, cmd.label_key);
                if !q.is_empty()
                    && !label.to_ascii_lowercase().contains(&q)
                    && !cmd.id.contains(q.as_str())
                {
                    continue;
                }
                let row = ui.add(
                    egui::Button::new(RichText::new(format!("   {label}")).color(t.text))
                        .fill(t.surface_hover)
                        .corner_radius(CornerRadius::same(radius::MD as u8))
                        .min_size(egui::vec2(ui.available_width(), 34.0)),
                );
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(row.rect.left() + 16.0, row.rect.center().y),
                    egui::vec2(14.0, 14.0),
                );
                icons::paint(ui, cmd.icon, icon_rect, t.text_secondary);
                if row.clicked() || (row.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                    ran = Some(cmd.id);
                }
                ui.add_space(2.0);
            }

            if let Some(id) = ran {
                run_command(id, controller, engine);
                controller.show_command_palette = false;
                controller.command_query.clear();
            }
        });
}

fn run_command(id: &str, controller: &mut BrowserController, engine: &mut PageBackend) {
    match id {
        "back" => {
            let id = controller.browser.active_tab;
            let _ = engine.go_back(EngineViewId(id));
        }
        "forward" => {
            let id = controller.browser.active_tab;
            let _ = engine.go_forward(EngineViewId(id));
        }
        "reload" => {
            let id = controller.browser.active_tab;
            let _ = engine.reload(EngineViewId(id));
        }
        "new_tab" => controller.new_tab(engine),
        "close_tab" => controller.close_active_tab(engine),
        "bookmark" => controller.bookmark_active(),
        "settings" => {
            controller.open_settings(engine);
        }
        "history" => {
            controller.show_history = true;
        }
        "bookmarks" => {
            controller.show_bookmarks = true;
        }
        "downloads" => {
            controller.show_downloads = true;
        }
        "vpn" => controller.show_vpn_popover = !controller.show_vpn_popover,
        "focus_address" => controller.focus_address = true,
        _ => {}
    }
}
