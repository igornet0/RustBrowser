//! Settings application drawn inside the browser content area.

use crate::controller::{BrowserController, SettingsTab};
use crate::icons::{self, Icon};
use crate::i18n;
use crate::theme::{self, radius, space};
use crate::widgets;
use crate::page_backend::PageBackend;
use browser_profile::{ColorScheme, LocalHostEntry, StartupBehavior, Theme, UiLanguage};
use egui::{Align2, CornerRadius, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};

const SIDEBAR_W: f32 = 220.0;

/// Full-bleed settings UI inside the browser viewport (not a floating window).
pub fn draw_settings_page(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let lang = controller.store.settings.language;
    let t = theme::current();

    ui.painter().rect_filled(rect, 0.0, t.bg);

    // One isolated Ui for the whole page — avoids sibling `new_child("child")`
    // collisions that produced "Second use of widget ID".
    let mut page = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("settings_root")
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    page.set_clip_rect(rect);

    // ── Sidebar ──
    let sidebar_rect = Rect::from_min_size(rect.min, Vec2::new(SIDEBAR_W, rect.height()));
    page.painter().rect_filled(sidebar_rect, 0.0, t.surface);
    page.allocate_ui_with_layout(
        Vec2::new(SIDEBAR_W, rect.height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(SIDEBAR_W);
            ui.add_space(space::LG);
            ui.horizontal(|ui| {
                ui.add_space(space::MD);
                let icon_r = Rect::from_center_size(
                    Pos2::new(ui.cursor().left() + 8.0, ui.cursor().top() + 8.0),
                    Vec2::splat(16.0),
                );
                icons::paint(ui, Icon::Settings, icon_r, t.text_secondary);
                ui.add_space(22.0);
                ui.label(RichText::new(i18n::t(lang, "settings")).strong().size(14.0));
            });
            ui.add_space(space::MD);

            ScrollArea::vertical()
                .id_salt("settings_nav")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.set_width(SIDEBAR_W - 4.0);
                    let items: &[(SettingsTab, Icon, &str)] = &[
                        (SettingsTab::General, Icon::Home, "general"),
                        (SettingsTab::Appearance, Icon::Palette, "appearance"),
                        (SettingsTab::Privacy, Icon::Privacy, "privacy"),
                        (SettingsTab::Security, Icon::Lock, "security"),
                        (SettingsTab::Network, Icon::Network, "network"),
                        (SettingsTab::Vpn, Icon::Shield, "vpn"),
                        (SettingsTab::Profiles, Icon::Profile, "profiles"),
                        (SettingsTab::Downloads, Icon::Download, "downloads"),
                        (SettingsTab::Bookmarks, Icon::Bookmark, "bookmarks"),
                        (SettingsTab::History, Icon::History, "history"),
                        (SettingsTab::Passwords, Icon::Password, "passwords"),
                        (SettingsTab::Import, Icon::Import, "import"),
                        (SettingsTab::Extensions, Icon::Extension, "extensions"),
                        (SettingsTab::Advanced, Icon::Advanced, "advanced"),
                        (SettingsTab::Language, Icon::Globe, "language"),
                        (SettingsTab::About, Icon::Info, "about"),
                    ];
                    for &(tab, icon, key) in items {
                        sidebar_item(ui, controller, tab, icon, i18n::t(lang, key));
                    }
                    ui.add_space(space::LG);
                });
        },
    );

    // Divider
    page.painter().vline(
        sidebar_rect.right(),
        rect.y_range(),
        Stroke::new(1.0_f32, t.border),
    );

    // ── Content ──
    let content_w = (rect.width() - SIDEBAR_W - 1.0).max(1.0);
    page.allocate_ui_with_layout(
        Vec2::new(content_w, rect.height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(content_w);
            ui.add_space(space::LG);
            ui.horizontal(|ui| {
                ui.add_space(space::LG);
                ui.label(
                    RichText::new(tab_title(lang, controller.settings_tab))
                        .size(22.0)
                        .strong()
                        .color(t.text),
                );
            });
            ui.add_space(space::MD);

            ScrollArea::vertical()
                .id_salt("settings_body")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.set_width(content_w);
                    ui.horizontal(|ui| {
                        ui.add_space(space::LG);
                        ui.vertical(|ui| {
                            ui.set_max_width(560.0);
                            ui.push_id(controller.settings_tab, |ui| {
                                match controller.settings_tab {
                                    SettingsTab::General | SettingsTab::Search => {
                                        draw_general(ui, controller, engine)
                                    }
                                    SettingsTab::Appearance => {
                                        draw_appearance(ui, controller, engine)
                                    }
                                    SettingsTab::Language => draw_language(ui, controller, engine),
                                    SettingsTab::Privacy | SettingsTab::Security => {
                                        draw_privacy(ui, controller)
                                    }
                                    SettingsTab::Network => draw_network(ui, controller),
                                    SettingsTab::Vpn => draw_vpn(ui, controller),
                                    SettingsTab::Profiles => draw_profiles(ui, controller),
                                    SettingsTab::Downloads => {
                                        draw_downloads_settings(ui, controller)
                                    }
                                    SettingsTab::Bookmarks | SettingsTab::History => {
                                        draw_library_hint(ui, controller)
                                    }
                                    SettingsTab::Extensions => draw_extensions(ui, controller),
                                    SettingsTab::Advanced => {
                                        draw_advanced(ui, controller, engine)
                                    }
                                    SettingsTab::About => draw_about(ui, controller),
                                    SettingsTab::Import => {
                                        controller.ensure_detected_browsers();
                                        draw_import(ui, controller, engine);
                                    }
                                    SettingsTab::Passwords => draw_passwords(ui, controller),
                                }
                            });
                        });
                    });
                    ui.add_space(space::XL);
                });
        },
    );
}

