//! Compact modern browser chrome — tabs + toolbar only (no bottom bar).

use crate::command_palette;
use crate::controller::{BrowserController, SettingsTab, VpnUiState};
use crate::icons::{self, Icon};
use crate::i18n;
use crate::settings_panel;
use crate::theme::{self, radius, size, space};
use crate::widgets;
use browser_engine::EngineViewId;
use crate::page_backend::PageBackend;
use browser_profile::DownloadState;
use egui::{
    Align2, Color32, CornerRadius, Key, LayerId, PaintCallback, Panel, Pos2, Rect,
    RichText, Sense, Stroke,
};
use egui_glow::CallbackFn;
use std::sync::Arc;
use winit::dpi::PhysicalSize;

pub struct ChromeOutput {
    /// Left edge of the page content area in egui points.
    pub content_left_points: f32,
    /// Top edge of the page content area in egui points.
    pub content_top_points: f32,
    /// Width / height of the page content area in egui points.
    pub content_width_points: f32,
    pub content_height_points: f32,
    /// `pixels_per_point` used when measuring the content rect — must match paint.
    pub pixels_per_point: f32,
    pub request_repaint: bool,
}

pub fn draw_chrome(
    ui: &mut egui::Ui,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) -> ChromeOutput {
    let mut request_repaint = false;
    let ctx = ui.ctx().clone();
    let lang = controller.store.settings.language;
    theme::apply(
        &ctx,
        controller.store.settings.theme.clone(),
        controller.store.settings.color_scheme,
    );
    handle_shortcuts(&ctx, controller, engine);

    let t = theme::current();

    // ── Tab strip ──────────────────────────────────────────────
    Panel::top("tabs")
        .exact_size(size::TAB_H + 8.0)
        .frame(theme::tab_strip_frame())
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let tabs: Vec<_> = controller
                    .browser
                    .tabs
                    .iter()
                    .map(|tab| {
                        let title = if crate::controller::is_settings_url(tab.url.as_ref()) {
                            i18n::t(lang, "settings").to_string()
                        } else if tab.title.is_empty() {
                            i18n::t(lang, "new_tab").to_string()
                        } else {
                            tab.title.clone()
                        };
                        (
                            tab.id,
                            title,
                            tab.id == controller.browser.active_tab,
                            tab.loading(),
                            matches!(tab.state, browser_core::TabState::Crashed { .. }),
                        )
                    })
                    .collect();

                for (id, title, active, loading, crashed) in tabs {
                    let chip_title = if crashed {
                        format!("⚠ {title}")
                    } else {
                        title
                    };
                    let (resp, close) = widgets::tab_chip(ui, &chip_title, active, loading);
                    if close {
                        let _ = controller.browser.switch_tab(id);
                        controller.close_active_tab(engine);
                    } else if resp.clicked() {
                        controller.switch_tab(engine, id);
                    } else if resp.middle_clicked() {
                        let _ = controller.browser.switch_tab(id);
                        controller.close_active_tab(engine);
                    }
                }

                if icons::icon_button(ui, Icon::Plus, i18n::t(lang, "new_tab"), true, false)
                    .clicked()
                {
                    controller.new_tab(engine);
                }
            });
        });

    // Hairline between tabs and toolbar
    {
        let t = theme::current();
        let y = ui.cursor().top();
        let w = ui.max_rect();
        ui.painter().hline(
            w.x_range(),
            y,
            Stroke::new(1.0_f32, t.border),
        );
    }

    // ── Toolbar ────────────────────────────────────────────────
    Panel::top("toolbar")
        .exact_size(size::TOOLBAR_H)
        .frame(theme::chrome_frame())
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let active = controller.browser.active().ok();
                let can_back = active.as_ref().map(|t| t.can_go_back).unwrap_or(false);
                let can_fwd = active.as_ref().map(|t| t.can_go_forward).unwrap_or(false);
                let loading = active.as_ref().map(|t| t.loading()).unwrap_or(false);

                if icons::icon_button(ui, Icon::Back, "Back", can_back, false).clicked() {
                    let id = controller.browser.active_tab;
                    let _ = engine.go_back(EngineViewId(id));
                }
                if icons::icon_button(ui, Icon::Forward, "Forward", can_fwd, false).clicked() {
                    let id = controller.browser.active_tab;
                    let _ = engine.go_forward(EngineViewId(id));
                }
                let reload_icon = if loading { Icon::Stop } else { Icon::Reload };
                if icons::icon_button(ui, reload_icon, i18n::t(lang, "reload"), true, false)
                    .clicked()
                {
                    let id = controller.browser.active_tab;
                    let crashed = controller
                        .browser
                        .tab(id)
                        .ok()
                        .map(|t| t.state.is_crashed())
                        .unwrap_or(false);
                    if crashed {
                        controller.recover_tab(engine, id);
                    } else {
                        let _ = engine.reload(EngineViewId(id));
                    }
                }

                ui.add_space(space::SM);

                // Address bar takes remaining width minus trailing actions
                let trailing = size::ICON_BTN * 5.0 + 16.0;
                let addr_w = (ui.available_width() - trailing).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(addr_w, size::ADDR_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let resp = widgets::address_bar(
                            ui,
                            &mut controller.address,
                            &mut controller.focus_address,
                            "toolbar",
                        );
                        if resp.changed() {
                            controller.address_dirty = true;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            controller.navigate_address(engine);
                        }
                        if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            controller.navigate_address(engine);
                        }
                    },
                );

                ui.add_space(space::XS);

                if icons::icon_button(ui, Icon::Star, i18n::t(lang, "bookmark"), true, false)
                    .clicked()
                {
                    controller.bookmark_active();
                }

                let vpn_active = !matches!(controller.vpn_state, VpnUiState::Off);
                let vpn_icon = match controller.vpn_state {
                    VpnUiState::Off | VpnUiState::Error => Icon::ShieldOff,
                    _ => Icon::Shield,
                };
                let vpn_tip = match controller.vpn_state {
                    VpnUiState::Off => "VPN · Off",
                    VpnUiState::Connecting => "VPN · Connecting",
                    VpnUiState::On => "VPN · Protected",
                    VpnUiState::Error => "VPN · Error",
                    VpnUiState::Auto => "VPN · Auto",
                };
                if icons::icon_button(ui, vpn_icon, vpn_tip, true, vpn_active).clicked() {
                    controller.show_vpn_popover = !controller.show_vpn_popover;
                    controller.show_profile_popover = false;
                    controller.show_main_menu = false;
                }

                if icons::icon_button(ui, Icon::Download, i18n::t(lang, "downloads"), true, false)
                    .clicked()
                {
                    controller.show_downloads = !controller.show_downloads;
                }

                if icons::icon_button(
                    ui,
                    Icon::User,
                    i18n::t(lang, "profiles"),
                    true,
                    controller.show_profile_popover,
                )
                .clicked()
                {
                    controller.show_profile_popover = !controller.show_profile_popover;
                    controller.show_vpn_popover = false;
                    controller.show_main_menu = false;
                }

                if icons::icon_button(
                    ui,
                    Icon::Menu,
                    "Menu",
                    true,
                    controller.show_main_menu,
                )
                .clicked()
                {
                    controller.show_main_menu = !controller.show_main_menu;
                    controller.show_vpn_popover = false;
                    controller.show_profile_popover = false;
                }
            });
        });

    // Popovers (anchored near top-right)
    draw_vpn_popover(&ctx, controller, engine);
    draw_profile_popover(&ctx, controller, engine);
    draw_main_menu(&ctx, controller, engine);

    // Content area
    let content = ui.available_rect_before_wrap();
    let is_new_tab = is_new_tab_page(controller);
    let show_settings = controller.settings_tab_active();

    if show_settings {
        settings_panel::draw_settings_page(ui, content, controller, engine);
    } else if is_new_tab {
        ui.allocate_rect(content, Sense::click());
        draw_new_tab(ui, content, controller, engine);
    } else {
        // Leave the page rect non-interactive so egui does not steal clicks/scroll.
        let scale = ctx.pixels_per_point().clamp(0.5, 4.0);
        let page_w = (content.width() * scale).round().clamp(1.0, 8192.0) as u32;
        let page_h = (content.height() * scale).round().clamp(1.0, 8192.0) as u32;
        let new_size = PhysicalSize::new(page_w, page_h);
        if engine.page_size() != new_size {
            let _ = engine.resize(new_size);
        }
        let _ = engine.paint_webview();

        if let Some(render_to_parent) = engine.render_to_parent_callback() {
            let rect = content;
            ctx.layer_painter(LayerId::background()).add(PaintCallback {
                rect,
                callback: Arc::new(CallbackFn::new(move |info, painter| {
                    let clip = info.viewport_in_pixels();
                    let rect_in_parent = euclid::default::Rect::new(
                        euclid::point2(clip.left_px, clip.from_bottom_px),
                        euclid::size2(clip.width_px.max(0), clip.height_px.max(0)),
                    );
                    render_to_parent(painter.gl(), rect_in_parent);
                })),
            });
        } else {
            // P1.1 isolated: CPU frames from Content Process.
            let tab = controller.browser.active_tab;
            engine.paint_page_egui(ui, content, tab);
        }
        ui.advance_cursor_after_rect(content);
    }

    // Crash overlay for active tab
    if let Ok(tab) = controller.browser.active() {
        if let browser_core::TabState::Crashed { reason } = &tab.state {
            let reason = reason.clone();
            let id = tab.id;
            egui::Area::new(egui::Id::new("tab_crash_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(content.min)
                .show(&ctx, |ui| {
                    ui.set_max_size(content.size());
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(12, 14, 20, 220))
                        .show(ui, |ui| {
                            ui.set_min_size(content.size());
                            ui.vertical_centered(|ui| {
                                ui.add_space(content.height() * 0.28);
                                ui.label(
                                    RichText::new("This tab crashed")
                                        .size(22.0)
                                        .strong()
                                        .color(Color32::from_rgb(240, 240, 245)),
                                );
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(&reason)
                                        .size(13.0)
                                        .color(Color32::from_rgb(160, 165, 180)),
                                );
                                ui.add_space(16.0);
                                if widgets::primary_button(ui, "Reload").clicked() {
                                    controller.recover_tab(engine, id);
                                    request_repaint = true;
                                }
                            });
                        });
                });
        }
    }

    // Exact content rect — do not clamp with a fake minimum top; that skews hit-testing.
    let content_left_points = content.min.x;
    let content_top_points = content.min.y;
    let content_width_points = content.width().max(1.0);
    let content_height_points = content.height().max(1.0);
    let pixels_per_point = ctx.pixels_per_point().clamp(0.5, 4.0);

    // Overlays
    if controller.show_restore_prompt {
        egui::Window::new(i18n::t(lang, "restore_prompt"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(&ctx, |ui| {
                ui.set_min_width(320.0);
                ui.label(i18n::t(lang, "restore_prompt"));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, i18n::t(lang, "restore_yes")).clicked() {
                        controller.accept_restore(engine);
                        request_repaint = true;
                    }
                    if widgets::ghost_button(ui, i18n::t(lang, "restore_no")).clicked() {
                        controller.decline_restore();
                    }
                });
            });
    }

    draw_library_windows(&ctx, controller, engine);
    settings_panel::draw_import_overlay(&ctx, controller, engine);
    command_palette::draw(&ctx, controller, engine);

    // Soft status toast — auto-hides, dismissible with ✕
    if controller.tick_status() {
        request_repaint = true;
    }
    if !controller.status.is_empty() {
        egui::Area::new(egui::Id::new("status_toast"))
            .anchor(Align2::LEFT_BOTTOM, [12.0, -12.0])
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(t.surface_raised)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(CornerRadius::same(radius::MD as u8))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: t.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&controller.status)
                                    .size(12.0)
                                    .color(t.text_secondary),
                            );
                            ui.add_space(space::SM);
                            let close = icons::icon_button(
                                ui,
                                Icon::Close,
                                "Dismiss",
                                true,
                                false,
                            );
                            if close.clicked() {
                                controller.dismiss_status();
                            }
                        });
                    });
            });
        // Keep waking the loop so auto-dismiss fires without user input.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    ChromeOutput {
        content_left_points,
        content_top_points,
        content_width_points,
        content_height_points,
        pixels_per_point,
        request_repaint,
    }
}

