//! Bookmark service (T054, FR-024..026): synchronous CRUD through the
//! storage writer (bookmarks are low-frequency), nested tree assembly,
//! and title/URL search.

use std::collections::HashMap;

use blaze_storage::bookmarks::{self as db, BookmarkNode};

use crate::{BlazeCore, CoreError};

/// One node of the nested bookmarks tree returned to shells.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BookmarkTreeNode {
    pub id: i64,
    pub is_folder: bool,
    pub title: String,
    pub url: Option<String>,
    pub children: Vec<BookmarkTreeNode>,
}

fn build_tree(nodes: Vec<BookmarkNode>) -> Vec<BookmarkTreeNode> {
    let mut children_of: HashMap<Option<i64>, Vec<BookmarkNode>> = HashMap::new();
    for node in nodes {
        children_of.entry(node.parent_id).or_default().push(node);
    }
    fn assemble(
        parent: Option<i64>,
        children_of: &mut HashMap<Option<i64>, Vec<BookmarkNode>>,
    ) -> Vec<BookmarkTreeNode> {
        let mut level = children_of.remove(&parent).unwrap_or_default();
        level.sort_by_key(|n| n.position);
        level
            .into_iter()
            .map(|n| {
                let children = if n.is_folder {
                    assemble(Some(n.id), children_of)
                } else {
                    Vec::new()
                };
                BookmarkTreeNode {
                    id: n.id,
                    is_folder: n.is_folder,
                    title: n.title,
                    url: n.url,
                    children,
                }
            })
            .collect()
    }
    assemble(None, &mut children_of)
}

impl BlazeCore {
    /// Add a bookmark leaf; returns its id (FR-024).
    pub fn add_bookmark(
        &self,
        parent_id: Option<i64>,
        title: &str,
        url: &str,
    ) -> Result<i64, CoreError> {
        let (title, url) = (title.to_string(), url.to_string());
        let id = self
            .profile
            .write_sync(move |c| db::insert_bookmark(c, parent_id, &title, &url))??;
        Ok(id)
    }

    /// Create a folder; returns its id (FR-025).
    pub fn create_bookmark_folder(
        &self,
        parent_id: Option<i64>,
        title: &str,
    ) -> Result<i64, CoreError> {
        let title = title.to_string();
        let id = self
            .profile
            .write_sync(move |c| db::insert_folder(c, parent_id, &title))??;
        Ok(id)
    }

    /// Edit title and/or URL of a bookmark or folder.
    pub fn edit_bookmark(
        &self,
        id: i64,
        title: Option<&str>,
        url: Option<&str>,
    ) -> Result<(), CoreError> {
        let (title, url) = (title.map(str::to_owned), url.map(str::to_owned));
        self.profile
            .write_sync(move |c| db::update_node(c, id, title.as_deref(), url.as_deref()))??;
        Ok(())
    }

    /// Delete a bookmark, or a folder together with its descendants.
    pub fn delete_bookmark(&self, id: i64) -> Result<(), CoreError> {
        self.profile.write_sync(move |c| db::delete_node(c, id))??;
        Ok(())
    }

    /// Move/reorder a node; rejects cycle-creating moves (data-model rule).
    pub fn move_bookmark(
        &self,
        id: i64,
        new_parent: Option<i64>,
        position: i64,
    ) -> Result<(), CoreError> {
        self.profile
            .write_sync(move |c| db::move_node(c, id, new_parent, position))??;
        Ok(())
    }

    /// Full nested tree, siblings in stored order.
    pub fn bookmarks_tree(&self) -> Result<Vec<BookmarkTreeNode>, CoreError> {
        self.profile.flush();
        let conn = self.profile.read_conn()?;
        Ok(build_tree(db::all_nodes(&conn)?))
    }

    /// Case-insensitive title/URL search, folders first (FR-026).
    pub fn search_bookmarks(&self, query: &str) -> Result<Vec<BookmarkNode>, CoreError> {
        self.profile.flush();
        let conn = self.profile.read_conn()?;
        Ok(db::search(&conn, query)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, EventSink};

    struct NullSink;
    impl EventSink for NullSink {
        fn on_events(&self, _: Vec<Event>) {}
    }

    fn core() -> (BlazeCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap(), dir)
    }

    #[test]
    fn tree_nests_and_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let core = BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap();
            let folder = core.create_bookmark_folder(None, "Work").unwrap();
            core.add_bookmark(Some(folder), "Docs", "https://docs.rs")
                .unwrap();
            core.add_bookmark(None, "Home", "https://example.com")
                .unwrap();
            core.shutdown();
        }
        let core = BlazeCore::new(dir.path(), Box::new(NullSink)).unwrap();
        let tree = core.bookmarks_tree().unwrap();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "Work");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].url.as_deref(), Some("https://docs.rs"));
        assert_eq!(tree[1].title, "Home");
    }

    #[test]
    fn search_and_invalid_moves_surface_errors() {
        let (core, _dir) = core();
        let folder = core.create_bookmark_folder(None, "F").unwrap();
        let leaf = core
            .add_bookmark(Some(folder), "Rust", "https://rust-lang.org")
            .unwrap();

        let hits = core.search_bookmarks("rust").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, leaf);

        assert!(core.move_bookmark(folder, Some(folder), 0).is_err());
        assert!(core.add_bookmark(Some(leaf), "x", "https://x.com").is_err());
        core.move_bookmark(leaf, None, 0).unwrap();
        assert_eq!(core.bookmarks_tree().unwrap()[0].id, leaf);
    }
}
