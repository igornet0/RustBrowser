//! Desktop shell: winit event loop, egui chrome, Servo page content.

mod app;
mod branding;
mod browser_icons;
mod chrome;
mod command_palette;
mod controller;
mod i18n;
mod icons;
mod page_backend;
mod remote_content;
mod settings_panel;
mod splash;
mod theme;
mod waker;
mod widgets;

pub use app::run;
pub use remote_content::RemoteContentSession;
