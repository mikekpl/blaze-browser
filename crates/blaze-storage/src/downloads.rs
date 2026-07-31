//! Persistence for the `downloads` table (T047, data-model.md §Download).
//! Write helpers take `&mut Connection` so they run on the writer thread;
//! read helpers run on any WAL reader connection.

use rusqlite::{Connection, OptionalExtension, Result, params};

/// One row of the `downloads` table.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DownloadRow {
    pub id: String,
    pub source_url: String,
    pub dest_path: String,
    pub total_bytes: Option<u64>,
    pub received_bytes: u64,
    pub state: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

pub fn upsert(conn: &Connection, row: &DownloadRow) -> Result<()> {
    conn.execute(
        "INSERT INTO downloads (id, source_url, dest_path, total_bytes, received_bytes,
                                state, etag, last_modified, created_at, completed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
           source_url = excluded.source_url,
           dest_path = excluded.dest_path,
           total_bytes = excluded.total_bytes,
           received_bytes = excluded.received_bytes,
           state = excluded.state,
           etag = excluded.etag,
           last_modified = excluded.last_modified,
           completed_at = excluded.completed_at",
        params![
            row.id,
            row.source_url,
            row.dest_path,
            row.total_bytes,
            row.received_bytes,
            row.state,
            row.etag,
            row.last_modified,
            row.created_at,
            row.completed_at,
        ],
    )?;
    Ok(())
}

/// Progress tick: bytes plus (idempotently) server validators.
pub fn update_progress(
    conn: &Connection,
    id: &str,
    received_bytes: u64,
    total_bytes: Option<u64>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE downloads SET received_bytes = ?2,
                total_bytes = COALESCE(?3, total_bytes),
                etag = COALESCE(?4, etag),
                last_modified = COALESCE(?5, last_modified)
         WHERE id = ?1",
        params![id, received_bytes, total_bytes, etag, last_modified],
    )?;
    Ok(())
}

pub fn set_state(
    conn: &Connection,
    id: &str,
    state: &str,
    completed_at: Option<i64>,
) -> Result<()> {
    conn.execute(
        "UPDATE downloads SET state = ?2, completed_at = ?3 WHERE id = ?1",
        params![id, state, completed_at],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
    Ok(())
}

/// Crash recovery (T048): rows still marked `active` at launch were
/// interrupted by the previous process; returns the affected ids.
pub fn mark_stale_active_as_interrupted(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT id FROM downloads WHERE state = 'active' ORDER BY created_at")?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    conn.execute(
        "UPDATE downloads SET state = 'interrupted' WHERE state = 'active'",
        [],
    )?;
    Ok(ids)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<DownloadRow>> {
    conn.query_row(
        "SELECT id, source_url, dest_path, total_bytes, received_bytes,
                state, etag, last_modified, created_at, completed_at
         FROM downloads WHERE id = ?1",
        params![id],
        row_from,
    )
    .optional()
}

/// All downloads, newest first (downloads UI history list).
pub fn list(conn: &Connection) -> Result<Vec<DownloadRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_url, dest_path, total_bytes, received_bytes,
                state, etag, last_modified, created_at, completed_at
         FROM downloads ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt.query_map([], row_from)?;
    rows.collect()
}

fn row_from(r: &rusqlite::Row<'_>) -> Result<DownloadRow> {
    Ok(DownloadRow {
        id: r.get(0)?,
        source_url: r.get(1)?,
        dest_path: r.get(2)?,
        total_bytes: r.get(3)?,
        received_bytes: r.get(4)?,
        state: r.get(5)?,
        etag: r.get(6)?,
        last_modified: r.get(7)?,
        created_at: r.get(8)?,
        completed_at: r.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::open_profile_db;

    fn conn() -> (Connection, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let c = open_profile_db(&dir.path().join("profile.db")).unwrap();
        (c, dir)
    }

    fn row(id: &str, state: &str, created_at: i64) -> DownloadRow {
        DownloadRow {
            id: id.into(),
            source_url: "https://example.com/f.bin".into(),
            dest_path: "/tmp/f.bin".into(),
            total_bytes: Some(1000),
            received_bytes: 0,
            state: state.into(),
            etag: None,
            last_modified: None,
            created_at,
            completed_at: None,
        }
    }

    #[test]
    fn upsert_progress_and_list_roundtrip() {
        let (c, _d) = conn();
        upsert(&c, &row("a", "active", 1)).unwrap();
        upsert(&c, &row("b", "completed", 2)).unwrap();
        update_progress(&c, "a", 512, None, Some("\"tag\""), None).unwrap();

        let all = list(&c).unwrap();
        assert_eq!(
            all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        let a = get(&c, "a").unwrap().unwrap();
        assert_eq!(a.received_bytes, 512);
        assert_eq!(a.etag.as_deref(), Some("\"tag\""));
        assert_eq!(a.total_bytes, Some(1000)); // COALESCE kept prior total

        set_state(&c, "a", "paused", None).unwrap();
        assert_eq!(get(&c, "a").unwrap().unwrap().state, "paused");
        delete(&c, "a").unwrap();
        assert!(get(&c, "a").unwrap().is_none());
    }

    #[test]
    fn stale_active_rows_become_interrupted() {
        let (c, _d) = conn();
        upsert(&c, &row("a", "active", 1)).unwrap();
        upsert(&c, &row("b", "paused", 2)).unwrap();
        upsert(&c, &row("c", "active", 3)).unwrap();
        let ids = mark_stale_active_as_interrupted(&c).unwrap();
        assert_eq!(ids, vec!["a".to_string(), "c".to_string()]);
        assert_eq!(get(&c, "a").unwrap().unwrap().state, "interrupted");
        assert_eq!(get(&c, "b").unwrap().unwrap().state, "paused");
    }
}
