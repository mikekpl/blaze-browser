//! Per-site blocking exceptions (site_exceptions table, FR-010).
//!
//! Writes go through the single writer thread; reads use a WAL read
//! connection. Host patterns are exact hosts ("news.site") — shields lookup
//! walks parent domains so an exception on a domain covers its subdomains.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Profile, StorageError};

/// Persist (or update) an exception: `blocking_enabled = false` disables
/// shields for the host.
pub fn set_site_exception(profile: &Profile, host: &str, blocking_enabled: bool) {
    let host = host.to_ascii_lowercase();
    profile.writer().submit(move |conn| {
        let now = now_secs();
        if let Err(e) = conn.execute(
            "INSERT INTO site_exceptions (host_pattern, blocking_enabled, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(host_pattern) DO UPDATE SET blocking_enabled = ?2",
            params![host, blocking_enabled, now],
        ) {
            tracing::error!(error = %e, "failed to persist site exception");
        }
    });
}

/// Remove an exception, restoring the global default for the host.
pub fn clear_site_exception(profile: &Profile, host: &str) {
    let host = host.to_ascii_lowercase();
    profile.writer().submit(move |conn| {
        if let Err(e) = conn.execute(
            "DELETE FROM site_exceptions WHERE host_pattern = ?1",
            params![host],
        ) {
            tracing::error!(error = %e, "failed to clear site exception");
        }
    });
}

/// Is blocking enabled for `host`? Checks the host and each parent domain;
/// defaults to `true` (shields on) when no exception exists (FR-006).
pub fn blocking_enabled_for(conn: &Connection, host: &str) -> Result<bool, StorageError> {
    let host = host.to_ascii_lowercase();
    let mut candidate = host.as_str();
    loop {
        let row: Option<bool> = conn
            .query_row(
                "SELECT blocking_enabled FROM site_exceptions WHERE host_pattern = ?1",
                params![candidate],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(enabled) = row {
            return Ok(enabled);
        }
        match candidate.split_once('.') {
            // Stop at the registrable-ish boundary (require one more dot).
            Some((_, rest)) if rest.contains('.') => candidate = rest,
            _ => return Ok(true),
        }
    }
}

/// All exceptions (for a future settings UI).
pub fn list_site_exceptions(conn: &Connection) -> Result<Vec<(String, bool)>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT host_pattern, blocking_enabled FROM site_exceptions ORDER BY host_pattern",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> (tempfile::TempDir, Profile) {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profile::open(dir.path()).expect("profile opens");
        (dir, profile)
    }

    #[test]
    fn default_is_blocking_on() {
        let (_d, p) = profile();
        let conn = p.read_conn().expect("read conn");
        assert!(blocking_enabled_for(&conn, "example.com").expect("query"));
    }

    #[test]
    fn exception_persists_and_covers_subdomains() {
        let (_d, p) = profile();
        set_site_exception(&p, "News.Site", false);
        p.flush();
        let conn = p.read_conn().expect("read conn");
        assert!(!blocking_enabled_for(&conn, "news.site").expect("query"));
        assert!(!blocking_enabled_for(&conn, "cdn.news.site").expect("query"));
        assert!(blocking_enabled_for(&conn, "other.site").expect("query"));
    }

    #[test]
    fn clear_restores_default() {
        let (_d, p) = profile();
        set_site_exception(&p, "news.site", false);
        clear_site_exception(&p, "news.site");
        p.flush();
        let conn = p.read_conn().expect("read conn");
        assert!(blocking_enabled_for(&conn, "news.site").expect("query"));
    }
}