/// Import dialog as an in-window centered modal.
pub fn draw_import_overlay(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    if controller.show_import_modal {
        draw_import_modal(ctx, controller, engine);
    }
}

fn tab_title(lang: UiLanguage, tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::General | SettingsTab::Search => i18n::t(lang, "general"),
        SettingsTab::Appearance => i18n::t(lang, "appearance"),
        SettingsTab::Language => i18n::t(lang, "language"),
        SettingsTab::Privacy => i18n::t(lang, "privacy"),
        SettingsTab::Security => i18n::t(lang, "security"),
        SettingsTab::Network => i18n::t(lang, "network"),
        SettingsTab::Vpn => i18n::t(lang, "vpn"),
        SettingsTab::Profiles => i18n::t(lang, "profiles"),
        SettingsTab::Downloads => i18n::t(lang, "downloads"),
        SettingsTab::Bookmarks => i18n::t(lang, "bookmarks"),
        SettingsTab::History => i18n::t(lang, "history"),
        SettingsTab::Extensions => i18n::t(lang, "extensions"),
        SettingsTab::Advanced => i18n::t(lang, "advanced"),
        SettingsTab::About => i18n::t(lang, "about"),
        SettingsTab::Import => i18n::t(lang, "import"),
        SettingsTab::Passwords => i18n::t(lang, "passwords"),
    }
}

fn sidebar_item(
    ui: &mut egui::Ui,
    controller: &mut BrowserController,
    tab: SettingsTab,
    icon: Icon,
    label: &str,
) {
    let selected = controller.settings_tab == tab;
    let t = theme::current();
    let desired = Vec2::new((SIDEBAR_W - 16.0).max(120.0), 34.0);

    // Reserve layout space, then register exactly one interactive id.
    let (_, rect) = ui.allocate_space(desired);
    let resp = ui.interact(rect, ui.id().with(("settings_nav_item", tab)), Sense::click());

    let fill = if selected {
        t.accent_muted
    } else if resp.hovered() {
        t.surface_hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, radius::MD, fill);

    let icon_color = if selected { t.accent } else { t.text_secondary };
    let text_color = if selected { t.accent } else { t.text_secondary };
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 18.0, rect.center().y),
        Vec2::splat(14.0),
    );
    icons::paint(ui, icon, icon_rect, icon_color);
    ui.painter().text(
        Pos2::new(rect.left() + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        text_color,
    );

    if resp.clicked() {
        controller.settings_tab = tab;
    }
    ui.add_space(2.0);
}

