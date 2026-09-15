use crate::error::{BrowserError, BrowserResult};
use crate::tab::{Tab, TabId};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use url::Url;

/// Snapshot of a closed tab for restore (Cmd/Ctrl+Shift+T).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosedTab {
    pub title: String,
    pub url: Option<Url>,
}

/// In-memory browser session state (tabs). Persistence lives in browser-profile.
#[derive(Debug, Clone)]
pub struct Browser {
    pub tabs: Vec<Tab>,
    pub active_tab: TabId,
    closed_tabs: Vec<ClosedTab>,
}

impl Browser {
    pub fn new(homepage: Url) -> Self {
        let tab = Tab::with_url(homepage);
        let active_tab = tab.id;
        info!(tab_id = %active_tab, "tab creation");
        Self {
            tabs: vec![tab],
            active_tab,
            closed_tabs: Vec::new(),
        }
    }

    pub fn from_tabs(tabs: Vec<Tab>, active_tab: TabId) -> BrowserResult<Self> {
        if tabs.is_empty() {
            return Err(BrowserError::Other("cannot restore empty tab list".into()));
        }
        if !tabs.iter().any(|t| t.id == active_tab) {
            return Err(BrowserError::TabNotFound(active_tab.to_string()));
        }
        Ok(Self {
            tabs,
            active_tab,
            closed_tabs: Vec::new(),
        })
    }

    pub fn active(&self) -> BrowserResult<&Tab> {
        self.tabs
            .iter()
            .find(|t| t.id == self.active_tab)
            .ok_or_else(|| BrowserError::TabNotFound(self.active_tab.to_string()))
    }

    pub fn active_mut(&mut self) -> BrowserResult<&mut Tab> {
        let id = self.active_tab;
        self.tabs
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| BrowserError::TabNotFound(id.to_string()))
    }

    pub fn tab(&self, id: TabId) -> BrowserResult<&Tab> {
        self.tabs
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| BrowserError::TabNotFound(id.to_string()))
    }

    pub fn tab_mut(&mut self, id: TabId) -> BrowserResult<&mut Tab> {
        self.tabs
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| BrowserError::TabNotFound(id.to_string()))
    }

    pub fn new_tab(&mut self, url: Option<Url>) -> TabId {
        let tab = match url {
            Some(u) => Tab::with_url(u),
            None => Tab::new(None),
        };
        let id = tab.id;
        info!(tab_id = %id, "tab creation");
        self.tabs.push(tab);
        self.active_tab = id;
        id
    }

    pub fn close_tab(&mut self, id: TabId) -> BrowserResult<Option<ClosedTab>> {
        let idx = self
            .tabs
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| BrowserError::TabNotFound(id.to_string()))?;

        if self.tabs.len() == 1 {
            // Keep at least one tab: reset it instead of closing the window.
            let tab = &mut self.tabs[0];
            let closed = ClosedTab {
                title: tab.title.clone(),
                url: tab.url.clone(),
            };
            *tab = Tab::new(None);
            self.active_tab = tab.id;
            info!(tab_id = %id, "tab closing (reset last tab)");
            self.closed_tabs.push(closed.clone());
            return Ok(Some(closed));
        }

        let removed = self.tabs.remove(idx);
        info!(tab_id = %id, "tab closing");
        let closed = ClosedTab {
            title: removed.title,
            url: removed.url,
        };
        self.closed_tabs.push(closed.clone());

        if self.active_tab == id {
            let new_idx = idx.saturating_sub(1).min(self.tabs.len() - 1);
            self.active_tab = self.tabs[new_idx].id;
        }
        Ok(Some(closed))
    }

    pub fn switch_tab(&mut self, id: TabId) -> BrowserResult<()> {
        if !self.tabs.iter().any(|t| t.id == id) {
            return Err(BrowserError::TabNotFound(id.to_string()));
        }
        self.active_tab = id;
        Ok(())
    }

    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == self.active_tab) else {
            return;
        };
        let next = (idx + 1) % self.tabs.len();
        self.active_tab = self.tabs[next].id;
    }

    pub fn previous_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == self.active_tab) else {
            return;
        };
        let prev = if idx == 0 {
            self.tabs.len() - 1
        } else {
            idx - 1
        };
        self.active_tab = self.tabs[prev].id;
    }

    pub fn recent_closed_tabs(&self, limit: usize) -> Vec<ClosedTab> {
        self.closed_tabs.iter().rev().take(limit).cloned().collect()
    }

    pub fn restore_closed_tab(&mut self) -> Option<TabId> {
        let closed = self.closed_tabs.pop()?;
        let mut tab = match closed.url {
            Some(url) => Tab::with_url(url),
            None => Tab::new(None),
        };
        tab.title = closed.title;
        let id = tab.id;
        info!(tab_id = %id, "tab creation (restore closed)");
        self.tabs.push(tab);
        self.active_tab = id;
        Some(id)
    }

    pub fn duplicate_tab(&mut self, id: TabId) -> BrowserResult<TabId> {
        let url = self.tab(id)?.url.clone();
        let title = self.tab(id)?.title.clone();
        let mut tab = match url {
            Some(u) => Tab::with_url(u),
            None => Tab::new(None),
        };
        tab.title = title;
        let new_id = tab.id;
        info!(tab_id = %new_id, source = %id, "tab creation (duplicate)");
        self.tabs.push(tab);
        self.active_tab = new_id;
        Ok(new_id)
    }

    pub fn navigate_active(&mut self, url: Url) -> BrowserResult<()> {
        let tab = self.active_mut()?;
        tab.url = Some(url);
        tab.mark_loading();
        Ok(())
    }

    pub fn update_tab_url(&mut self, id: TabId, url: Url) -> BrowserResult<()> {
        let tab = self.tab_mut(id)?;
        tab.url = Some(url);
        Ok(())
    }

    pub fn update_tab_title(&mut self, id: TabId, title: String) -> BrowserResult<()> {
        let tab = self.tab_mut(id)?;
        tab.title = if title.is_empty() {
            "New Tab".into()
        } else {
            title
        };
        Ok(())
    }

    pub fn set_loading(&mut self, id: TabId, loading: bool) -> BrowserResult<()> {
        let tab = self.tab_mut(id)?;
        if loading {
            tab.mark_loading();
        } else if !tab.state.is_crashed() {
            tab.mark_ready();
        }
        Ok(())
    }

    pub fn mark_tab_crashed(&mut self, id: TabId, reason: String) -> BrowserResult<()> {
        self.tab_mut(id)?.mark_crashed(reason);
        Ok(())
    }

    pub fn begin_tab_recovery(&mut self, id: TabId) -> BrowserResult<()> {
        let tab = self.tab_mut(id)?;
        tab.mark_recovering();
        tab.mark_loading();
        Ok(())
    }

    pub fn set_nav_state(
        &mut self,
        id: TabId,
        can_go_back: bool,
        can_go_forward: bool,
    ) -> BrowserResult<()> {
        let tab = self.tab_mut(id)?;
        tab.can_go_back = can_go_back;
        tab.can_go_forward = can_go_forward;
        Ok(())
    }

    pub fn closed_stack_len(&self) -> usize {
        self.closed_tabs.len()
    }
}

