//! Kantor Sheets' object model for macros: `Application`, `Workbook`,
//! `Worksheets`, `Worksheet`, `Range`, `Font`, `Interior`, `Borders`,
//! `WorksheetFunction`… as [`kantor_script::Dispatch`] objects over a
//! [`sheets_engine::Workbook`], and the host that runs a workbook's VBA
//! macros with them ([`run_macro`]).
//!
//! It is its own crate because it needs both the engine and the
//! interpreter, and neither should know the other: `sheets_ui` (Run in the
//! Macros dialog, buttons), the `kantor sheets macros run` command and the
//! tests use it the same way. See `docs/scripting.md`.
//!
//! - A run borrows the workbook: [`run_macro`] takes it and gives it back,
//!   with every change folded into one undo step. Edits go through the
//!   engine's own edit operations, so recalculation, undo and save behave
//!   as for edits made by hand. Recalculation is deferred while the macro
//!   writes and brought up to date before it reads.
//! - Objects hold the run's [`session::Session`] (`Rc`): the workbook, the
//!   active sheet and selections, the clipboard, Find's last search.
//!   Sheets are kept by `sheetId`, so a `Worksheet` object stays right when
//!   sheets are added, moved or renamed.
//! - Dialogs go through [`MacroUi`], which is lent the workbook while one
//!   shows (so the grid can show the cells as the macro left them).
//! - [`udf::VbaFunctions`]: the workbook's VBA functions in cell formulas.
//! - [`MacroEngine`]: a workbook's live project (module variables last
//!   between runs), its events ([`Event`]) and other workbooks (`books`).

pub mod addr;
pub mod app;
pub mod books;
pub mod consts;
pub mod engine;
pub mod events;
pub mod format;
pub mod range;
pub mod run;
pub mod session;
pub mod sheet;
pub mod timers;
pub mod udf;
mod udf_range;
pub mod values;

pub use books::BookOut;
pub use engine::{MacroEngine, Timer};
pub use events::{Event, EventIndex, EventKind};
pub use run::{
    HeadlessUi, InputBoxRequest, InputKind, MacroError, MacroInfo, MacroProject, MacroUi,
    MsgBoxRequest, MsgButtons, MsgIcon, OtherBook, RunOptions, RunOutcome, broken_for, local_now,
    macro_list, run_headless, run_macro, set_utc_offset_minutes,
};
pub use session::{Selection, View};
pub use udf::{VbaFunctions, enable_functions};

/// The macro a button or shape is assigned (`[0]!Macro1`,
/// `'Book1.xlsm'!Module1.Macro1`, `Macro1`), as a name to run
/// (`Module1.Macro1`, `Macro1`).
pub fn assigned_macro(on_action: &str) -> Option<String> {
    let t = on_action.trim();
    let name = match t.rfind('!') {
        Some(i) => &t[i + 1..],
        None => t,
    };
    let name = name.trim().trim_matches('\'').trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::assigned_macro;

    #[test]
    fn assigned_macros() {
        assert_eq!(assigned_macro("[0]!Macro1").as_deref(), Some("Macro1"));
        assert_eq!(
            assigned_macro("'Book 1.xlsm'!Module1.Go").as_deref(),
            Some("Module1.Go")
        );
        assert_eq!(assigned_macro("Macro2").as_deref(), Some("Macro2"));
        assert_eq!(assigned_macro("  "), None);
    }
}
