//! In-cell editor: while a cell is being edited, an [`Input`] sits exactly
//! over it (like Excel / Google Sheets), focused, and grows to the right for
//! long text.
//!
//! **Two inputs, kept in sync.** The cell editor and the formula bar each own
//! an [`InputState`]; a change in either is copied into the other (`set_value`
//! emits no events, so this cannot loop). One shared state rendered twice does
//! not work: an `InputState` keeps the layout of the element that painted it
//! last (caret, mouse hit-testing, IME candidate position), so two elements
//! would fight over it. Focus can move between the two freely; the edit only
//! commits when focus leaves both (see [`SheetsView::on_editor_blur`]).
//!
//! **Placement.** The overlay is drawn from the edited cell's own `render_td`,
//! so it follows column widths and scrolling with no bookkeeping. A small
//! element ([`Overlay`]) takes the cell's bounds in prepaint, sizes the editor
//! to `max(cell width, text width)`, and draws it *deferred* so it paints over
//! the neighbouring cells, clipped to the table body (taken from the table's
//! scroll handles, so it never covers the header or the row gutter). The
//! editor exists only while its cell is rendered: `start_edit` scrolls the
//! cell into view first, and the wheel is blocked over the editor itself.
//!
//! **Keys** (bound for both inputs unless noted): Enter / Shift+Enter commit
//! and move down / up, Tab / Shift+Tab commit and move right / left, Escape
//! cancels. In the cell editor only: arrows commit and move when the edit was
//! started by typing ("Enter mode"), and move the caret when it was started
//! with F2, Enter or a double-click ("Edit mode"); F2 or a click inside the
//! editor switches to Edit mode. These bindings use `KantorCellEditor > Input`
//! (and `KantorFormulaBar > Input`), which match as deep as the input's own
//! bindings and are registered later, so they win; an arrow in Edit mode
//! propagates and falls through to the input's caret movement.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, VirtualListScrollHandle};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};

use std::ops::Range;

use crate::SheetsView;
use crate::point::colored_refs;

actions!(
    sheets,
    [
        CommitDown,
        CommitUp,
        CommitRight,
        CommitLeft,
        CancelEdit,
        EditUp,
        EditDown,
        EditLeft,
        EditRight,
        ToggleEditMode
    ]
);

pub(crate) const EDITOR_CONTEXT: &str = "KantorCellEditor";
pub(crate) const FORMULA_BAR_CONTEXT: &str = "KantorFormulaBar";
/// Room for the caret past the last character, so text never scrolls while
/// the editor can still grow.
const CARET_ROOM: Pixels = SPACE_7;

/// How arrow keys behave in the cell editor (Excel's Enter and Edit modes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditMode {
    /// Started by typing: arrows commit and move to the next cell.
    Enter,
    /// Started with F2 / Enter / double-click: arrows move the caret.
    Edit,
}

pub(crate) fn bind_keys(cx: &mut App) {
    let editor = format!("{EDITOR_CONTEXT} > Input");
    let bar = format!("{FORMULA_BAR_CONTEXT} > Input");
    for ctx in [editor.as_str(), bar.as_str()] {
        let ctx = Some(ctx);
        cx.bind_keys([
            KeyBinding::new("enter", CommitDown, ctx),
            KeyBinding::new("shift-enter", CommitUp, ctx),
            KeyBinding::new("tab", CommitRight, ctx),
            KeyBinding::new("shift-tab", CommitLeft, ctx),
            KeyBinding::new("escape", CancelEdit, ctx),
        ]);
    }
    // In the formula bar arrows only point (or move the function list's
    // selection); otherwise they fall through to the caret.
    for ctx in [editor.as_str(), bar.as_str()] {
        let ctx = Some(ctx);
        cx.bind_keys([
            KeyBinding::new("up", EditUp, ctx),
            KeyBinding::new("down", EditDown, ctx),
            KeyBinding::new("left", EditLeft, ctx),
            KeyBinding::new("right", EditRight, ctx),
        ]);
    }
    cx.bind_keys([KeyBinding::new("f2", ToggleEditMode, Some(editor.as_str()))]);
}

/// What the grid needs to draw the editor over the edited cell.
#[derive(Clone)]
pub(crate) struct CellEditor {
    /// The edited cell, sheet coordinates `(row, col)`, and its sheet.
    pub cell: (u32, u32),
    pub sheet: u32,
    input: Entity<InputState>,
    view: WeakEntity<SheetsView>,
    /// The table's scroll handles; their bounds are the visible body.
    rows: UniformListScrollHandle,
    cols: VirtualListScrollHandle,
}

