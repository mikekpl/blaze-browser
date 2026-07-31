//! URL sanitization, validation, and URL-vs-search disambiguation (FR-002, FR-006).

use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("empty input")]
    Empty,
    #[error("dangerous scheme rejected: {0}")]
    DangerousScheme(String),
    #[error("invalid url: {0}")]
    Invalid(String),
}

/// Schemes web content or the address bar must never navigate to directly.
const DANGEROUS_SCHEMES: &[&str] = &["javascript", "data", "file", "vbscript", "about"];

/// Schemes the address bar accepts verbatim.
const ALLOWED_SCHEMES: &[&str] = &["http", "https"];

/// Reject navigation to dangerous schemes (edge case: malformed/malicious URLs).
pub fn sanitize(url: &Url) -> Result<(), UrlError> {
    let scheme = url.scheme();
    if DANGEROUS_SCHEMES.contains(&scheme) {
        return Err(UrlError::DangerousScheme(scheme.to_owned()));
    }
    if !ALLOWED_SCHEMES.contains(&scheme) {
        return Err(UrlError::Invalid(format!("unsupported scheme: {scheme}")));
    }
    Ok(())
}

/// Resolve address-bar input into a navigable URL: direct URL when it parses
/// (and is safe), otherwise a search query against `search_template`
/// (`%s` is replaced with the percent-encoded query).
pub fn resolve_input(input: &str, search_template: &str) -> Result<Url, UrlError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UrlError::Empty);
    }

    // Explicit scheme → dangerous is rejected outright, http(s) accepted verbatim,
    // anything else (e.g. `localhost:8080` parsing as scheme "localhost") falls
    // through to the host heuristic.
    if let Ok(url) = Url::parse(input) {
        let scheme = url.scheme().to_owned();
        if DANGEROUS_SCHEMES.contains(&scheme.as_str()) {
            return Err(UrlError::DangerousScheme(scheme));
        }
        if ALLOWED_SCHEMES.contains(&scheme.as_str()) {
            return Ok(url);
        }
    }

    // Bare domain heuristic: no spaces and contains a dot or is localhost.
    let looks_like_host = !input.contains(char::is_whitespace)
        && (input.contains('.') || input.starts_with("localhost"));
    if looks_like_host {
        if let Ok(url) = Url::parse(&format!("https://{input}")) {
            if url.host_str().is_some() {
                return Ok(url);
            }
        }
    }

    // Everything else is a search.
    let query: String = url::form_urlencoded::byte_serialize(input.as_bytes()).collect();
    let target = search_template.replace("%s", &query);
    Url::parse(&target).map_err(|e| UrlError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "https://www.google.com/search?q=%s";

    #[test]
    fn direct_https_url_passes() {
        assert_eq!(
            resolve_input("https://example.com/a", G).unwrap().as_str(),
            "https://example.com/a"
        );
    }

    #[test]
    fn bare_domain_gets_https() {
        assert_eq!(
            resolve_input("example.com", G).unwrap().as_str(),
            "https://example.com/"
        );
    }

    #[test]
    fn phrase_becomes_search() {
        let url = resolve_input("rust browser engine", G).unwrap();
        assert!(
            url.as_str()
                .starts_with("https://www.google.com/search?q=rust")
        );
    }

    #[test]
    fn javascript_scheme_rejected() {
        assert!(matches!(
            resolve_input("javascript:alert(1)", G),
            Err(UrlError::DangerousScheme(_))
        ));
    }

    #[test]
    fn file_scheme_rejected() {
        assert!(matches!(
            resolve_input("file:///etc/passwd", G),
            Err(UrlError::DangerousScheme(_))
        ));
    }

    #[test]
    fn data_scheme_rejected() {
        assert!(matches!(
            resolve_input("data:text/html,<b>x</b>", G),
            Err(UrlError::DangerousScheme(_))
        ));
    }

    #[test]
    fn empty_input_rejected() {
        assert_eq!(resolve_input("   ", G), Err(UrlError::Empty));
    }

    #[test]
    fn single_word_is_search_not_host() {
        let url = resolve_input("weather", G).unwrap();
        assert!(url.as_str().contains("google.com/search"));
    }

    #[test]
    fn localhost_is_host() {
        assert_eq!(
            resolve_input("localhost:8080", G).unwrap().as_str(),
            "https://localhost:8080/"
        );
    }
}
