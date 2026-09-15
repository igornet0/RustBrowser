//! Lightweight lifecycle stress helpers (no Servo GUI).

use browser_core::{Browser, TabState};
use browser_profile::{SessionState, SessionStore, SessionTab, WindowState};
use tempfile::tempdir;
use url::Url;

#[test]
fn tab_crash_state_recreate_path() {
    let mut browser = Browser::new(Url::parse("https://example.com").unwrap());
    let id = browser.active_tab;
    browser.mark_tab_crashed(id, "test panic".into()).unwrap();
    assert!(browser.tab(id).unwrap().state.is_crashed());
    browser.begin_tab_recovery(id).unwrap();
    assert!(matches!(
        browser.tab(id).unwrap().state,
        TabState::Loading
    ));
    browser.set_loading(id, false).unwrap();
    assert!(matches!(browser.tab(id).unwrap().state, TabState::Ready));
    // Tab remains present
    assert_eq!(browser.tabs.len(), 1);
}

#[test]
fn session_checkpoint_survives_restart_and_corruption() {
    let dir = tempdir().unwrap();
    let mut store = SessionStore::new(dir.path().join("session.json"));
    let mut browser = Browser::new(Url::parse("https://a.test").unwrap());
    for i in 0..50 {
        browser.new_tab(Some(Url::parse(&format!("https://site{i}.test")).unwrap()));
    }
    let state = SessionState {
        tabs: browser.tabs.iter().map(SessionTab::from).collect(),
        active_tab: browser.active_tab,
        window: WindowState::default(),
    };
    store.checkpoint(state).unwrap();

    // Simulate unclean exit
    store.mark_running().unwrap();
    assert!(store.previous_session_unclean());

    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.tabs.len(), 51);

    // Corrupt file must not panic startup
    std::fs::write(store.path(), "{broken").unwrap();
    assert!(store.load().unwrap().is_none());
}

#[test]
fn repeated_tab_create_close_keeps_invariants() {
    let mut browser = Browser::new(Url::parse("https://example.com").unwrap());
    for _ in 0..100 {
        let id = browser.new_tab(Some(Url::parse("https://example.org").unwrap()));
        browser.close_tab(id).unwrap();
    }
    assert!(!browser.tabs.is_empty());
    assert!(browser.active().is_ok());
}
