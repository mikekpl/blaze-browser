//! Bookmark/folder CRUD on the `bookmarks` table (T053, FR-024..026).
//! Folders are rows with `is_folder = 1` and NULL url. Hierarchy is kept
//! acyclic at move time; siblings keep dense `position` ordering.

use rusqlite::{Connection, OptionalExtension, params};

use crate::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BookmarkNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub is_folder: bool,
    pub title: String,
    pub url: Option<String>,
    pub position: i64,
    pub created_at: i64,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parent must be NULL (root) or an existing folder row.
fn validate_parent(conn: &Connection, parent_id: Option<i64>) -> Result<(), StorageError> {
    let Some(pid) = parent_id else {
        return Ok(());
    };
    let is_folder: Option<bool> = conn
        .query_row(
            "SELECT is_folder FROM bookmarks WHERE id = ?1",
            params![pid],
            |r| r.get(0),
        )
        .optional()?;
    match is_folder {
        Some(true) => Ok(()),
        Some(false) => Err(StorageError::Invalid(format!(
            "parent {pid} is not a folder"
        ))),
        None => Err(StorageError::Invalid(format!("parent {pid} not found"))),
    }
}

fn next_position(conn: &Connection, parent_id: Option<i64>) -> Result<i64, StorageError> {
    let pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM bookmarks
         WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    Ok(pos)
}