fn draw_appearance(
    ui: &mut egui::Ui,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let lang = controller.store.settings.language;
    widgets::caption(ui, i18n::t(lang, "theme"));
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (value, key) in [
            (Theme::System, "theme_system"),
            (Theme::Light, "theme_light"),
            (Theme::Dark, "theme_dark"),
        ] {
            let selected = controller.store.settings.theme == value;
            if widgets::theme_option(ui, i18n::t(lang, key), selected).clicked() {
                controller.store.settings.theme = value;
                theme::invalidate_applied();
            }
        }
    });

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "color_scheme"));
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for scheme in ColorScheme::ALL {
            let selected = controller.store.settings.color_scheme == scheme;
            let resp = theme::color_swatch(ui, scheme, selected)
                .on_hover_text(i18n::t(lang, scheme.label_key()));
            if resp.clicked() {
                controller.store.settings.color_scheme = scheme;
                theme::invalidate_applied();
            }
        }
    });

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "home_background"));
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        if widgets::ghost_button(ui, i18n::t(lang, "choose_image")).clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                .pick_file()
            {
                controller.set_home_background_from_path(path);
            }
        }
        if controller.store.settings.home_background.is_some()
            && widgets::ghost_button(ui, i18n::t(lang, "clear_background")).clicked()
        {
            controller.clear_home_background();
        }
    });
    if let Some(path) = &controller.store.settings.home_background {
        ui.add_space(space::SM);
        ui.label(
            RichText::new(path.display().to_string())
                .size(12.0)
                .color(theme::current().text_tertiary),
        );
    }

    ui.add_space(space::XL);
    if widgets::primary_button(ui, i18n::t(lang, "save")).clicked() {
        save_settings(controller, engine);
    }
}

fn draw_language(
    ui: &mut egui::Ui,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let lang = controller.store.settings.language;
    widgets::caption(ui, i18n::t(lang, "ui_language"));
    ui.add_space(space::SM);
    for value in UiLanguage::ALL {
        let selected = controller.store.settings.language == value;
        let label = format!("{} ({})", value.native_name(), value.code());
        if ui.selectable_label(selected, label).clicked() {
            controller.store.settings.language = value;
        }
    }
    ui.add_space(space::XL);
    if widgets::primary_button(ui, i18n::t(lang, "save")).clicked() {
        save_settings(controller, engine);
    }
}

fn draw_general(ui: &mut egui::Ui, controller: &mut BrowserController, engine: &mut PageBackend) {
    let lang = controller.store.settings.language;

    widgets::caption(ui, i18n::t(lang, "startup"));
    ui.add_space(space::SM);
    egui::Frame::new()
        .fill(theme::current().surface_raised)
        .stroke(Stroke::new(1.0_f32, theme::current().border))
        .corner_radius(CornerRadius::same(radius::MD as u8))
        .inner_margin(egui::Margin::same(space::MD as i8))
        .show(ui, |ui| {
            ui.checkbox(
                &mut controller.store.settings.restore_previous_session,
                i18n::t(lang, "restore_session"),
            );
            ui.add_space(space::SM);
            egui::ComboBox::from_id_salt("startup")
                .selected_text(startup_label(
                    lang,
                    &controller.store.settings.startup_behavior,
                ))
                .show_ui(ui, |ui| {
                    for (v, key) in [
                        (StartupBehavior::Homepage, "startup_homepage"),
                        (StartupBehavior::RestorePreviousSession, "startup_restore"),
                        (StartupBehavior::AskToRestore, "startup_ask"),
                    ] {
                        ui.selectable_value(
                            &mut controller.store.settings.startup_behavior,
                            v,
                            i18n::t(lang, key),
                        );
                    }
                });
        });

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "homepage"));
    ui.add_space(space::SM);
    ui.add(
        egui::TextEdit::singleline(&mut controller.store.settings.homepage)
            .id_salt("settings_homepage")
            .desired_width(f32::INFINITY)
            .hint_text("about:newtab"),
    );
    ui.label(
        RichText::new(i18n::t(lang, "homepage_hint"))
            .size(12.0)
            .color(theme::current().text_tertiary),
    );

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "search_engine"));
    ui.add_space(space::SM);
    ui.add(
        egui::TextEdit::singleline(&mut controller.store.settings.search_engine)
            .id_salt("settings_search_engine")
            .desired_width(f32::INFINITY),
    );

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "zoom"));
    let zoom_before = controller.store.settings.default_zoom;
    ui.add(
        egui::Slider::new(&mut controller.store.settings.default_zoom, 0.5..=2.0)
            .show_value(true)
            .suffix("×"),
    );
    if (controller.store.settings.default_zoom - zoom_before).abs() > f32::EPSILON {
        controller.apply_page_zoom(engine);
    }

    ui.add_space(space::XL);
    if widgets::primary_button(ui, i18n::t(lang, "save")).clicked() {
        save_settings(controller, engine);
    }
}

