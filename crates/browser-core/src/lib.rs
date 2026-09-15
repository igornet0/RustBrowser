//! Core browser models shared across UI, engine, and profile crates.

mod browser;
mod content_process;
mod error;
mod lifecycle;
mod network;
mod process_manager;
mod tab;
mod url_util;
mod watchdog;

pub use browser::{Browser, ClosedTab};
pub use content_process::{ContentProcess, LocalContentProcess};
pub use error::{BrowserError, BrowserResult};
pub use lifecycle::{
    ContentResourceLimits, LifecycleEvent, LifecycleState, TransitionError,
};
pub use network::{
    DirectNetworkPolicy, NetworkPolicy, NetworkRoute, NetworkRouter, ProxyConfig, RequestContext,
    Route, SettingsNetworkRouter, VpnProfileId,
};
pub use process_manager::{
    content_isolation_enabled, ContentProcessHandle, ContentProcessId, ContentProcessManager,
    ContentProcessState, CrashLoopConfig, ProcessAssignmentPolicy, RestartDecision, SiteKey,
};
pub use tab::{Tab, TabId, TabState};
pub use url_util::normalize_url;
pub use watchdog::{Watchdog, WatchdogConfig, WatchdogStatus};
