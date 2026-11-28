//! Command-line tools that run without a window (`kantor convert …`,
//! `kantor pdf …`, `kantor slides …`, `kantor docs …`, `kantor pls …`, `kantor agent run`).

pub mod agent;
pub mod convert;
pub mod docs;
mod docs_bench;
pub mod merge;
pub mod pdf;
pub mod pls;
pub mod render_png;
pub mod sheets;
pub mod slides;
pub mod slides_check;
