//! Seam for content execution. Browser Core talks to this trait — not to Servo.
//!
//! P0: [`LocalContentProcess`] (in-process, no isolation).
//! P1: [`RemoteContentProcess`] lives in `browser-ui` / engine glue and speaks IPC.

use crate::error::BrowserResult;
use crate::tab::TabId;
use url::Url;

pub trait ContentProcess: Send {
    fn id_label(&self) -> String;
    fn navigate(&mut self, tab_id: TabId, url: Url) -> BrowserResult<()>;
    fn reload(&mut self, tab_id: TabId) -> BrowserResult<()>;
    fn close_tab(&mut self, tab_id: TabId) -> BrowserResult<()>;
    fn shutdown(&mut self) -> BrowserResult<()>;
}

/// Placeholder for the legacy in-process embedding model.
/// Real Servo work remains in `ServoEngine` when `RUST_BROWSER_CONTENT=inprocess`.
#[derive(Debug, Clone)]
pub struct LocalContentProcess {
    pub tab_id: TabId,
}

impl LocalContentProcess {
    pub fn new(tab_id: TabId) -> Self {
        Self { tab_id }
    }
}

impl ContentProcess for LocalContentProcess {
    fn id_label(&self) -> String {
        format!("local:{}", self.tab_id)
    }

    fn navigate(&mut self, _tab_id: TabId, _url: Url) -> BrowserResult<()> {
        Ok(())
    }

    fn reload(&mut self, _tab_id: TabId) -> BrowserResult<()> {
        Ok(())
    }

    fn close_tab(&mut self, _tab_id: TabId) -> BrowserResult<()> {
        Ok(())
    }

    fn shutdown(&mut self) -> BrowserResult<()> {
        Ok(())
    }
}
