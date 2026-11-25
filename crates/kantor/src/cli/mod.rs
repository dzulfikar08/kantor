//! Command-line tools that run without a window (`kantor convert …`,
//! `kantor pdf …`, `kantor slides …`, `kantor docs …`).

pub mod convert;
pub mod docs;
pub mod merge;
pub mod pdf;
pub mod render_png;
pub mod sheets;
pub mod slides;
pub mod slides_check;
