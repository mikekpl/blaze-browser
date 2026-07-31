//! Ad/tracker blocking built on Brave's adblock-rust (research.md R2).
//! Modules: engine wrapper (DAT-cached), WebKit rule compilation, cosmetic
//! filters/scriptlets, shield stats, and site exceptions.

pub mod cosmetic;
pub mod engine;
pub mod shields;
pub mod webkit_rules;

pub use engine::{AdblockEngine, AdblockError, BlockDecision, CosmeticPayload};
pub use shields::{BlockKind, ShieldCounters, ShieldStats};
