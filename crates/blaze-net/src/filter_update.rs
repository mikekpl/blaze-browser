//! Filter-list background updater with ETag caching (T021, FR-008).
//!
//! Runs on a background thread (never the UI/startup path — B1): fetch each
//! enabled list with `If-None-Match`, and only when something actually
//! changed does the caller stage a new engine build and swap it in.

use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected status: {0}")]
    Status(u16),
}

/// A fetched list body plus its validator for the next update cycle.
#[derive(Debug, Clone)]
pub struct FetchedList {
    pub body: String,
    pub etag: Option<String>,
}

/// Fetch `url`, honoring the previous `etag`. Returns `None` on 304
/// (unchanged) so callers can skip the expensive engine rebuild.
pub fn fetch_list_if_modified(
    url: &str,
    etag: Option<&str>,
) -> Result<Option<FetchedList>, FetchError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("Blaze/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut request = client.get(url);
    if let Some(tag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, tag);
    }
    let response = request.send()?;

    match response.status().as_u16() {
        304 => Ok(None),
        200 => {
            let new_etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            Ok(Some(FetchedList {
                body: response.text()?,
                etag: new_etag,
            }))
        }
        code => Err(FetchError::Status(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Minimal one-shot HTTP server for exercising the ETag protocol.
    fn serve_once(status_line: &'static str, headers: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "{status_line}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}/list.txt")
    }

    #[test]
    fn fetches_new_list_with_etag() {
        let url = serve_once(
            "HTTP/1.1 200 OK",
            "ETag: \"v1\"\r\n",
            "||ads.example.com^\n",
        );
        let fetched = fetch_list_if_modified(&url, None)
            .expect("fetch ok")
            .expect("modified");
        assert_eq!(fetched.body, "||ads.example.com^\n");
        assert_eq!(fetched.etag.as_deref(), Some("\"v1\""));
    }

    #[test]
    fn not_modified_returns_none() {
        let url = serve_once("HTTP/1.1 304 Not Modified", "", "");
        let result = fetch_list_if_modified(&url, Some("\"v1\"")).expect("fetch ok");
        assert!(result.is_none());
    }

    #[test]
    fn server_error_is_surfaced() {
        let url = serve_once("HTTP/1.1 500 Internal Server Error", "", "");
        assert!(matches!(
            fetch_list_if_modified(&url, None),
            Err(FetchError::Status(500))
        ));
    }
}
