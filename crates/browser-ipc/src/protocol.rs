use browser_core::TabId;
use serde::{Deserialize, Serialize};
use url::Url;

/// Bump when breaking wire format. Mismatch → content process rejected.
pub const PROTOCOL_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub version: u32,
    pub request_id: u64,
    /// Content session generation. Stale after Content restart.
    #[serde(default)]
    pub generation: u64,
    /// Optional ordering for frames / load events (per tab when applicable).
    #[serde(default)]
    pub sequence: u64,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(request_id: u64, payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            generation: 0,
            sequence: 0,
            payload,
        }
    }

    pub fn with_generation(request_id: u64, generation: u64, payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            generation,
            sequence: 0,
            payload,
        }
    }

    pub fn with_sequence(request_id: u64, sequence: u64, payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            generation: 0,
            sequence,
            payload,
        }
    }

    pub fn full(request_id: u64, generation: u64, sequence: u64, payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            generation,
            sequence,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrowserToContent {
    Hello {
        protocol_version: u32,
    },
    CreateTab {
        tab_id: TabId,
        url: Option<Url>,
    },
    Navigate {
        tab_id: TabId,
        url: Url,
    },
    Reload {
        tab_id: TabId,
    },
    GoBack {
        tab_id: TabId,
    },
    GoForward {
        tab_id: TabId,
    },
    Stop {
        tab_id: TabId,
    },
    CloseTab {
        tab_id: TabId,
    },
    FocusTab {
        tab_id: TabId,
    },
    SuspendTab {
        tab_id: TabId,
    },
    ResumeTab {
        tab_id: TabId,
    },
    Resize {
        width: u32,
        height: u32,
        scale_factor: f64,
    },
    SetViewport {
        tab_id: TabId,
        width: u32,
        height: u32,
        scale_factor: f64,
    },
    Input {
        tab_id: TabId,
        event: InputEventMsg,
    },
    /// Ask content to paint + send a frame for the focused tab.
    RequestFrame {
        tab_id: TabId,
    },
    /// Evaluate JavaScript in the tab's main frame. Answered by
    /// [`ContentToBrowser::ScriptResult`] with the same `request_id`.
    EvaluateScript {
        tab_id: TabId,
        script: String,
    },
    /// PNG screenshot of the viewport once the page has settled (fonts, images).
    /// Answered by [`ContentToBrowser::Screenshot`] with the same `request_id`.
    CaptureScreenshot {
        tab_id: TabId,
    },
    /// Requests the tab has made so far (automation network log).
    /// Answered by [`ContentToBrowser::NetworkLog`] with the same `request_id`.
    GetNetworkLog {
        tab_id: TabId,
    },
    /// Apply proxy routing policy (no secrets — host/port/scheme only).
    SetNetworkRoute {
        mode: NetworkRouteMsg,
    },
    Heartbeat,
    Shutdown,
}

/// One resource request seen by the engine (no bodies — those come from the page hook).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkRequestMsg {
    pub url: String,
    pub method: String,
    /// Fetch destination: `document`, `script`, `image`, … `empty` = fetch()/XHR.
    pub destination: String,
    pub main_frame: bool,
    /// Blocked by the content process (private network in automation mode).
    #[serde(default)]
    pub blocked: bool,
}

/// Serializable proxy/route hint for Content (P1.2 boundary prep — not full Network Core).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkRouteMsg {
    Direct,
    Proxy {
        scheme: String,
        host: String,
        port: u16,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEventMsg {
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseButton {
        x: f64,
        y: f64,
        button: u8,
        pressed: bool,
    },
    MouseWheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Key {
        key: String,
        pressed: bool,
        modifiers: u8,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentToBrowser {
    HelloAck {
        protocol_version: u32,
        pid: u32,
    },
    Ready,
    ProtocolMismatch {
        expected: u32,
        got: u32,
    },
    NavigationStarted {
        tab_id: TabId,
        url: Url,
    },
    NavigationFinished {
        tab_id: TabId,
        url: Url,
    },
    UrlChanged {
        tab_id: TabId,
        url: Url,
    },
    TitleChanged {
        tab_id: TabId,
        title: String,
    },
    LoadStatusChanged {
        tab_id: TabId,
        loading: bool,
    },
    LoadProgress {
        tab_id: TabId,
        progress: f32,
    },
    HistoryChanged {
        tab_id: TabId,
        can_go_back: bool,
        can_go_forward: bool,
    },
    Frame {
        tab_id: TabId,
        frame: FrameBuffer,
    },
    ConsoleMessage {
        tab_id: TabId,
        level: String,
        message: String,
    },
    TabCrashed {
        tab_id: TabId,
        reason: String,
    },
    /// Reply to [`BrowserToContent::EvaluateScript`] (same `request_id`).
    /// DOM nodes / windows come back as opaque id strings.
    ScriptResult {
        tab_id: TabId,
        result: Result<serde_json::Value, String>,
    },
    /// Reply to [`BrowserToContent::CaptureScreenshot`] (same `request_id`).
    Screenshot {
        tab_id: TabId,
        result: Result<String, String>,
    },
    /// Reply to [`BrowserToContent::GetNetworkLog`] (same `request_id`).
    NetworkLog {
        tab_id: TabId,
        requests: Vec<NetworkRequestMsg>,
    },
    Error {
        tab_id: Option<TabId>,
        message: String,
    },
    HeartbeatAck,
    ShutdownAck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA8 row-major pixels (may be empty for placeholder).
    pub rgba: Vec<u8>,
}

impl FrameBuffer {
    pub fn placeholder(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            rgba: Vec::new(),
        }
    }

    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

/// Tab id extracted from payload for sequence tracking (if any).
pub fn content_msg_tab(msg: &ContentToBrowser) -> Option<TabId> {
    match msg {
        ContentToBrowser::NavigationStarted { tab_id, .. }
        | ContentToBrowser::NavigationFinished { tab_id, .. }
        | ContentToBrowser::UrlChanged { tab_id, .. }
        | ContentToBrowser::TitleChanged { tab_id, .. }
        | ContentToBrowser::LoadStatusChanged { tab_id, .. }
        | ContentToBrowser::LoadProgress { tab_id, .. }
        | ContentToBrowser::HistoryChanged { tab_id, .. }
        | ContentToBrowser::Frame { tab_id, .. }
        | ContentToBrowser::ConsoleMessage { tab_id, .. }
        | ContentToBrowser::TabCrashed { tab_id, .. }
        | ContentToBrowser::ScriptResult { tab_id, .. }
        | ContentToBrowser::Screenshot { tab_id, .. }
        | ContentToBrowser::NetworkLog { tab_id, .. } => Some(*tab_id),
        ContentToBrowser::Error { tab_id, .. } => *tab_id,
        _ => None,
    }
}
