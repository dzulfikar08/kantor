//! Read-only documents: a gate around a document view that refuses edits.
//!
//! The shell wraps a read-only pane in a [`Gate`]. During the capture
//! phase (before the view sees anything) it stops:
//! - every app action that isn't on the allow list below (moving and
//!   selecting, copying, finding, zooming, printing, exporting, presenting,
//!   Save As);
//! - typed text going to the document's own editor (the find bar and other
//!   fields keep working).
//!
//! Edits that arrive any other way (a toolbar button, a dialog, a drag) are
//! undone by the shell right after they happen; see `Workspace` in the
//! `kantor` crate. Nothing in the apps needs to know about read-only.

use std::any::TypeId;
use std::rc::Rc;
use std::sync::OnceLock;

use gpui_kit::*;

/// Action namespaces of the document apps (and their shared parts).
const APP_NAMESPACES: &[&str] = &["docs", "sheets", "slides", "kantor_ai"];

/// Allowed while read-only, by the part after `::`: names starting with
/// one of these…
const ALLOWED_PREFIXES: &[&str] = &[
    "Move", "Select", "Extend", "Zoom", "Present", "Find", "Go", "Scroll", "Next", "Prev", "Print",
    "Export", "Copy", "Page", "Show", "Open", "Close",
];

/// …or exactly one of these.
const ALLOWED_NAMES: &[&str] = &[
    "Collapse",
    "Escape",
    "Cancel",
    "Dismiss",
    "Left",
    "Right",
    "Up",
    "Down",
    "ShiftLeft",
    "ShiftRight",
    "ShiftUp",
    "ShiftDown",
    "LineStart",
    "LineEnd",
    "SaveAs",
    "ToggleNotes",
    "Recalculate",
    "RecalculateSheet",
    "ExplainFormula",
    "OpenAiSettings",
];

/// Whether action `name` (`Action::name()`, e.g. `docs::MoveLeft`) may run
/// in a read-only document. Actions outside the apps' namespaces (the
/// shell's, text fields') are never stopped.
pub fn allows(name: &str) -> bool {
    let Some((ns, short)) = name.rsplit_once("::") else {
        return true;
    };
    if !APP_NAMESPACES.contains(&ns) {
        return true;
    }
    ALLOWED_NAMES.contains(&short) || ALLOWED_PREFIXES.iter().any(|p| short.starts_with(p))
}

/// The registered app actions a read-only document refuses (computed once,
/// after all apps registered their actions).
fn blocked_types(cx: &App) -> Rc<Vec<TypeId>> {
    thread_local! {
        static BLOCKED: OnceLock<Rc<Vec<TypeId>>> = const { OnceLock::new() };
    }
    BLOCKED.with(|b| {
        b.get_or_init(|| {
            Rc::new(
                cx.all_action_names()
                    .iter()
                    .filter(|n| !allows(n))
                    .filter_map(|n| cx.build_action(n, None).ok())
                    .map(|a| a.as_any().type_id())
                    .collect(),
            )
        })
        .clone()
    })
}

/// Called when the gate refuses an edit (shows why).
pub type Refuse = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a gate needs: the document editor's focus (typed text is stopped
/// only there) and what to do when it refuses something.
#[derive(Clone)]
pub struct Guard {
    pub editor: Option<FocusHandle>,
    pub refuse: Refuse,
    /// Stop double-clicks too (Sheets: a double-click edits the cell).
    pub block_double_click: bool,
}

/// Wraps a document view; refuses edits while it has a [`Guard`].
pub struct Gate {
    child: AnyElement,
    guard: Option<Guard>,
}

/// `child`, refusing edits when `guard` is set.
pub fn gate(child: impl IntoElement, guard: Option<Guard>) -> Gate {
    Gate {
        child: child.into_any_element(),
        guard,
    }
}

/// A keystroke that edits by itself in a document editor: typed text, or
/// Enter / Backspace / Delete / F2 (which start or change a cell edit in
/// Sheets), without Ctrl/Alt/Cmd (shortcuts arrive as actions).
fn types_text(k: &Keystroke) -> bool {
    let m = &k.modifiers;
    if m.control || m.alt || m.platform {
        return false;
    }
    k.key_char
        .as_deref()
        .is_some_and(|c| !c.is_empty() && !c.chars().all(char::is_control))
        || matches!(k.key.as_str(), "enter" | "backspace" | "delete" | "f2")
}

impl IntoElement for Gate {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Gate {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(guard) = &self.guard {
            for &ty in blocked_types(cx).iter() {
                let refuse = guard.refuse.clone();
                window.on_action(ty, move |_, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        cx.stop_propagation();
                        refuse(window, cx);
                    }
                });
            }
            if guard.block_double_click {
                let refuse = guard.refuse.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && e.click_count >= 2 {
                        cx.stop_propagation();
                        refuse(window, cx);
                    }
                });
            }
            let (editor, refuse) = (guard.editor.clone(), guard.refuse.clone());
            window.on_key_event(move |e: &KeyDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && types_text(&e.keystroke)
                    && editor.as_ref().is_none_or(|f| f.is_focused(window))
                {
                    cx.stop_propagation();
                    refuse(window, cx);
                }
            });
        }
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn allow_list() {
        for ok in [
            "docs::MoveLeft",
            "docs::SelectAll",
            "docs::Copy",
            "docs::ZoomIn",
            "docs::ExportPdf",
            "docs::SaveAs",
            "slides::Present",
            "slides::Left",
            "sheets::ExtendDown",
            "sheets::Recalculate",
            "find::FindNext",
            "kantor::NewSpreadsheet",
            "input::Backspace",
            "kantor_ai::ExplainFormula",
        ] {
            assert!(allows(ok), "{ok} should be allowed");
        }
        for no in [
            "docs::Backspace",
            "docs::Paste",
            "docs::Cut",
            "docs::ToggleBold",
            "docs::InsertTable",
            "docs::Undo",
            "docs::Save",
            "slides::NewSlide",
            "slides::DeleteSlide",
            "sheets::InsertChart",
            "sheets::FormatAsTable",
            "kantor_ai::RewriteSelection",
            "kantor_ai::AskForFormula",
        ] {
            assert!(!allows(no), "{no} should be refused");
        }
    }

    #[std::prelude::v1::test]
    fn text_keys() {
        let k = |s: &'static str| Keystroke::parse(s).unwrap();
        let mut a = k("a");
        a.key_char = Some("a".into());
        assert!(types_text(&a));
        let mut ctrl_c = k("ctrl-c");
        ctrl_c.key_char = Some("c".into());
        assert!(!types_text(&ctrl_c));
        let left = k("left");
        assert!(!types_text(&left));
        assert!(types_text(&k("enter")));
        assert!(types_text(&k("delete")));
        assert!(!types_text(&k("ctrl-enter")));
    }
}
