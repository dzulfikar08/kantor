//! The grid's table: a fork of GPUI Kit's `DataTable` (`gpui-component`
//! 0.9.0, `src/table/`, Apache-2.0, see `LICENSE-APACHE` here), kept as close
//! to upstream as possible so it can be compared with new releases. Upstream
//! lays rows out with `uniform_list`, one height for every row; Kantor's
//! changes (marked `KANTOR PATCH`) give each row its own height. What was
//! removed: the stateless `Table` component and upstream's tests (they need
//! `gpui_component::init`); `t!` strings became English.

use gpui::App;

mod column;
mod data_table;
mod delegate;
mod loading;
mod state;

pub use column::*;
pub use data_table::*;
pub use delegate::*;
pub use state::*;

pub(crate) fn init(cx: &mut App) {
    data_table::init(cx);
}

/// Whether measurement logging is enabled, read once per process.
fn measure_enable() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(gpui_kit::base::measurement_enabled);
    *ENABLED
}
