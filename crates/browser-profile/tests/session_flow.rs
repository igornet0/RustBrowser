//! Integration-style test of profile → tab → history → session restore (no GUI/Servo).

use browser_core::{Browser, Tab};
use browser_profile::{
    ProfileManager, ProfilePaths, ProfileStore, SessionState, SessionTab, WindowState,
};
use tempfile::tempdir;
use url::Url;

#[test]
fn profile_tab_history_session_roundtrip() {
    let dir = tempdir().unwrap();
    let mut profiles = ProfileManager::open(dir.path().join("profiles")).unwrap();
    let store = profiles.open_active_store().unwrap();

    let homepage = Url::parse("https://example.com/").unwrap();
    let mut browser = Browser::new(homepage.clone());
    store
        .history
        .record_visit(&homepage, "Example Domain")
        .unwrap();

    let second = Url::parse("https://example.org/").unwrap();
    let tab_id = browser.new_tab(Some(second.clone()));
    store.history.record_visit(&second, "Example Org").unwrap();
    assert_eq!(browser.tabs.len(), 2);

    let session = SessionState {
        tabs: browser.tabs.iter().map(SessionTab::from).collect(),
        active_tab: tab_id,
        window: WindowState::default(),
    };
    store.session.save(&session).unwrap();

    // Simulate restart.
    let store2 = ProfileStore::open(ProfilePaths::for_directory(
        &profiles.active().unwrap().directory,
    ))
    .unwrap();
    let loaded = store2.session.load().unwrap().unwrap();
    assert_eq!(loaded.tabs.len(), 2);
    assert_eq!(loaded.active_tab, tab_id);

    let mut tabs = Vec::new();
    for t in &loaded.tabs {
        let mut tab = match &t.url {
            Some(u) => Tab::with_url(u.clone()),
            None => Tab::new(None),
        };
        tab.id = t.id;
        tab.title = t.title.clone();
        tabs.push(tab);
    }
    let restored = Browser::from_tabs(tabs, loaded.active_tab).unwrap();
    assert_eq!(restored.active_tab, tab_id);
    assert_eq!(
        restored.active().unwrap().url.as_ref().map(Url::as_str),
        Some("https://example.org/")
    );

    let history = store2.history.list_recent(10).unwrap();
    assert!(history.len() >= 2);
}