fn startup_label(lang: UiLanguage, b: &StartupBehavior) -> &'static str {
    match b {
        StartupBehavior::Homepage => i18n::t(lang, "startup_homepage"),
        StartupBehavior::RestorePreviousSession => i18n::t(lang, "startup_restore"),
        StartupBehavior::AskToRestore => i18n::t(lang, "startup_ask"),
    }
}

fn draw_privacy(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let lang = controller.store.settings.language;
    let t = theme::current();
    ui.label(
        RichText::new(i18n::t(lang, "privacy_placeholder"))
            .color(t.text_secondary),
    );
    ui.add_space(space::MD);
    if widgets::ghost_button(ui, i18n::t(lang, "clear_all")).clicked() {
        let _ = controller.store.history.clear_all();
        controller.push_status(i18n::t(lang, "history_cleared"));
    }
}

fn draw_network(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let lang = controller.store.settings.language;
    let t = theme::current();

    widgets::caption(ui, i18n::t(lang, "local_hosts"));
    ui.add_space(space::SM);
    ui.label(
        RichText::new(i18n::t(lang, "local_hosts_help"))
            .size(12.0)
            .color(t.text_secondary),
    );
    ui.add_space(space::MD);

    let mut remove_at: Option<usize> = None;
    let entries: Vec<_> = controller.store.settings.local_hosts.clone();
    if entries.is_empty() {
        ui.label(
            RichText::new(i18n::t(lang, "local_hosts_empty"))
                .size(12.0)
                .color(t.text_tertiary),
        );
    } else {
        for (idx, entry) in entries.iter().enumerate() {
            ui.push_id(("local_host_row", idx), |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} → {}", entry.domain, entry.target_display()))
                            .size(13.0)
                            .color(t.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::ghost_button(ui, i18n::t(lang, "local_hosts_remove")).clicked()
                        {
                            remove_at = Some(idx);
                        }
                    });
                });
            });
            ui.add_space(space::SM);
        }
    }
    if let Some(idx) = remove_at {
        if idx < controller.store.settings.local_hosts.len() {
            controller.store.settings.local_hosts.remove(idx);
        }
    }

    ui.add_space(space::LG);
    widgets::caption(ui, i18n::t(lang, "local_hosts_add"));
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width().min(560.0));
        ui.vertical(|ui| {
            ui.label(RichText::new(i18n::t(lang, "local_hosts_domain")).size(12.0));
            ui.add(
                egui::TextEdit::singleline(&mut controller.local_host_domain)
                    .hint_text("app.local")
                    .desired_width(260.0),
            );
        });
        ui.add_space(space::MD);
        ui.vertical(|ui| {
            ui.label(RichText::new(i18n::t(lang, "local_hosts_ip")).size(12.0));
            ui.add(
                egui::TextEdit::singleline(&mut controller.local_host_ip)
                    .hint_text("192.168.1.10:3000")
                    .desired_width(180.0),
            );
        });
    });
    ui.add_space(space::SM);
    if widgets::ghost_button(ui, i18n::t(lang, "local_hosts_add_btn")).clicked() {
        match LocalHostEntry::from_target(
            controller.local_host_domain.clone(),
            &controller.local_host_ip,
        ) {
            Ok(entry) => {
                if let Some(existing) = controller
                    .store
                    .settings
                    .local_hosts
                    .iter_mut()
                    .find(|e| e.domain.eq_ignore_ascii_case(&entry.domain))
                {
                    *existing = entry;
                } else {
                    controller.store.settings.local_hosts.push(entry);
                }
                controller.local_host_domain.clear();
                controller.local_host_ip.clear();
            }
            Err(reason) => {
                controller.push_status(reason);
            }
        }
    }

    ui.add_space(space::XL);
    if widgets::primary_button(ui, i18n::t(lang, "save")).clicked() {
        let lang = controller.store.settings.language;
        theme::invalidate_applied();
        if let Err(err) = controller.store.save_settings() {
            controller.push_status(err.to_string());
        } else {
            controller.push_status(i18n::t(lang, "local_hosts_restart").to_string());
        }
    }
}

