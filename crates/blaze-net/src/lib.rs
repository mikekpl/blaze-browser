//! Networking concerns owned by the core: URL handling now; downloads and
//! filter-list updates are added in later phases. Page-content networking
//! belongs to the active engine backend (see research.md R6).

pub mod download;
pub mod filter_update;
pub mod url;