fn is_new_tab_page(controller: &BrowserController) -> bool {
    if controller.settings_tab_active() {
        return false;
    }
    let Ok(tab) = controller.browser.active() else {
        return true;
    };
    crate::controller::is_new_tab_url(tab.url.as_ref())
}

fn draw_new_tab(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let t = theme::current();
    let lang = controller.store.settings.language;
    let ctx = ui.ctx().clone();
    controller.ensure_home_background_texture(&ctx);

    ui.painter().rect_filled(rect, 0.0, t.bg);
    if let Some(tex) = controller.home_bg_texture.clone() {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_unmultiplied(255, 255, 255, 200),
        );
        ui.painter().rect_filled(
            rect,
            0.0,
            Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), 140),
        );
    } else {
        ui.painter().rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + rect.height() * 0.45)),
            0.0,
            Color32::from_rgba_unmultiplied(t.surface.r(), t.surface.g(), t.surface.b(), 90),
        );
    }

    let mut page = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home_page")
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    page.set_clip_rect(rect);
    page.add_space((rect.height() * 0.08).clamp(24.0, 72.0));
    page.label(
        RichText::new("Rust Browser")
            .size(28.0)
            .strong()
            .color(t.text),
    );
    page.add_space(6.0);
    page.label(
        RichText::new(i18n::t(lang, "new_tab_subtitle"))
            .size(13.0)
            .color(t.text_secondary),
    );
    page.add_space(space::LG);

    let search_w = 480.0_f32.min(rect.width() - 64.0);
    page.allocate_ui_with_layout(
        egui::vec2(search_w, size::ADDR_H + 6.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let resp = widgets::address_bar(
                ui,
                &mut controller.home_search,
                &mut controller.focus_home_search,
                "home_search",
            );
            let submit = (resp.lost_focus() || resp.has_focus())
                && ui.input(|i| i.key_pressed(Key::Enter));
            if submit && !controller.home_search.trim().is_empty() {
                controller.address = controller.home_search.clone();
                controller.home_search.clear();
                controller.navigate_address(engine);
            }
        },
    );

    page.add_space(space::XL);

    let content_w = (rect.width() - 80.0).clamp(320.0, 860.0);
    page.allocate_ui_with_layout(
        egui::vec2(content_w, rect.height() * 0.55),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            let col_w = (content_w - space::LG) * 0.5;

            // Recently closed tabs
            ui.allocate_ui_with_layout(
                egui::vec2(col_w, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(col_w);
                    ui.label(
                        RichText::new(i18n::t(lang, "recent_tabs"))
                            .strong()
                            .size(14.0)
                            .color(t.text),
                    );
                    ui.add_space(space::SM);
                    let recent = controller.browser.recent_closed_tabs(8);
                    if recent.is_empty() {
                        ui.label(
                            RichText::new(i18n::t(lang, "recent_tabs_empty"))
                                .size(12.0)
                                .color(t.text_tertiary),
                        );
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("home_recent_tabs")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                for (idx, closed) in recent.into_iter().enumerate() {
                                    let title = if closed.title.trim().is_empty() {
                                        closed
                                            .url
                                            .as_ref()
                                            .map(|u| u.to_string())
                                            .unwrap_or_else(|| i18n::t(lang, "new_tab").to_string())
                                    } else {
                                        closed.title.clone()
                                    };
                                    ui.push_id(("recent", idx), |ui| {
                                        if home_link_row(
                                            ui,
                                            &title,
                                            closed
                                                .url
                                                .as_ref()
                                                .map(|u| u.as_str())
                                                .unwrap_or(""),
                                        )
                                        .clicked()
                                        {
                                            if let Some(url) = closed.url {
                                                controller.address = url.to_string();
                                                controller.navigate_address(engine);
                                            }
                                        }
                                    });
                                    ui.add_space(4.0);
                                }
                            });
                    }
                },
            );

            ui.add_space(space::LG);

            // Bookmarks
            ui.allocate_ui_with_layout(
                egui::vec2(col_w, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(col_w);
                    ui.label(
                        RichText::new(i18n::t(lang, "bookmarks"))
                            .strong()
                            .size(14.0)
                            .color(t.text),
                    );
                    ui.add_space(space::SM);
                    controller.ensure_bookmarks_cache();
                    let bookmarks = controller.bookmarks_cache.clone().unwrap_or_default();
                    if bookmarks.is_empty() {
                        ui.label(
                            RichText::new(i18n::t(lang, "bookmarks_empty"))
                                .size(12.0)
                                .color(t.text_tertiary),
                        );
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("home_bookmarks")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                for bm in bookmarks.into_iter().take(12) {
                                    ui.push_id(bm.id, |ui| {
                                        if home_link_row(ui, &bm.title, bm.url.as_str()).clicked() {
                                            controller.address = bm.url.to_string();
                                            controller.navigate_address(engine);
                                        }
                                    });
                                    ui.add_space(4.0);
                                }
                            });
                    }
                },
            );
        },
    );
}

