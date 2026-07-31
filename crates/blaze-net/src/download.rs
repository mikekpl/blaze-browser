//! Ranged, resumable download engine (FR-020..023, T046).
//!
//! Pure transport: HTTP ranged requests with `If-Range` validation, pause /
//! cancel via a shared control flag, and progress callbacks. State-machine
//! bookkeeping and persistence live in blaze-core / blaze-storage.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

/// Control flag values shared between the owner and the download loop.
pub const CONTROL_RUN: u8 = 0;
pub const CONTROL_PAUSE: u8 = 1;
pub const CONTROL_CANCEL: u8 = 2;

const CHUNK: usize = 64 * 1024;

/// Everything the transport needs to (re)start one download.
pub struct DownloadJob {
    pub url: String,
    pub dest_path: PathBuf,
    /// Bytes already on disk; > 0 requests a ranged continuation.
    pub resume_from: u64,
    /// Validators from the original response (FR-023).
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Server metadata captured from the (first) response.
#[derive(Debug, Clone, Default)]
pub struct ServerMeta {
    pub total_bytes: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// True when the server ignored our Range request and restarted at 0.
    pub restarted: bool,
}

/// Terminal result of one transport run.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    Completed,
    Paused,
    Cancelled,
    /// Network failure or early EOF; partial file kept for later resume.
    Interrupted(String),
}

/// Run the blocking download loop. `on_meta` fires once after headers are in;
/// `on_progress` fires after each written chunk with total bytes on disk.
pub fn run_download(
    job: &DownloadJob,
    control: &AtomicU8,
    mut on_meta: impl FnMut(&ServerMeta),
    mut on_progress: impl FnMut(u64),
) -> DownloadOutcome {
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(None) // large files must outlive the default 30s cap
        .build()
    {
        Ok(c) => c,
        Err(e) => return DownloadOutcome::Interrupted(format!("client: {e}")),
    };

    let validator = job.etag.as_deref().or(job.last_modified.as_deref());
    let want_resume = job.resume_from > 0 && validator.is_some();

    let mut req = client.get(&job.url);
    if want_resume {
        req = req
            .header("Range", format!("bytes={}-", job.resume_from))
            .header("If-Range", validator.unwrap_or_default());
    }

    let mut resp = match req.send() {
        Ok(r) => r,
        Err(e) => return DownloadOutcome::Interrupted(format!("request: {e}")),
    };
    let status = resp.status();
    if !status.is_success() {
        return DownloadOutcome::Interrupted(format!("http status {status}"));
    }

    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let resumed = want_resume && status.as_u16() == 206;
    let total_bytes = if resumed {
        // Content-Range: bytes <from>-<to>/<total>
        header("content-range")
            .and_then(|v| v.rsplit('/').next().and_then(|t| t.parse::<u64>().ok()))
    } else {
        resp.content_length()
    };
    let meta = ServerMeta {
        total_bytes,
        etag: header("etag"),
        last_modified: header("last-modified"),
        restarted: want_resume && !resumed,
    };
    on_meta(&meta);

    let mut received: u64 = if resumed { job.resume_from } else { 0 };
    let mut file = {
        let open = if resumed {
            OpenOptions::new().append(true).open(&job.dest_path)
        } else {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&job.dest_path)
        };
        match open {
            Ok(f) => f,
            Err(e) => return DownloadOutcome::Interrupted(format!("open dest: {e}")),
        }
    };

    let mut buf = vec![0u8; CHUNK];
    loop {
        match control.load(Ordering::Relaxed) {
            CONTROL_PAUSE => {
                let _ = file.flush();
                return DownloadOutcome::Paused;
            }
            CONTROL_CANCEL => {
                drop(file);
                let _ = fs::remove_file(&job.dest_path); // partial deleted on cancel
                return DownloadOutcome::Cancelled;
            }
            _ => {}
        }
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = file.flush();
                return DownloadOutcome::Interrupted(format!("read: {e}"));
            }
        };
        if let Err(e) = file.write_all(&buf[..n]) {
            return DownloadOutcome::Interrupted(format!("write: {e}"));
        }
        received += n as u64;
        on_progress(received);
    }

    let _ = file.flush();
    if let Some(total) = meta.total_bytes
        && received < total
    {
        return DownloadOutcome::Interrupted(format!(
            "connection closed early ({received}/{total} bytes)"
        ));
    }
    DownloadOutcome::Completed
}

/// Best-effort filename from a URL's final path segment.
pub fn suggested_name_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let segment = parsed.path_segments()?.next_back()?.trim();
    if segment.is_empty() {
        return None;
    }
    let decoded = percent_decode(segment);
    Some(sanitize_filename(&decoded))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reduce a server-suggested filename to a single safe path component
/// (path-traversal guard, FR data-model: dest stays inside download dir).
pub fn sanitize_filename(suggested: &str) -> String {
    let last = suggested
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(suggested)
        .trim();
    let mut name: String = last
        .chars()
        .filter(|c| !c.is_control() && *c != ':' && *c != '\0')
        .collect();
    // Never allow dot-only names or hidden files from remote input.
    while name.starts_with('.') {
        name.remove(0);
    }
    let name = name.trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() {
        return "download".to_string();
    }
    // Cap length, preserving the extension where possible.
    if name.len() > 200 {
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let mut stem: String = name.chars().take(150).collect();
        if !ext.is_empty() {
            stem.push('.');
            stem.push_str(ext);
        }
        return stem;
    }
    name
}

/// First non-colliding path for `name` inside `dir`: "name.ext",
/// "name (2).ext", ... Also avoids clashing with in-flight `.part` siblings.
pub fn collision_safe_path(dir: &Path, name: &str) -> PathBuf {
    let safe = sanitize_filename(name);
    let stem = Path::new(&safe)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    let ext = Path::new(&safe)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_owned);

    for n in 1u32.. {
        let candidate = if n == 1 {
            safe.clone()
        } else {
            match &ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            }
        };
        let path = dir.join(&candidate);
        if !path.exists() && !dir.join(format!("{candidate}.part")).exists() {
            debug_assert_eq!(path.parent(), Some(dir));
            return path;
        }
    }
    unreachable!("u32 exhausted finding a free filename")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal_and_separators() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(sanitize_filename("/tmp/x/report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename(""), "download");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("a:b\u{0}c.txt"), "abc.txt");
    }

    #[test]
    fn sanitize_caps_length_keeps_extension() {
        let long = format!("{}.tar.gz", "x".repeat(400));
        let out = sanitize_filename(&long);
        assert!(out.len() <= 200);
        assert!(out.ends_with(".gz"));
    }

    #[test]
    fn suggested_name_comes_from_last_segment() {
        assert_eq!(
            suggested_name_from_url("https://x.com/a/b/report%20final.pdf?dl=1"),
            Some("report final.pdf".to_string())
        );
        assert_eq!(suggested_name_from_url("https://x.com/"), None);
        assert_eq!(suggested_name_from_url("not a url"), None);
    }

    #[test]
    fn collision_safe_appends_counter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("file (2).txt"), b"x").unwrap();
        let p = collision_safe_path(dir.path(), "file.txt");
        assert_eq!(p, dir.path().join("file (3).txt"));

        std::fs::write(dir.path().join("noext"), b"x").unwrap();
        assert_eq!(
            collision_safe_path(dir.path(), "noext"),
            dir.path().join("noext (2)")
        );
    }

    #[test]
    fn collision_safe_respects_part_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("video.mp4.part"), b"x").unwrap();
        assert_eq!(
            collision_safe_path(dir.path(), "video.mp4"),
            dir.path().join("video (2).mp4")
        );
    }
}
