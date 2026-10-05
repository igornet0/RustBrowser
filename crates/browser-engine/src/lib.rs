//! Browser engine abstraction. UI talks only to this layer, never to Servo directly
//! except through this crate (or via Content Process IPC in P1 OOP mode).

mod content_runtime;
mod events;
mod identity;
mod ipc_keymap;
mod keyutils;
mod servo_backend;
mod site_data;
mod traits;
mod ui_surface;

pub use content_runtime::{run_content_process, ContentOptions};
pub use events::{EngineEvent, EngineViewId};
pub use identity::{build_preferences, desktop_user_agent};
pub use servo_backend::{ServoEngine, ServoEngineConfig};
pub use site_data::{storage_capability, SiteDataManager, StorageCapability};
pub use traits::{clear_proxy_config, BrowserEngine};
pub use ui_surface::UiSurface;