fn draw_vpn(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let t = theme::current();
    ui.label(
        RichText::new("VPN integration point — UI only for now.")
            .color(t.text_secondary),
    );
    ui.add_space(space::MD);
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, "Connect").clicked() {
            controller.vpn_state = crate::controller::VpnUiState::On;
        }
        if widgets::ghost_button(ui, "Disconnect").clicked() {
            controller.vpn_state = crate::controller::VpnUiState::Off;
        }
    });
}

fn draw_profiles(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let t = theme::current();
    let list: Vec<_> = controller
        .profiles
        .list()
        .iter()
        .map(|p| (p.id, p.name.clone()))
        .collect();
    let active = controller.profiles.active().ok().map(|p| p.id);
    for (id, name) in list {
        let selected = Some(id) == active;
        ui.push_id(id, |ui| {
            ui.horizontal(|ui| {
                ui.label(if selected {
                    format!("●  {name}")
                } else {
                    format!("○  {name}")
                });
                if !selected && widgets::ghost_button(ui, "Switch").clicked() {
                    let _ = controller.profiles.switch(id);
                }
            });
        });
        ui.add_space(4.0);
    }
    ui.add_space(space::MD);
    ui.label(
        RichText::new("Create additional profiles from the profile manager.")
            .color(t.text_tertiary)
            .size(12.0),
    );
}

fn draw_downloads_settings(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let lang = controller.store.settings.language;
    widgets::caption(ui, i18n::t(lang, "downloads"));
    ui.add_space(space::SM);
    if widgets::ghost_button(ui, i18n::t(lang, "open_downloads")).clicked() {
        controller.show_downloads = true;
    }
}

fn draw_library_hint(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let lang = controller.store.settings.language;
    let t = theme::current();
    ui.label(
        RichText::new(i18n::t(lang, "library_hint")).color(t.text_secondary),
    );
    ui.add_space(space::MD);
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, i18n::t(lang, "history")).clicked() {
            controller.show_history = true;
        }
        if widgets::ghost_button(ui, i18n::t(lang, "bookmarks")).clicked() {
            controller.show_bookmarks = true;
        }
    });
}

fn draw_extensions(ui: &mut egui::Ui, _controller: &mut BrowserController) {
    let t = theme::current();
    ui.label(RichText::new("Extensions are not available yet.").color(t.text_secondary));
}

fn draw_advanced(ui: &mut egui::Ui, controller: &mut BrowserController, engine: &mut PageBackend) {
    let lang = controller.store.settings.language;
    widgets::caption(ui, i18n::t(lang, "zoom"));
    let zoom_before = controller.store.settings.default_zoom;
    ui.add(
        egui::Slider::new(&mut controller.store.settings.default_zoom, 0.5..=2.0)
            .text(i18n::t(lang, "zoom"))
            .suffix("×"),
    );
    if (controller.store.settings.default_zoom - zoom_before).abs() > f32::EPSILON {
        controller.apply_page_zoom(engine);
    }
    ui.add_space(space::XL);
    if widgets::primary_button(ui, i18n::t(lang, "save")).clicked() {
        save_settings(controller, engine);
    }
}

fn draw_about(ui: &mut egui::Ui, _controller: &mut BrowserController) {
    let t = theme::current();
    ui.label(RichText::new("Rust Browser").size(22.0).strong());
    ui.add_space(space::SM);
    ui.label(RichText::new("Desktop browser powered by Servo").color(t.text_secondary));
    ui.add_space(space::MD);
    ui.label(RichText::new("Version 0.1.0").size(12.0).color(t.text_tertiary));
}

