use browser_core::TabId;
use serde::{Deserialize, Serialize};
use url::Url;

/// Stable id mapping a browser tab to an engine view (Servo `WebView`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineViewId(pub TabId);

impl From<TabId> for EngineViewId {
    fn from(id: TabId) -> Self {
        Self(id)
    }
}

/// Events emitted by the engine toward the controller/UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    UrlChanged {
        view: EngineViewId,
        url: Url,
    },
    TitleChanged {
        view: EngineViewId,
        title: Option<String>,
    },
    LoadStatusChanged {
        view: EngineViewId,
        loading: bool,
    },
    HistoryChanged {
        view: EngineViewId,
        can_go_back: bool,
        can_go_forward: bool,
    },
    NewFrameReady {
        view: EngineViewId,
    },
    Crashed {
        view: EngineViewId,
        reason: String,
    },
    /// Page asked for a privileged capability; embedder default-denied and surfaced this.
    PermissionRequested {
        view: EngineViewId,
        feature: String,
    },
    /// Page content wants a different mouse cursor (link, text, etc.).
    CursorChanged {
        cursor: servo::Cursor,
    },
}