impl Default for Browser {
    fn default() -> Self {
        match Url::parse("https://example.com") {
            Ok(url) => Self::new(url),
            Err(err) => {
                warn!(?err, "failed to parse default homepage");
                let tab = Tab::new(None);
                let active_tab = tab.id;
                Self {
                    tabs: vec![tab],
                    active_tab,
                    closed_tabs: Vec::new(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Url {
        Url::parse("https://example.com").unwrap()
    }

    #[test]
    fn new_and_close_tabs() {
        let mut browser = Browser::new(example());
        let first = browser.active_tab;
        let second = browser.new_tab(Some(Url::parse("https://example.org").unwrap()));
        assert_eq!(browser.tabs.len(), 2);
        assert_eq!(browser.active_tab, second);
        browser.close_tab(second).unwrap();
        assert_eq!(browser.tabs.len(), 1);
        assert_eq!(browser.active_tab, first);
    }

    #[test]
    fn restore_closed_tab() {
        let mut browser = Browser::new(example());
        let _ = browser.new_tab(Some(Url::parse("https://example.org").unwrap()));
        let closed_id = browser.active_tab;
        browser.close_tab(closed_id).unwrap();
        let restored = browser.restore_closed_tab().unwrap();
        assert_eq!(
            browser.tab(restored).unwrap().url.as_ref().map(|u| u.as_str()),
            Some("https://example.org/")
        );
    }

    #[test]
    fn next_previous_tab() {
        let mut browser = Browser::new(example());
        let a = browser.active_tab;
        let b = browser.new_tab(None);
        let c = browser.new_tab(None);
        browser.switch_tab(a).unwrap();
        browser.next_tab();
        assert_eq!(browser.active_tab, b);
        browser.next_tab();
        assert_eq!(browser.active_tab, c);
        browser.previous_tab();
        assert_eq!(browser.active_tab, b);
    }

    #[test]
    fn duplicate_tab() {
        let mut browser = Browser::new(example());
        let id = browser.active_tab;
        let dup = browser.duplicate_tab(id).unwrap();
        assert_ne!(id, dup);
        assert_eq!(
            browser.tab(dup).unwrap().url.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
    }
}
