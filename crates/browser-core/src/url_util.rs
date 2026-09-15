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
}
