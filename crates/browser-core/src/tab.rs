use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Centralized tab lifecycle. Do not track crash with a lone `bool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabState {
    Creating,
    Loading,
    Ready,
    Crashed { reason: String },
    Recovering,
    Closed,
}

impl TabState {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Creating | Self::Loading | Self::Recovering)
    }

    pub fn is_crashed(&self) -> bool {
        matches!(self, Self::Crashed { .. })
    }

    pub fn allows_transition(&self, next: &TabState) -> bool {
        use TabState::*;
        match (self, next) {
            (Closed, _) => false,
            (_, Closed) => true,
            (Creating, Loading | Ready | Crashed { .. }) => true,
            (Loading, Ready | Loading | Crashed { .. }) => true,
            (Ready, Loading | Crashed { .. }) => true,
            (Crashed { .. }, Recovering) => true,
            (Recovering, Loading | Ready | Crashed { .. }) => true,
            // idempotent
            (a, b) if a == b => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub url: Option<Url>,
    pub state: TabState,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// Last crash reason (kept after recover for UI history).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_crash_reason: Option<String>,
}

impl Tab {
    pub fn new(url: Option<Url>) -> Self {
        Self {
            id: TabId::new(),
            title: "New Tab".to_string(),
            url,
            state: TabState::Creating,
            can_go_back: false,
            can_go_forward: false,
            last_crash_reason: None,
        }
    }

    pub fn with_url(url: Url) -> Self {
        let title = url.host_str().unwrap_or("New Tab").to_string();
        Self {
            id: TabId::new(),
            title,
            url: Some(url),
            state: TabState::Loading,
            can_go_back: false,
            can_go_forward: false,
            last_crash_reason: None,
        }
    }

    pub fn loading(&self) -> bool {
        self.state.is_loading()
    }

    pub fn transition(&mut self, next: TabState) -> Result<(), String> {
        if !self.state.allows_transition(&next) {
            return Err(format!(
                "invalid tab state transition {:?} → {:?}",
                self.state, next
            ));
        }
        if let TabState::Crashed { reason } = &next {
            self.last_crash_reason = Some(reason.clone());
        }
        self.state = next;
        Ok(())
    }

    pub fn mark_loading(&mut self) {
        let _ = match self.state {
            TabState::Crashed { .. } => self.transition(TabState::Recovering),
            TabState::Recovering => self.transition(TabState::Loading),
            TabState::Ready | TabState::Creating => self.transition(TabState::Loading),
            TabState::Loading => Ok(()),
            TabState::Closed => Ok(()),
        };
        if matches!(self.state, TabState::Recovering) {
            let _ = self.transition(TabState::Loading);
        }
    }

    pub fn mark_ready(&mut self) {
        let _ = self.transition(TabState::Ready);
    }

    pub fn mark_crashed(&mut self, reason: String) {
        let _ = self.transition(TabState::Crashed { reason });
    }

    pub fn mark_recovering(&mut self) {
        let _ = self.transition(TabState::Recovering);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_then_recover_path() {
        let mut tab = Tab::with_url(Url::parse("https://example.com").unwrap());
        assert!(matches!(tab.state, TabState::Loading));
        tab.mark_ready();
        assert!(matches!(tab.state, TabState::Ready));
        tab.mark_crashed("boom".into());
        assert!(tab.state.is_crashed());
        tab.mark_recovering();
        tab.mark_loading();
        assert!(matches!(tab.state, TabState::Loading));
        tab.mark_ready();
        assert!(matches!(tab.state, TabState::Ready));
    }

    #[test]
    fn closed_is_terminal() {
        let mut tab = Tab::new(None);
        tab.transition(TabState::Closed).unwrap();
        assert!(tab.transition(TabState::Ready).is_err());
    }
}