fn home_link_row(ui: &mut egui::Ui, title: &str, subtitle: &str) -> egui::Response {
    let t = theme::current();
    let desired = egui::vec2(ui.available_width(), 40.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    let fill = if resp.hovered() {
        t.surface_hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, radius::MD, fill);
    ui.painter().text(
        Pos2::new(rect.left() + 10.0, rect.center().y - 7.0),
        Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        t.text,
    );
    if !subtitle.is_empty() {
        ui.painter().text(
            Pos2::new(rect.left() + 10.0, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            subtitle,
            egui::FontId::proportional(11.0),
            t.text_tertiary,
        );
    }
    resp
}

fn draw_vpn_popover(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    if !controller.show_vpn_popover {
        return;
    }
    let t = theme::current();
    egui::Window::new("vpn_pop")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::RIGHT_TOP, [-12.0, 84.0])
        .fixed_size([260.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(t.surface_raised)
                .stroke(Stroke::new(1.0_f32, t.border))
                .corner_radius(CornerRadius::same(radius::LG as u8))
                .inner_margin(egui::Margin::same(space::LG as i8))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: t.shadow,
                }),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                icons::paint(
                    ui,
                    Icon::Shield,
                    egui::Rect::from_min_size(ui.cursor().min, egui::vec2(16.0, 16.0)),
                    t.accent,
                );
                ui.add_space(20.0);
                ui.label(RichText::new("Privacy").strong());
            });
            ui.add_space(space::MD);
            ui.label(RichText::new("Connection").size(11.0).color(t.text_tertiary));
            let status = match controller.vpn_state {
                VpnUiState::Off => ("● Disconnected", t.text_secondary),
                VpnUiState::Connecting => ("● Connecting…", t.warning),
                VpnUiState::On => ("● Protected", t.success),
                VpnUiState::Error => ("● Error", t.danger),
                VpnUiState::Auto => ("● Auto", t.accent),
            };
            ui.label(RichText::new(status.0).color(status.1));
            ui.add_space(space::SM);
            ui.label(RichText::new("Location").size(11.0).color(t.text_tertiary));
            ui.label("Auto · Best available");
            ui.add_space(space::MD);
            ui.horizontal(|ui| {
                if widgets::primary_button(
                    ui,
                    match controller.vpn_state {
                        VpnUiState::On | VpnUiState::Auto => "Disconnect",
                        VpnUiState::Connecting => "Cancel",
                        _ => "Connect",
                    },
                )
                .clicked()
                {
                    controller.vpn_state = match controller.vpn_state {
                        VpnUiState::Off | VpnUiState::Error => VpnUiState::On,
                        VpnUiState::Connecting => VpnUiState::Off,
                        VpnUiState::On | VpnUiState::Auto => VpnUiState::Off,
                    };
                }
                if widgets::ghost_button(ui, "Settings").clicked() {
                    controller.show_vpn_popover = false;
                    controller.settings_tab = SettingsTab::Vpn;
                    controller.open_settings(engine);
                }
            });
            ui.add_space(space::SM);
            ui.label(
                RichText::new("UI preview — VPN tunnel not connected yet.")
                    .size(11.0)
                    .color(t.text_tertiary),
            );
        });
}