fn draw_import(ui: &mut egui::Ui, controller: &mut BrowserController, engine: &mut PageBackend) {
    let lang = controller.store.settings.language;
    widgets::caption(ui, i18n::t(lang, "import_source"));
    ui.add_space(space::SM);

    ui.horizontal(|ui| {
        if widgets::ghost_button(ui, i18n::t(lang, "import_refresh")).clicked() {
            controller.refresh_detected_browsers();
        }
        if widgets::primary_button(ui, i18n::t(lang, "import_open_modal")).clicked() {
            controller.show_import_modal = true;
        }
    });
    ui.add_space(space::MD);

    // Brand strip — always visible so users see supported browsers.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(space::MD, space::MD);
        for brand in crate::browser_icons::BrowserBrand::ALL {
            ui.vertical(|ui| {
                ui.set_min_width(64.0);
                ui.horizontal(|ui| {
                    ui.add_space((64.0 - 36.0) * 0.5);
                    crate::browser_icons::paint_brand_icon(
                        ui,
                        &mut controller.browser_icons,
                        brand,
                        36.0,
                    );
                });
                ui.label(
                    RichText::new(brand.label())
                        .size(11.0)
                        .color(theme::current().text_secondary),
                );
            });
        }
    });
    ui.add_space(space::LG);

    if controller.detected_browsers.is_empty() {
        ui.label(
            RichText::new(i18n::t(lang, "import_none_found")).color(theme::current().text_secondary),
        );
    } else {
        for i in 0..controller.detected_browsers.len() {
            let (id, label, caps, selected, brand) = {
                let source = &controller.detected_browsers[i];
                (
                    source.id.clone(),
                    source.label().to_string(),
                    source.capabilities_label().to_string(),
                    controller.selected_import_source.as_deref() == Some(source.id.as_str()),
                    crate::browser_icons::BrowserBrand::from_browser_name(&source.browser_name),
                )
            };
            ui.push_id(id.as_str(), |ui| {
                if widgets::card(ui, selected, |ui| {
                    ui.horizontal(|ui| {
                        crate::browser_icons::paint_brand_icon(
                            ui,
                            &mut controller.browser_icons,
                            brand,
                            28.0,
                        );
                        ui.add_space(space::SM);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&label).strong());
                            ui.label(
                                RichText::new(&caps)
                                    .size(12.0)
                                    .color(theme::current().text_secondary),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::primary_button(ui, i18n::t(lang, "import_from")).clicked() {
                                controller.run_import_detected(engine, &id);
                            }
                        });
                    });
                })
                .clicked()
                {
                    controller.selected_import_source = Some(id.clone());
                }
            });
            ui.add_space(space::SM);
        }
    }

    ui.add_space(space::LG);
    ui.separator();
    ui.add_space(space::MD);
    widgets::caption(ui, i18n::t(lang, "import_manual"));
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        if widgets::ghost_button(ui, i18n::t(lang, "import_pick_file")).clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Browser export", &["html", "htm", "csv", "json"])
                .pick_file()
            {
                controller.import_path_buf = path.display().to_string();
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut controller.import_path_buf)
                .id_salt("settings_import_path")
                .desired_width(ui.available_width())
                .hint_text("bookmarks.html"),
        );
    });
    ui.add_space(space::MD);
    ui.checkbox(&mut controller.import_bookmarks, i18n::t(lang, "bookmarks"));
    ui.checkbox(&mut controller.import_history, i18n::t(lang, "history"));
    ui.checkbox(&mut controller.import_passwords, i18n::t(lang, "passwords"));
    ui.add_space(space::MD);
    if widgets::primary_button(ui, i18n::t(lang, "import_run")).clicked() {
        controller.run_import(engine);
    }
    if !controller.import_status.is_empty() {
        ui.add_space(space::SM);
        ui.label(RichText::new(&controller.import_status).color(theme::current().accent));
    }
}

