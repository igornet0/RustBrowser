use crate::error::{BrowserError, BrowserResult};
use url::Url;

/// Normalize user address-bar input into an absolute URL.
///
/// - Adds `https://` when the scheme is missing and the text looks like a host.
/// - Falls back to a search URL when the text is a free-form query.
pub fn normalize_url(input: &str, search_engine_template: &str) -> BrowserResult<Url> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BrowserError::InvalidUrl("empty address".into()));
    }

    if let Ok(url) = Url::parse(trimmed) {
        if url.has_host() || url.scheme() == "about" || url.scheme() == "data" {
            return Ok(url);
        }
    }

    if looks_like_host(trimmed) {
        let candidate = format!("https://{trimmed}");
        return Url::parse(&candidate)
            .map_err(|e| BrowserError::InvalidUrl(format!("{trimmed}: {e}")));
    }

    let encoded = urlencoding_lite(trimmed);
    let search = search_engine_template.replace("%s", &encoded);
    Url::parse(&search).map_err(|e| BrowserError::InvalidUrl(format!("search url: {e}")))
}

fn looks_like_host(s: &str) -> bool {
    if s.contains(' ') {
        return false;
    }
    if s.starts_with("localhost") {
        return true;
    }
    // Simple heuristic: has a dot and no obvious search punctuation.
    s.contains('.') && !s.contains('?')
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(nibble(b >> 4));
                out.push(nibble(b & 0xf));
            }
        }
    }
    out
}

fn nibble(n: u8) -> char {
    char::from(if n < 10 { b'0' + n } else { b'A' + (n - 10) })
}

/// localhost, `.local`, loopback / RFC 1918 / link-local / unique-local addresses.
pub fn is_private_network_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local")
        }
        Some(url::Host::Ipv4(ip)) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
        }
        Some(url::Host::Ipv6(ip)) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return v4.is_loopback() || v4.is_private() || v4.is_link_local();
            }
            ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUCK: &str = "https://duckduckgo.com/?q=%s";

    #[test]
    fn parses_absolute_https() {
        let url = normalize_url("https://example.com/path", DUCK).unwrap();
        assert_eq!(url.as_str(), "https://example.com/path");
    }

    #[test]
    fn adds_https_for_host() {
        let url = normalize_url("example.com", DUCK).unwrap();
        assert_eq!(url.as_str(), "https://example.com/");
    }

    #[test]
    fn search_fallback() {
        let url = normalize_url("rust browser engine", DUCK).unwrap();
        assert!(url.as_str().contains("duckduckgo.com"));
        assert!(url.as_str().contains("rust+browser+engine"));
    }

    #[test]
    fn private_network_urls_are_detected() {
        for u in [
            "http://localhost:8080/",
            "http://api.localhost/",
            "http://printer.local/",
            "http://127.0.0.1/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(is_private_network_url(&Url::parse(u).unwrap()), "{u}");
        }
        for u in ["https://example.com/", "http://8.8.8.8/", "data:text/plain,hi"] {
            assert!(!is_private_network_url(&Url::parse(u).unwrap()), "{u}");
        }
    }

}
