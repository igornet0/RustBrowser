# Network Architecture

## Flow (required)

```
Navigation request
  → RequestContext (url, tab, profile)
  → NetworkRouter::route
  → Route::{ Direct | Proxy(config) | Vpn(profile) | Unavailable }
  → Engine / network backend
```

No UI → Servo bypass of the router.

## Routes

| Route | P0 behaviour |
|-------|----------------|
| `Direct` | Servo default networking (rustls TLS) |
| `Proxy(ProxyConfig)` | Apply Servo `Preferences` HTTP(S) proxy fields for the session / navigation |
| `Vpn(_)` | **Unavailable** until a real tunnel backend exists — never silently Direct |
| `Block` | Reject navigation (future privacy / policy) |

## Traits

```rust
trait NetworkRouter: Send + Sync {
    fn route(&self, ctx: &RequestContext) -> Route;
}
```

Default: profile/settings-driven router (Direct or configured Proxy).

## Out of scope for P0

- WireGuard / userspace VPN
- Per-tab VPN exit nodes
- DoH/DoT custom resolvers
- HTTP/3 policy knobs beyond what Servo already does