fn draw_profile_popover(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    if !controller.show_profile_popover {
        return;
    }
    let t = theme::current();
    egui::Window::new("profile_pop")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::RIGHT_TOP, [-12.0, 84.0])
        .fixed_size([260.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(t.surface_raised)
                .stroke(Stroke::new(1.0_f32, t.border))
                .corner_radius(CornerRadius::same(radius::LG as u8))
                .inner_margin(egui::Margin::same(space::LG as i8))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: t.shadow,
                }),
        )
        .show(ctx, |ui| {
            let active_id = controller.profiles.active().ok().map(|p| p.id);
            let list: Vec<_> = controller
                .profiles
                .list()
                .iter()
                .map(|p| (p.id, p.name.clone()))
                .collect();
            for (id, name) in list {
                let is_active = Some(id) == active_id;
                let label = if is_active {
                    format!("✓  {name}")
                } else {
                    format!("    {name}")
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(label).color(if is_active {
                            t.accent
                        } else {
                            t.text
                        }))
                        .fill(if is_active {
                            t.accent_muted
                        } else {
                            Color32::TRANSPARENT
                        })
                        .corner_radius(CornerRadius::same(radius::MD as u8))
                        .min_size(egui::vec2(ui.available_width(), 32.0)),
                    )
                    .clicked()
                {
                    let _ = controller.profiles.switch(id);
                    controller.push_status(format!("Switched profile (restart may be needed)"));
                }
            }
            ui.add_space(space::SM);
            ui.separator();
            ui.add_space(space::SM);
            if ui.button("+ New profile").clicked() {
                controller.settings_tab = SettingsTab::Profiles;
                controller.show_profile_popover = false;
                controller.open_settings(engine);
            }
            if ui.button("Manage profiles").clicked() {
                controller.settings_tab = SettingsTab::Profiles;
                controller.show_profile_popover = false;
                controller.open_settings(engine);
            }
        });
}

