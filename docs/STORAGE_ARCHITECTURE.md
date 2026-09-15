# Storage Architecture

## Profile layout

```
Profile/
├── History          (history.db)           — browser-profile
├── Bookmarks        (bookmarks.db)         — browser-profile
├── Downloads        (downloads.db + dir)   — browser-profile
├── Credentials      (passwords.db meta + OS keyring / encrypted secret)
├── Settings         (preferences.json)
├── Session          (session.json + running.lock)
├── Cookies          (Servo config_dir / storage/)
├── LocalStorage     (Servo storage/)
├── IndexedDB        (Servo, if enabled)    — engine-owned
└── Cache            (Servo + optional cache/ dir)
```

All of the above are **profile-scoped**. No intentional global cookie/credential pool across profiles.

## Ownership

| Store | Writer | Reader |
|-------|--------|--------|
| History/Bookmarks/Downloads/Settings/Session | browser-profile / UI | UI |
| Credential secrets | CredentialStore (keyring) | Autofill / settings |
| Cookies / DOM storage | Servo | Servo + SiteDataManager clear/list |
| HTTP cache | Servo NetworkManager | SiteDataManager / clear cache |

## SiteDataManager (app abstraction)

Single entry for:

- list / clear cookies
- clear local/session site data
- clear HTTP cache (via engine)
- future: service workers, permissions bulk reset

Backed by Servo `SiteDataManager` + `NetworkManager` where available.

## Partitioning (Servo 0.5 — audit)

| Storage | App control | Engine |
|---------|-------------|--------|
| Cookies | clear/list via SiteDataManager | SOP / engine cookie jar — **PARTIAL** app view |
| localStorage | clear via StorageType::Local | **PARTIAL** |
| sessionStorage | clear via StorageType::Session | **PARTIAL** |
| IndexedDB | not fully exposed in our wrapper yet | **NOT SUPPORTED** at app layer |
| Cache API / SW | not wrapped | **NOT SUPPORTED** / experimental in engine |
| CHIPS / storage keys | none | **NOT SUPPORTED** in app |

Do not claim full storage partitioning until engine + app both enforce it.

## Credentials

SQLite may store: id, origin, username, note, timestamps, keyring account key.  
SQLite must **not** store password plaintext after migration.
