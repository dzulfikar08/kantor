//! Saving a workbook with macros as a macro-free `.xlsx`: Excel's warning
//! ("The following features cannot be saved in macro-free workbooks: VB
//! project"), with a way to keep them as `.xlsm` instead.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::t;

/// What to do with a workbook's macros on Save As `.xlsx`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MacroChoice {
    /// Save the `.xlsx` without them.
    Drop,
    /// Save as `.xlsm` (same name) instead.
    KeepAsXlsm,
    /// Don't save.
    Cancel,
}

/// `path` with the macro-enabled extension.
pub(crate) fn as_xlsm(path: &std::path::Path) -> PathBuf {
    path.with_extension("xlsm")
}

/// Whether saving to `path` drops macros (a macro-free `.xlsx`).
pub(crate) fn drops_macros(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"))
}

/// Ask what to do; `answer` gets the choice (also Cancel and Escape).
pub(crate) fn confirm_macro_free(
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(MacroChoice, &mut Window, &mut App) + 'static,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let title: SharedString =
        t("The following features cannot be saved in macro-free workbooks: VB project")
            .into();
    let detail: SharedString = t(
        "An .xlsx workbook can't hold macros: saving as .xlsx leaves them out. To keep them, save it as a macro-enabled workbook (.xlsm).")
    .into();
    let answer = Rc::new(answer);
    window.open_dialog(cx, move |dialog, _, _| {
        let reply = |choice: MacroChoice| {
            let answer = answer.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                let answer = answer.clone();
                window.defer(cx, move |window, cx| answer(choice, window, cx));
            }
        };
        dialog
            .w(px(480.))
            .overlay_closable(false)
            .close_button(false)
            .title(title.clone())
            .child(v_flex().child(DialogDescription::new().child(detail.clone())))
            .on_cancel({
                let answer = answer.clone();
                move |_, window, cx| {
                    let answer = answer.clone();
                    window.defer(cx, move |window, cx| answer(MacroChoice::Cancel, window, cx));
                    true
                }
            })
            .on_ok({
                let answer = answer.clone();
                move |_, window, cx| {
                    let answer = answer.clone();
                    window.defer(cx, move |window, cx| {
                        answer(MacroChoice::KeepAsXlsm, window, cx)
                    });
                    true
                }
            })
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("macros-cancel")
                            .label(t("Cancel"))
                            .ghost()
                            .on_click(reply(MacroChoice::Cancel)),
                    )
                    .child(
                        Button::new("macros-drop")
                            .label(t("Save Without Macros"))
                            .on_click(reply(MacroChoice::Drop)),
                    )
                    .child(
                        Button::new("macros-keep")
                            .label(t("Save as .xlsm"))
                            .primary()
                            .on_click(reply(MacroChoice::KeepAsXlsm)),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[std::prelude::v1::test]
    fn only_xlsx_drops_macros() {
        assert!(drops_macros(Path::new("/a/b.xlsx")));
        assert!(drops_macros(Path::new("/a/b.XLSX")));
        assert!(!drops_macros(Path::new("/a/b.xlsm")));
        assert_eq!(as_xlsm(Path::new("/a/b.xlsx")), Path::new("/a/b.xlsm"));
    }
}
