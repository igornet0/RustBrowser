//! Network routing decisions for navigations.
//!
//! Every navigation must pass through [`NetworkRouter`]. VPN without a tunnel
//! backend returns [`Route::Unavailable`] — never a silent Direct fallback.

use url::Url;

/// How traffic for a URL should be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkRoute {
    Direct,
    Proxy,
    Vpn,
}

/// Concrete route decision including configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Direct,
    Proxy(ProxyConfig),
    /// Real VPN profile selected — only valid when a tunnel backend exists.
    Vpn(VpnProfileId),
    /// Policy asked for VPN/proxy that cannot be satisfied. Must not fall back silently.
    Unavailable { reason: String },
    Block { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub http_proxy_uri: String,
    pub https_proxy_uri: String,
    pub no_proxy: String,
}

impl ProxyConfig {
    pub fn from_uri(uri: impl Into<String>) -> Self {
        let uri = uri.into();
        Self {
            http_proxy_uri: uri.clone(),
            https_proxy_uri: uri,
            no_proxy: String::new(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.http_proxy_uri.trim().is_empty() || !self.https_proxy_uri.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VpnProfileId(pub String);

/// Context for a single navigation / network decision.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub url: Url,
    pub tab_id: Option<String>,
    pub profile_id: Option<String>,
}

/// Legacy trait kept for compatibility; prefer [`NetworkRouter`].
pub trait NetworkPolicy: Send + Sync {
    fn route(&self, url: &Url) -> NetworkRoute;
}

/// MVP policy: everything is direct. Certificate validation stays with the engine.
#[derive(Debug, Default, Clone, Copy)]
pub struct DirectNetworkPolicy;

impl NetworkPolicy for DirectNetworkPolicy {
    fn route(&self, _url: &Url) -> NetworkRoute {
        NetworkRoute::Direct
    }
}

/// Decides how a request leaves the host.
pub trait NetworkRouter: Send + Sync {
    fn route(&self, request: &RequestContext) -> Route;
}

/// Profile/settings-driven router.
#[derive(Debug, Clone)]
pub struct SettingsNetworkRouter {
    pub preferred: NetworkRoute,
    pub proxy: Option<ProxyConfig>,
    /// When true, selecting Vpn yields Unavailable instead of pretending to connect.
    pub vpn_available: bool,
    pub vpn_profile: Option<VpnProfileId>,
}

impl Default for SettingsNetworkRouter {
    fn default() -> Self {
        Self {
            preferred: NetworkRoute::Direct,
            proxy: None,
            vpn_available: false,
            vpn_profile: None,
        }
    }
}

impl NetworkRouter for SettingsNetworkRouter {
    fn route(&self, _request: &RequestContext) -> Route {
        match self.preferred {
            NetworkRoute::Direct => Route::Direct,
            NetworkRoute::Proxy => match &self.proxy {
                Some(cfg) if cfg.is_configured() => Route::Proxy(cfg.clone()),
                _ => Route::Unavailable {
                    reason: "Proxy selected but no proxy URI configured".into(),
                },
            },
            NetworkRoute::Vpn => {
                if self.vpn_available {
                    if let Some(id) = &self.vpn_profile {
                        return Route::Vpn(id.clone());
                    }
                }
                Route::Unavailable {
                    reason: "VPN selected but no tunnel backend is available".into(),
                }
            }
        }
    }
}

impl NetworkRouter for DirectNetworkPolicy {
    fn route(&self, _request: &RequestContext) -> Route {
        Route::Direct
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_policy_always_direct() {
        let policy = DirectNetworkPolicy;
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(NetworkPolicy::route(&policy, &url), NetworkRoute::Direct);
        let route = NetworkRouter::route(
            &policy,
            &RequestContext {
                url,
                tab_id: None,
                profile_id: None,
            },
        );
        assert_eq!(route, Route::Direct);
    }

    #[test]
    fn vpn_without_backend_is_unavailable() {
        let router = SettingsNetworkRouter {
            preferred: NetworkRoute::Vpn,
            vpn_available: false,
            ..Default::default()
        };
        let route = router.route(&RequestContext {
            url: Url::parse("https://example.com").unwrap(),
            tab_id: None,
            profile_id: None,
        });
        assert!(matches!(route, Route::Unavailable { .. }));
    }

    #[test]
    fn proxy_requires_uri() {
        let router = SettingsNetworkRouter {
            preferred: NetworkRoute::Proxy,
            proxy: None,
            ..Default::default()
        };
        assert!(matches!(
            router.route(&RequestContext {
                url: Url::parse("https://example.com").unwrap(),
                tab_id: None,
                profile_id: None,
            }),
            Route::Unavailable { .. }
        ));
    }
}
