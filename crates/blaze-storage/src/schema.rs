//! SQLite schema, WAL setup, and forward-only migrations
//! (contracts/storage-schema.md, `PRAGMA user_version`).

use std::path::Path;

use rusqlite::Connection;

use crate::StorageError;

const SCHEMA_V1: &str = r#"
CREATE TABLE bookmarks (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES bookmarks(id) ON DELETE CASCADE,
    is_folder   INTEGER NOT NULL DEFAULT 0,
    title       TEXT NOT NULL,
    url         TEXT,
    position    INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    CHECK ((is_folder = 1 AND url IS NULL) OR (is_folder = 0 AND url IS NOT NULL))
);
CREATE INDEX idx_bookmarks_parent ON bookmarks(parent_id, position);

CREATE TABLE history (
    id          INTEGER PRIMARY KEY,
    url         TEXT NOT NULL UNIQUE,
    title       TEXT NOT NULL DEFAULT '',
    visit_count INTEGER NOT NULL DEFAULT 1,
    last_visit  INTEGER NOT NULL
);
CREATE INDEX idx_history_last_visit ON history(last_visit);

CREATE TABLE downloads (
    id             TEXT PRIMARY KEY,
    source_url     TEXT NOT NULL,
    dest_path      TEXT NOT NULL,
    total_bytes    INTEGER,
    received_bytes INTEGER NOT NULL DEFAULT 0,
    state          TEXT NOT NULL CHECK (state IN
                     ('active','paused','completed','interrupted','cancelled')),
    etag           TEXT,
    last_modified  TEXT,
    created_at     INTEGER NOT NULL,
    completed_at   INTEGER
);
CREATE INDEX idx_downloads_state ON downloads(state, created_at);

CREATE TABLE session_snapshots (
    id          INTEGER PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    payload     TEXT NOT NULL
);

CREATE TABLE closed_tabs (
    id          INTEGER PRIMARY KEY,
    window_id   TEXT NOT NULL,
    payload     TEXT NOT NULL,
    closed_at   INTEGER NOT NULL
);

CREATE TABLE site_exceptions (
    host_pattern     TEXT PRIMARY KEY,
    blocking_enabled INTEGER NOT NULL,
    created_at       INTEGER NOT NULL
);

CREATE TABLE filter_lists (
    id           TEXT PRIMARY KEY,
    source_url   TEXT NOT NULL,
    etag         TEXT,
    version      TEXT,
    last_updated INTEGER,
    enabled      INTEGER NOT NULL DEFAULT 1,
    cached_path  TEXT NOT NULL
);
"#;

/// Forward-only migrations; index = from-version. Applied in a transaction:
/// a failed migration leaves the previous version intact.
const MIGRATIONS: &[&str] = &[SCHEMA_V1];

pub fn open_profile_db(path: &Path) -> Result<Connection, StorageError> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), StorageError> {
    loop {
        let version: usize =
            conn.query_row("SELECT user_version FROM pragma_user_version", [], |r| {
                r.get(0)
            })?;
        let Some(migration) = MIGRATIONS.get(version) else {
            return Ok(());
        };
        conn.execute_batch(&format!(
            "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
            version + 1
        ))?;
        tracing::info!(from = version, to = version + 1, "applied schema migration");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_migrates_to_latest() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_profile_db(&dir.path().join("profile.db")).unwrap();
        let v: usize = conn
            .query_row("SELECT user_version FROM pragma_user_version", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, MIGRATIONS.len());
        // All contract tables exist.
        for table in [
            "bookmarks",
            "history",
            "downloads",
            "session_snapshots",
            "closed_tabs",
            "site_exceptions",
            "filter_lists",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.db");
        drop(open_profile_db(&path).unwrap());
        drop(open_profile_db(&path).unwrap());
    }

    #[test]
    fn bookmark_check_constraint_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_profile_db(&dir.path().join("profile.db")).unwrap();
        // folder with a URL violates the CHECK
        let res = conn.execute(
            "INSERT INTO bookmarks (is_folder, title, url, position, created_at)
             VALUES (1, 'bad', 'https://x.com', 0, 0)",
            [],
        );
        assert!(res.is_err());
    }
}