impl CellEditor {
    /// The overlay element, to be added as a child of the edited cell.
    /// `refs` are the formula's references with their colours.
    pub fn render(&self, refs: Vec<(Range<usize>, Hsla)>, cx: &App) -> impl IntoElement + use<> {
        let t = tokens(cx);
        let view = self.view.clone();
        let editor = div()
            .key_context(EDITOR_CONTEXT)
            .size_full()
            .flex()
            .items_center()
            .bg(t.bg)
            // Same width as the active cell's outline, which it covers.
            .border(CELL_OUTLINE)
            .border_color(t.accent)
            .shadow_md()
            // Keep clicks away from the cell and the table underneath.
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = view.update(cx, |view, _| view.edit_mode = EditMode::Edit);
            })
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .items_center()
                    .child(
                        Input::new(&self.input)
                            .appearance(false)
                            .small()
                            // Text starts where the cell's own text does.
                            .px(SPACE_3 - CELL_OUTLINE)
                            .w_full(),
                    )
                    .child(colored_refs(self.input.clone(), refs)),
            );
        Overlay {
            input: self.input.clone(),
            rows: self.rows.clone(),
            cols: self.cols.clone(),
            child: Some(editor.into_any_element()),
        }
    }
}

/// Fills the cell for layout, then draws its child deferred at the cell's
/// origin, widened to fit the text.
struct Overlay {
    input: Entity<InputState>,
    rows: UniformListScrollHandle,
    cols: VirtualListScrollHandle,
    child: Option<AnyElement>,
}

impl Overlay {
    /// The visible table body (scrollable columns × visible rows), if laid out.
    fn body(&self) -> Option<Bounds<Pixels>> {
        let rows = self.rows.0.borrow().base_handle.bounds();
        let cols = self.cols.bounds();
        let body = Bounds::from_corners(
            point(cols.left(), rows.top()),
            point(cols.right(), rows.bottom()),
        );
        (body.size.width > px(0.) && body.size.height > px(0.)).then_some(body)
    }
}

impl Element for Overlay {
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
        let mut style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        cell: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        // The cell's text style (the table sets the same size as the input).
        let text: SharedString = self.input.read(cx).value().replace('\n', " ").into();
        let text_width = if text.is_empty() {
            px(0.)
        } else {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let run = style.to_run(text.len());
            window
                .text_system()
                .shape_line(text, font_size, &[run], None)
                .width
        };
        let body = self.body();
        let mut width = cell.size.width.max(text_width + SPACE_3 * 2. + CARET_ROOM);
        if let Some(body) = body {
            // Grow up to the body's right edge, never below the cell itself.
            width = width.min(body.right() - cell.left()).max(cell.size.width);
        }
        let mut editor = div()
            .w(width)
            .h(cell.size.height)
            .child(child)
            .into_any_element();
        editor.layout_as_root(
            size(
                AvailableSpace::Definite(width),
                AvailableSpace::Definite(cell.size.height),
            ),
            window,
            cx,
        );
        let mask = body.map(|bounds| ContentMask { bounds });
        window.defer_draw(editor, cell.origin, 1, mask);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

impl IntoElement for Overlay {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl SheetsView {
    /// Show (or hide) the editor overlay in the grid for `cell`.
    /// The edit belongs to the sheet shown now.
    pub(crate) fn set_editing(&mut self, cell: Option<(u32, u32)>, cx: &mut Context<Self>) {
        self.editing = cell;
        self.point = None;
        self.completion = None;
        let view = cx.entity().downgrade();
        let input = self.cell_input.clone();
        let sheet = self.grid_sheet(cx);
        self.edit_sheet = sheet;
        self.table.update(cx, |table, cx| {
            let editor = cell.map(|cell| CellEditor {
                cell,
                sheet,
                input,
                view,
                rows: table.vertical_scroll_handle.clone(),
                cols: table.horizontal_scroll_handle.clone(),
            });
            table.delegate_mut().editor = editor;
            cx.notify();
        });
        self.refresh_refs(cx);
        cx.notify();
    }

    /// Whether either editor (cell or formula bar) has keyboard focus.
    fn editor_focused(&self, window: &Window, cx: &App) -> bool {
        self.cell_input.read(cx).focus_handle(cx).is_focused(window)
            || self.formula.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The text being edited, from whichever input has focus (they match).
    pub(crate) fn edit_value(&self, window: &Window, cx: &App) -> String {
        let input = if self.formula.read(cx).focus_handle(cx).is_focused(window) {
            &self.formula
        } else {
            &self.cell_input
        };
        input.read(cx).value().to_string()
    }

    /// Copy `from`'s text into `to` if it differs.
    pub(crate) fn mirror(
        from: &Entity<InputState>,
        to: &Entity<InputState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text = from.read(cx).value();
        if to.read(cx).value() != text {
            to.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    pub(crate) fn on_cell_input_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                Self::mirror(&self.cell_input, &self.formula, window, cx);
                self.on_edit_changed(window, cx);
            }
            // Ctrl+Enter (Enter itself is our `CommitDown`).
            InputEvent::PressEnter { .. } => self.commit_edit(Some((1, 0)), window, cx),
            InputEvent::Blur => self.on_editor_blur(window, cx),
            _ => {}
        }
    }