fn draw_main_menu(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    if !controller.show_main_menu {
        return;
    }
    let t = theme::current();
    let lang = controller.store.settings.language;
    egui::Window::new("main_menu")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::RIGHT_TOP, [-12.0, 84.0])
        .fixed_size([220.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(t.surface_raised)
                .stroke(Stroke::new(1.0_f32, t.border))
                .corner_radius(CornerRadius::same(radius::LG as u8))
                .inner_margin(egui::Margin::same(space::SM as i8))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: t.shadow,
                }),
        )
        .show(ctx, |ui| {
            if menu_item_click(ui, i18n::t(lang, "settings")) {
                controller.show_main_menu = false;
                controller.open_settings(engine);
            }
            if menu_item_click(ui, i18n::t(lang, "history")) {
                controller.show_history = true;
                controller.show_main_menu = false;
            }
            if menu_item_click(ui, i18n::t(lang, "bookmarks")) {
                controller.show_bookmarks = true;
                controller.show_main_menu = false;
            }
            if menu_item_click(ui, i18n::t(lang, "downloads")) {
                controller.show_downloads = true;
                controller.show_main_menu = false;
            }
            if menu_item_click(ui, i18n::t(lang, "import")) {
                controller.show_main_menu = false;
                controller.settings_tab = SettingsTab::Import;
                controller.show_import_modal = true;
                controller.open_settings(engine);
            }
            ui.separator();
            if menu_item_click(ui, i18n::t(lang, "cmd_hint")) {
                controller.show_command_palette = true;
                controller.show_main_menu = false;
            }
        });
}

