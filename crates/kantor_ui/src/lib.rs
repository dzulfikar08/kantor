//! Kantor's design system.
//!
//! The rule (borrowed from Voltrus Tools): **views never use raw colours or sizes**.
//! Sizes come from [`metrics`], colours from [`tokens`], which are derived from the
//! Omarchy palette and also mapped onto GPUI Kit's theme so its components match.

pub mod ai;
pub mod assets;
pub mod cached;
pub mod components;
pub mod disk_guard;
pub mod find_bar;
pub mod fonts;
pub mod i18n;
pub mod keys;
pub mod metrics;
pub mod perf;
pub mod read_only;
pub mod spelling;
pub mod tokens;

pub use assets::KantorAssets;
pub use cached::CachedView;
pub use tokens::{Tokens, tokens};
