//! Session persistence (T035/T036): session_snapshots (keep last 3) and the
//! closed_tabs ring (25, FR-015). Payloads are opaque JSON owned by
//! blaze-core; writes ride the single writer thread.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Profile, StorageError};

pub const SNAPSHOTS_KEPT: usize = 3;
pub const CLOSED_TABS_KEPT: usize = 25;

/// Queue a snapshot write, pruning to the newest `SNAPSHOTS_KEPT`.
pub fn write_snapshot(profile: &Profile, payload_json: String) {
    profile.writer().submit(move |conn| {
        let now = now_secs();
        let result = conn
            .execute(
                "INSERT INTO session_snapshots (created_at, payload) VALUES (?1, ?2)",
                params![now, payload_json],
            )
            .and_then(|_| {
                conn.execute(
                    "DELETE FROM session_snapshots WHERE id NOT IN
                     (SELECT id FROM session_snapshots ORDER BY id DESC LIMIT ?1)",
                    params![SNAPSHOTS_KEPT as i64],
                )
            });
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to write session snapshot");
        }
    });
}

/// Most recent snapshot payload, if any.
pub fn latest_snapshot(conn: &Connection) -> Result<Option<String>, StorageError> {
    let row = conn
        .query_row(
            "SELECT payload FROM session_snapshots ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row)
}

/// Queue a closed-tab record, trimming the ring to `CLOSED_TABS_KEPT`.
pub fn push_closed_tab(profile: &Profile, window_id: String, payload_json: String) {
    profile.writer().submit(move |conn| {
        let now = now_secs();
        let result = conn
            .execute(
                "INSERT INTO closed_tabs (window_id, payload, closed_at) VALUES (?1, ?2, ?3)",
                params![window_id, payload_json, now],
            )
            .and_then(|_| {
                conn.execute(
                    "DELETE FROM closed_tabs WHERE id NOT IN
                     (SELECT id FROM closed_tabs ORDER BY id DESC LIMIT ?1)",
                    params![CLOSED_TABS_KEPT as i64],
                )
            });
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to persist closed tab");
        }
    });
}

/// Remove the newest closed-tab row (consumed by reopen), preferring `window_id`.
pub fn pop_closed_tab(profile: &Profile, window_id: String) {
    profile.writer().submit(move |conn| {
        let result = conn.execute(
            "DELETE FROM closed_tabs WHERE id =
             COALESCE((SELECT id FROM closed_tabs WHERE window_id = ?1 ORDER BY id DESC LIMIT 1),
                      (SELECT id FROM closed_tabs ORDER BY id DESC LIMIT 1))",
            params![window_id],
        );
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to consume closed tab");
        }
    });
}

/// All persisted closed tabs, oldest first (for ring reload at startup).
pub fn load_closed_tabs(conn: &Connection) -> Result<Vec<(String, String)>, StorageError> {
    let mut stmt = conn.prepare("SELECT window_id, payload FROM closed_tabs ORDER BY id ASC")?;
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
    fn snapshots_prune_to_last_three() {
        let (_d, p) = profile();
        for i in 0..5 {
            write_snapshot(&p, format!("{{\"n\":{i}}}"));
        }
        p.flush();
        let conn = p.read_conn().expect("conn");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 3);
        assert_eq!(
            latest_snapshot(&conn).expect("query").as_deref(),
            Some(r#"{"n":4}"#)
        );
    }

    #[test]
    fn closed_tabs_ring_trims_and_pops() {
        let (_d, p) = profile();
        for i in 0..30 {
            push_closed_tab(&p, "w1".into(), format!("{{\"n\":{i}}}"));
        }
        pop_closed_tab(&p, "w1".into());
        p.flush();
        let conn = p.read_conn().expect("conn");
        let rows = load_closed_tabs(&conn).expect("load");
        assert_eq!(rows.len(), CLOSED_TABS_KEPT - 1);
        assert_eq!(rows.last().expect("last").1, r#"{"n":28}"#);
    }
}