fn menu_item_click(ui: &mut egui::Ui, label: &str) -> bool {
    let t = theme::current();
    ui.add(
        egui::Button::new(RichText::new(label).color(t.text))
            .fill(Color32::TRANSPARENT)
            .corner_radius(CornerRadius::same(radius::SM as u8))
            .min_size(egui::vec2(ui.available_width(), 30.0)),
    )
    .clicked()
}

fn draw_library_windows(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let lang = controller.store.settings.language;
    let t = theme::current();

    let mut show_history = controller.show_history;
    if show_history {
        controller.ensure_history_cache();
        egui::Window::new(i18n::t(lang, "history"))
            .open(&mut show_history)
            .default_size([520.0, 420.0])
            .show(ctx, |ui| {
                if widgets::ghost_button(ui, i18n::t(lang, "clear_all")).clicked() {
                    let _ = controller.store.history.clear_all();
                    controller.history_cache = None;
                }
                ui.add_space(space::SM);
                let entries = controller.history_cache.clone().unwrap_or_default();
                egui::ScrollArea::vertical().id_salt("history_list").show(ui, |ui| {
                    for entry in entries {
                        ui.horizontal(|ui| {
                            if ui.link(RichText::new(&entry.title).color(t.accent)).clicked() {
                                controller.address = entry.url.to_string();
                                controller.navigate_address(engine);
                                controller.show_history = false;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::icon_button(
                                        ui,
                                        Icon::Close,
                                        "Remove",
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        let _ = controller.store.history.delete(entry.id);
                                        controller.history_cache = None;
                                    }
                                },
                            );
                        });
                        ui.label(
                            RichText::new(entry.url.as_str())
                                .size(11.0)
                                .color(t.text_tertiary),
                        );
                        ui.add_space(4.0);
                    }
                });
            });
    } else if controller.history_cache.is_some() {
        controller.history_cache = None;
    }
    controller.show_history = show_history;

    let mut show_bookmarks = controller.show_bookmarks;
    if show_bookmarks {
        controller.ensure_bookmarks_cache();
        egui::Window::new(i18n::t(lang, "bookmarks"))
            .open(&mut show_bookmarks)
            .default_size([480.0, 400.0])
            .show(ctx, |ui| {
                let items = controller.bookmarks_cache.clone().unwrap_or_default();
                egui::ScrollArea::vertical().id_salt("bookmarks_list").show(ui, |ui| {
                    for bm in items {
                        ui.horizontal(|ui| {
                            if ui.link(&bm.title).clicked() {
                                controller.address = bm.url.to_string();
                                controller.navigate_address(engine);
                                controller.show_bookmarks = false;
                            }
                            if icons::icon_button(ui, Icon::Close, "Remove", true, false)
                                .clicked()
                            {
                                let _ = controller.store.bookmarks.remove(bm.id);
                                controller.bookmarks_cache = None;
                            }
                        });
                    }
                });
            });
    } else if controller.bookmarks_cache.is_some() {
        controller.bookmarks_cache = None;
    }
    controller.show_bookmarks = show_bookmarks;

    let mut show_downloads = controller.show_downloads;
    if show_downloads {
        controller.ensure_downloads_cache();
        egui::Window::new(i18n::t(lang, "downloads"))
            .open(&mut show_downloads)
            .default_size([480.0, 360.0])
            .show(ctx, |ui| {
                let items = controller.downloads_cache.clone().unwrap_or_default();
                for d in items {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} — {:?} ({:.0}%)",
                            d.filename,
                            d.state,
                            d.progress * 100.0
                        ));
                        if matches!(
                            d.state,
                            DownloadState::Queued | DownloadState::Downloading
                        ) && icons::icon_button(ui, Icon::Close, "Cancel", true, false)
                            .clicked()
                        {
                            let _ = controller.store.downloads.cancel(d.id);
                            controller.downloads_cache = None;
                        }
                    });
                    ui.label(
                        RichText::new(d.destination.display().to_string())
                            .size(11.0)
                            .color(t.text_tertiary),
                    );
                    ui.separator();
                }
            });
    } else if controller.downloads_cache.is_some() {
        controller.downloads_cache = None;
    }
    controller.show_downloads = show_downloads;
}