/// Insert a bookmark leaf; returns the new row id.
pub fn insert_bookmark(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
    url: &str,
) -> Result<i64, StorageError> {
    if url.is_empty() {
        return Err(StorageError::Invalid("bookmark url is empty".into()));
    }
    validate_parent(conn, parent_id)?;
    let pos = next_position(conn, parent_id)?;
    conn.execute(
        "INSERT INTO bookmarks (parent_id, is_folder, title, url, position, created_at)
         VALUES (?1, 0, ?2, ?3, ?4, ?5)",
        params![parent_id, title, url, pos, now_epoch()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a folder; returns the new row id.
pub fn insert_folder(
    conn: &Connection,
    parent_id: Option<i64>,
    title: &str,
) -> Result<i64, StorageError> {
    validate_parent(conn, parent_id)?;
    let pos = next_position(conn, parent_id)?;
    conn.execute(
        "INSERT INTO bookmarks (parent_id, is_folder, title, url, position, created_at)
         VALUES (?1, 1, ?2, NULL, ?3, ?4)",
        params![parent_id, title, pos, now_epoch()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Edit title and/or url (url ignored for folders).
pub fn update_node(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    url: Option<&str>,
) -> Result<(), StorageError> {
    let n = conn.execute(
        "UPDATE bookmarks SET
           title = COALESCE(?2, title),
           url = CASE WHEN is_folder = 1 THEN NULL ELSE COALESCE(?3, url) END
         WHERE id = ?1",
        params![id, title, url],
    )?;
    if n == 0 {
        return Err(StorageError::Invalid(format!("bookmark {id} not found")));
    }
    Ok(())
}

/// Delete a node; folders cascade to their descendants (FK ON DELETE CASCADE).
pub fn delete_node(conn: &Connection, id: i64) -> Result<(), StorageError> {
    let n = conn.execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
    if n == 0 {
        return Err(StorageError::Invalid(format!("bookmark {id} not found")));
    }
    Ok(())
}

/// True when `candidate` is `node` itself or one of its descendants
/// (walking up from `candidate` reaches `node`).
fn would_create_cycle(
    conn: &Connection,
    node: i64,
    candidate: Option<i64>,
) -> Result<bool, StorageError> {
    let Some(start) = candidate else {
        return Ok(false);
    };
    let hit: Option<i64> = conn
        .query_row(
            "WITH RECURSIVE anc(id) AS (
               SELECT ?1
               UNION ALL
               SELECT b.parent_id FROM bookmarks b JOIN anc ON b.id = anc.id
               WHERE b.parent_id IS NOT NULL
             )
             SELECT 1 FROM anc WHERE id = ?2 LIMIT 1",
            params![start, node],
            |r| r.get(0),
        )
        .optional()?;
    Ok(hit.is_some())
}

/// Move `id` under `new_parent` at sibling index `position` (clamped).
/// Rejects moves that would create a cycle (data-model invariant).
pub fn move_node(
    conn: &Connection,
    id: i64,
    new_parent: Option<i64>,
    position: i64,
) -> Result<(), StorageError> {
    if Some(id) == new_parent || would_create_cycle(conn, id, new_parent)? {
        return Err(StorageError::Invalid(
            "move would create a bookmark cycle".into(),
        ));
    }
    validate_parent(conn, new_parent)?;
    let old_parent: Option<i64> = conn
        .query_row(
            "SELECT parent_id FROM bookmarks WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::Invalid(format!("bookmark {id} not found")))?;

    let sibling_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM bookmarks WHERE parent_id IS ?1 AND id != ?2",
        params![new_parent, id],
        |r| r.get(0),
    )?;
    let target = position.clamp(0, sibling_count);

    // Make room in the target, place the node, then compact the old parent.
    conn.execute(
        "UPDATE bookmarks SET position = position + 1
         WHERE parent_id IS ?1 AND position >= ?2 AND id != ?3",
        params![new_parent, target, id],
    )?;
    conn.execute(
        "UPDATE bookmarks SET parent_id = ?2, position = ?3 WHERE id = ?1",
        params![id, new_parent, target],
    )?;
    resequence(conn, old_parent)?;
    resequence(conn, new_parent)?;
    Ok(())
}

/// Rewrite sibling positions as a dense 0..n sequence.
fn resequence(conn: &Connection, parent: Option<i64>) -> Result<(), StorageError> {
    conn.execute(
        "WITH ordered AS (
           SELECT id, ROW_NUMBER() OVER (ORDER BY position, id) - 1 AS new_pos
           FROM bookmarks WHERE parent_id IS ?1
         )
         UPDATE bookmarks SET position = (SELECT new_pos FROM ordered WHERE ordered.id = bookmarks.id)
         WHERE parent_id IS ?1",
        params![parent],
    )?;
    Ok(())
}

/// Every node, ordered parent-first by (parent, position) for tree assembly.
pub fn all_nodes(conn: &Connection) -> Result<Vec<BookmarkNode>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, is_folder, title, url, position, created_at
         FROM bookmarks ORDER BY parent_id NULLS FIRST, position",
    )?;
    let rows = stmt.query_map([], row_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Case-insensitive substring search over title and URL (FR-026).
pub fn search(conn: &Connection, query: &str) -> Result<Vec<BookmarkNode>, StorageError> {
    let pattern = format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, is_folder, title, url, position, created_at
         FROM bookmarks
         WHERE title LIKE ?1 ESCAPE '\\' OR url LIKE ?1 ESCAPE '\\'
         ORDER BY is_folder DESC, title COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![pattern], row_from)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkNode> {
    Ok(BookmarkNode {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        is_folder: r.get(2)?,
        title: r.get(3)?,
        url: r.get(4)?,
        position: r.get(5)?,
        created_at: r.get(6)?,
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

    #[test]
    fn crud_and_sibling_ordering() {
        let (c, _d) = conn();
        let folder = insert_folder(&c, None, "News").unwrap();
        let a = insert_bookmark(&c, Some(folder), "A", "https://a.com").unwrap();
        let b = insert_bookmark(&c, Some(folder), "B", "https://b.com").unwrap();

        let all = all_nodes(&c).unwrap();
        assert_eq!(all.len(), 3);
        let positions: Vec<_> = all
            .iter()
            .filter(|n| n.parent_id == Some(folder))
            .map(|n| (n.id, n.position))
            .collect();
        assert_eq!(positions, vec![(a, 0), (b, 1)]);

        update_node(&c, a, Some("A2"), Some("https://a2.com")).unwrap();
        let node = all_nodes(&c)
            .unwrap()
            .into_iter()
            .find(|n| n.id == a)
            .unwrap();
        assert_eq!(node.title, "A2");
        assert_eq!(node.url.as_deref(), Some("https://a2.com"));

        // Folder delete cascades to children.
        delete_node(&c, folder).unwrap();
        assert!(all_nodes(&c).unwrap().is_empty());
    }

    #[test]
    fn moves_reorder_and_reject_cycles() {
        let (c, _d) = conn();
        let outer = insert_folder(&c, None, "Outer").unwrap();
        let inner = insert_folder(&c, Some(outer), "Inner").unwrap();
        let x = insert_bookmark(&c, None, "X", "https://x.com").unwrap();

        // Reorder at root: move x before outer.
        move_node(&c, x, None, 0).unwrap();
        let roots: Vec<_> = all_nodes(&c)
            .unwrap()
            .into_iter()
            .filter(|n| n.parent_id.is_none())
            .map(|n| n.id)
            .collect();
        assert_eq!(roots, vec![x, outer]);

        // Move into a nested folder.
        move_node(&c, x, Some(inner), 5).unwrap();
        let node = all_nodes(&c)
            .unwrap()
            .into_iter()
            .find(|n| n.id == x)
            .unwrap();
        assert_eq!(node.parent_id, Some(inner));
        assert_eq!(node.position, 0); // clamped

        // Cycle attempts are rejected.
        assert!(move_node(&c, outer, Some(inner), 0).is_err());
        assert!(move_node(&c, outer, Some(outer), 0).is_err());
        // Non-folder parents too.
        assert!(insert_bookmark(&c, Some(x), "Y", "https://y.com").is_err());
    }

    #[test]
    fn search_matches_title_and_url() {
        let (c, _d) = conn();
        insert_bookmark(&c, None, "Rust Blog", "https://blog.rust-lang.org").unwrap();
        insert_bookmark(&c, None, "News", "https://example.com/rust").unwrap();
        insert_bookmark(&c, None, "Other", "https://other.org").unwrap();
        insert_folder(&c, None, "Rust stuff").unwrap();

        let hits = search(&c, "rust").unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits[0].is_folder); // folders sort first
        assert!(search(&c, "100%").unwrap().is_empty()); // escaped wildcard
    }
}