fn draw_import_modal(
    ctx: &egui::Context,
    controller: &mut BrowserController,
    engine: &mut PageBackend,
) {
    let lang = controller.store.settings.language;
    let t = theme::current();
    let mut open = controller.show_import_modal;

    egui::Window::new(i18n::t(lang, "import_title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([520.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(t.surface_raised)
                .stroke(Stroke::new(1.0_f32, t.border))
                .corner_radius(CornerRadius::same(radius::LG as u8))
                .inner_margin(egui::Margin::same(space::LG as i8))
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: t.shadow,
                }),
        )
        .show(ctx, |ui| {
            widgets::caption(ui, i18n::t(lang, "import_source"));
            ui.add_space(space::SM);

            // Browser family cards with icons
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(space::SM, space::SM);
                for brand in crate::browser_icons::BrowserBrand::ALL {
                    let needle = brand.label().to_ascii_lowercase();
                    let matched = controller
                        .detected_browsers
                        .iter()
                        .find(|b| b.browser_name.to_ascii_lowercase().contains(&needle))
                        .map(|b| b.id.clone());
                    let selected = matched
                        .as_ref()
                        .map(|id| controller.selected_import_source.as_deref() == Some(id.as_str()))
                        .unwrap_or(false);
                    let available = matched.is_some();
                    if widgets::card(ui, selected, |ui| {
                        ui.set_min_size(egui::vec2(92.0, 72.0));
                        ui.vertical_centered(|ui| {
                            crate::browser_icons::paint_brand_icon(
                                ui,
                                &mut controller.browser_icons,
                                brand,
                                32.0,
                            );
                            ui.add_space(4.0);
                            let color = if available {
                                theme::current().text
                            } else {
                                theme::current().text_tertiary
                            };
                            ui.label(RichText::new(brand.label()).strong().size(11.0).color(color));
                        });
                    })
                    .on_hover_text(if available {
                        brand.label()
                    } else {
                        "Not detected on this Mac"
                    })
                    .clicked()
                    {
                        if let Some(id) = matched {
                            controller.selected_import_source = Some(id);
                        }
                    }
                }
            });

            ui.add_space(space::LG);
            widgets::caption(ui, i18n::t(lang, "import_path"));
            ui.add_space(space::SM);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut controller.import_path_buf)
                        .id_salt("import_modal_path")
                        .desired_width(ui.available_width() - 100.0)
                        .hint_text("bookmarks.html"),
                );
                if widgets::ghost_button(ui, i18n::t(lang, "import_pick_file")).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Browser export", &["html", "htm", "csv", "json"])
                        .pick_file()
                    {
                        controller.import_path_buf = path.display().to_string();
                    }
                }
            });

            ui.add_space(space::LG);
            widgets::caption(ui, i18n::t(lang, "import"));
            ui.add_space(space::SM);
            ui.checkbox(&mut controller.import_bookmarks, i18n::t(lang, "bookmarks"));
            ui.checkbox(&mut controller.import_history, i18n::t(lang, "history"));
            ui.checkbox(&mut controller.import_passwords, i18n::t(lang, "passwords"));

            ui.add_space(space::XL);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, i18n::t(lang, "import_run")).clicked() {
                        if let Some(id) = controller.selected_import_source.clone() {
                            controller.run_import_detected(engine, &id);
                        } else {
                            controller.run_import(engine);
                        }
                        controller.show_import_modal = false;
                    }
                    if widgets::ghost_button(ui, i18n::t(lang, "cancel")).clicked() {
                        controller.show_import_modal = false;
                    }
                });
            });

            if !controller.import_status.is_empty() {
                ui.add_space(space::SM);
                ui.label(RichText::new(&controller.import_status).color(t.accent));
            }
        });

    controller.show_import_modal = open;
}

fn draw_passwords(ui: &mut egui::Ui, controller: &mut BrowserController) {
    let lang = controller.store.settings.language;
    match controller.store.credentials.list() {
        Ok(items) if items.is_empty() => {
            ui.label(i18n::t(lang, "passwords_empty"));
        }
        Ok(items) => {
            for cred in items {
                egui::Frame::new()
                    .fill(theme::current().surface_raised)
                    .stroke(Stroke::new(1.0_f32, theme::current().border))
                    .corner_radius(CornerRadius::same(radius::MD as u8))
                    .inner_margin(egui::Margin::same(space::MD as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&cred.origin).strong());
                                ui.label(format!(
                                    "{}: {}",
                                    i18n::t(lang, "username"),
                                    cred.username
                                ));
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::icon_button(ui, Icon::Trash, "Remove", true, false)
                                        .clicked()
                                    {
                                        let _ = controller.store.credentials.remove(cred.id);
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(space::SM);
            }
        }
        Err(err) => {
            ui.colored_label(theme::current().danger, err.to_string());
        }
    }
}

fn save_settings(controller: &mut BrowserController, engine: &mut PageBackend) {
    let lang = controller.store.settings.language;
    theme::invalidate_applied();
    controller.apply_page_zoom(engine);
    if let Err(err) = controller.store.save_settings() {
        controller.push_status(err.to_string());
    } else {
        controller.push_status(i18n::t(lang, "saved"));
    }
}