fn handle_shortcuts(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let input = ctx.input(|i| {
        (
            i.modifiers.command || i.modifiers.ctrl,
            i.modifiers.shift,
            i.modifiers.alt,
            i.key_pressed(Key::T),
            i.key_pressed(Key::W),
            i.key_pressed(Key::L),
            i.key_pressed(Key::R),
            i.key_pressed(Key::K),
            i.key_pressed(Key::Comma),
            i.key_pressed(Key::Tab),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::N),
            i.key_pressed(Key::D),
            i.key_pressed(Key::F),
            i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus),
            i.key_pressed(Key::Minus),
            i.key_pressed(Key::Num0),
            i.key_pressed(Key::ArrowLeft),
            i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::F5),
            i.key_pressed(Key::F6),
        )
    });
    let (
        meta,
        shift,
        alt,
        t,
        w,
        l,
        r,
        k,
        comma,
        tab,
        esc,
        n,
        d,
        f,
        zoom_in,
        zoom_out,
        zoom_reset,
        arrow_left,
        arrow_right,
        f5,
        f6,
    ) = input;

    if esc {
        if controller.settings_tab_active() {
            controller.close_settings_tab(engine);
            controller.show_import_modal = false;
        }
        controller.show_command_palette = false;
        controller.show_vpn_popover = false;
        controller.show_profile_popover = false;
        controller.show_main_menu = false;
    }

    // Reload / focus address without ⌘
    if f5 {
        let id = controller.browser.active_tab;
        let _ = engine.reload(EngineViewId(id));
    }
    if f6 {
        controller.focus_address = true;
    }
    // Alt+← / Alt+→ history navigation (common browser binding)
    if alt && !meta && arrow_left {
        let id = controller.browser.active_tab;
        let _ = engine.go_back(EngineViewId(id));
    }
    if alt && !meta && arrow_right {
        let id = controller.browser.active_tab;
        let _ = engine.go_forward(EngineViewId(id));
    }

    if !meta {
        return;
    }

    if k {
        controller.show_command_palette = !controller.show_command_palette;
        return;
    }
    if t && shift {
        controller.restore_closed(engine);
    } else if t || (n && !shift) {
        controller.new_tab(engine);
    }
    if w {
        controller.close_active_tab(engine);
    }
    if l {
        controller.focus_address = true;
    }
    if r {
        let id = controller.browser.active_tab;
        let _ = engine.reload(EngineViewId(id));
    }
    if comma {
        controller.open_settings(engine);
    }
    if d {
        controller.bookmark_active();
    }
    if f {
        controller.focus_address = true;
    }
    if zoom_in {
        let z = (controller.store.settings.default_zoom + 0.1).clamp(0.5, 3.0);
        controller.store.settings.default_zoom = z;
        engine.set_page_zoom(z);
        let _ = controller.store.save_settings();
    }
    if zoom_out {
        let z = (controller.store.settings.default_zoom - 0.1).clamp(0.5, 3.0);
        controller.store.settings.default_zoom = z;
        engine.set_page_zoom(z);
        let _ = controller.store.save_settings();
    }
    if zoom_reset {
        controller.store.settings.default_zoom = 1.0;
        engine.set_page_zoom(1.0);
        let _ = controller.store.save_settings();
    }
    if arrow_left {
        let id = controller.browser.active_tab;
        let _ = engine.go_back(EngineViewId(id));
    }
    if arrow_right {
        let id = controller.browser.active_tab;
        let _ = engine.go_forward(EngineViewId(id));
    }
    if tab {
        if shift {
            controller.browser.previous_tab();
        } else {
            controller.browser.next_tab();
        }
        let id = controller.browser.active_tab;
        let _ = engine.focus_view(EngineViewId(id));
        controller.sync_address_from_active();
        controller.show_settings = controller.settings_tab_active();
    }
}
