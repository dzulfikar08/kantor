//! Kantor's design system.
//!
//! The rule (borrowed from Voltrus Tools): **views never use raw colours or sizes**.
//! Sizes come from [`metrics`], colours from [`tokens`], which are derived from the
//! Omarchy palette and also mapped onto GPUI Kit's theme so its components match.

pub mod cached;
pub mod components;
pub mod fonts;
pub mod metrics;
pub mod tokens;

pub use cached::CachedView;
pub use tokens::{Tokens, tokens};