    /// Typing in the formula bar shows up in the cell editor.
    pub(crate) fn mirror_formula(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            Self::mirror(&self.formula, &self.cell_input, window, cx);
            self.on_edit_changed(window, cx);
        }
    }

    /// One of the editors lost focus. Commit once focus has settled outside
    /// both (another cell was clicked), not when it moved between them.
    pub(crate) fn on_editor_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |this, window, cx| {
            if this.editing.is_some() && !this.editor_focused(window, cx) {
                this.commit_edit(None, window, cx);
            }
        });
    }

    fn commit_down(&mut self, _: &CommitDown, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accept_completion(None, window, cx) {
            self.commit_edit(Some((1, 0)), window, cx);
        }
    }

    fn commit_up(&mut self, _: &CommitUp, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(Some((-1, 0)), window, cx);
    }

    fn commit_right(&mut self, _: &CommitRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accept_completion(None, window, cx) {
            self.commit_edit(Some((0, 1)), window, cx);
        }
    }

    fn commit_left(&mut self, _: &CommitLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(Some((0, -1)), window, cx);
    }

    /// An arrow while editing: move the function list's selection or point
    /// (see [`crate::point`]); otherwise in the cell editor commit and move in
    /// Enter mode, and everywhere else let the input move the caret.
    fn edit_arrow(&mut self, step: (i64, i64), window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            cx.propagate();
            return;
        }
        if self.assist_arrow(step, window, cx) {
            return;
        }
        let in_bar = self.formula.read(cx).focus_handle(cx).is_focused(window);
        if in_bar || self.edit_mode == EditMode::Edit {
            cx.propagate();
            return;
        }
        self.commit_edit(Some(step), window, cx);
    }

    fn edit_up(&mut self, _: &EditUp, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_arrow((-1, 0), window, cx);
    }

    fn edit_down(&mut self, _: &EditDown, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_arrow((1, 0), window, cx);
    }

    fn edit_left(&mut self, _: &EditLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_arrow((0, -1), window, cx);
    }

    fn edit_right(&mut self, _: &EditRight, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_arrow((0, 1), window, cx);
    }

    fn toggle_edit_mode(&mut self, _: &ToggleEditMode, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_mode = match self.edit_mode {
            EditMode::Enter => EditMode::Edit,
            EditMode::Edit => EditMode::Enter,
        };
        cx.notify();
    }

    /// Register the editing actions on the view's root element.
    pub(crate) fn editor_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(Self::commit_down))
            .on_action(cx.listener(Self::commit_up))
            .on_action(cx.listener(Self::commit_right))
            .on_action(cx.listener(Self::commit_left))
            .on_action(cx.listener(Self::cancel_edit))
            .on_action(cx.listener(Self::edit_up))
            .on_action(cx.listener(Self::edit_down))
            .on_action(cx.listener(Self::edit_left))
            .on_action(cx.listener(Self::edit_right))
            .on_action(cx.listener(Self::toggle_edit_mode))
    }
}

/// The cell one `step` away from `cell`, kept inside a `rows` × `cols` grid.
pub(crate) fn step_cell(cell: (u32, u32), step: (i64, i64), rows: u32, cols: u32) -> (u32, u32) {
    let row = (cell.0 as i64 + step.0).clamp(0, rows.saturating_sub(1) as i64);
    let col = (cell.1 as i64 + step.1).clamp(0, cols.saturating_sub(1) as i64);
    (row as u32, col as u32)
}

#[cfg(test)]
mod tests {
    use super::step_cell;

    #[test]
    fn steps_stay_inside_the_grid() {
        assert_eq!(step_cell((3, 4), (1, 0), 10, 10), (4, 4));
        assert_eq!(step_cell((3, 4), (0, -1), 10, 10), (3, 3));
        assert_eq!(step_cell((0, 0), (-1, 0), 10, 10), (0, 0));
        assert_eq!(step_cell((0, 0), (0, -1), 10, 10), (0, 0));
        assert_eq!(step_cell((9, 9), (1, 1), 10, 10), (9, 9));
    }
}
