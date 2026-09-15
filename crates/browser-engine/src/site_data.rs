//! Thin wrapper around Servo site-data / cache APIs.

use servo::{Servo, SiteDataManager as ServoSiteData, StorageType};

/// Application-facing site data controls. Backed by Servo where available.
pub struct SiteDataManager<'a> {
    inner: &'a ServoSiteData,
    servo: &'a Servo,
}

impl<'a> SiteDataManager<'a> {
    pub fn new(servo: &'a Servo) -> Self {
        Self {
            inner: servo.site_data_manager(),
            servo,
        }
    }

    pub fn clear_all_cookies(&self) {
        self.inner.clear_cookies(None);
    }

    pub fn clear_session_cookies(&self) {
        self.inner.clear_session_cookies(None);
    }

    pub fn clear_local_and_session_storage(&self) {
        // Empty site list: Servo clears matching storage types for listed sites only.
        // Callers that need a full wipe should pass sites from `site_data` listing later.
        self.inner
            .clear_site_data(&[], StorageType::Local | StorageType::Session);
    }

    pub fn clear_http_cache(&self) {
        self.servo.network_manager().clear_cache();
    }

    pub fn clear_all_site_data(&self) {
        self.clear_all_cookies();
        self.clear_local_and_session_storage();
        self.clear_http_cache();
    }
}

/// Documented capability matrix for Servo 0.5 (app layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCapability {
    Supported,
    Partial,
    NotSupported,
}

pub fn storage_capability(name: &str) -> StorageCapability {
    match name {
        "cookies" | "localStorage" | "sessionStorage" | "http_cache" => StorageCapability::Partial,
        "indexedDB" | "cache_api" | "service_workers" | "chips" => StorageCapability::NotSupported,
        _ => StorageCapability::NotSupported,
    }
}
