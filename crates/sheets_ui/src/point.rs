//! Formula help while editing: point mode, reference colours, F4 and
//! function autocomplete. The text rules live in [`sheets_engine::formula`].
//!
//! **Point mode.** While the edit is a formula and the caret sits where a
//! reference can go (after `=`, `(`, `,` or an operator), clicking a cell
//! inserts its reference at the caret instead of committing; dragging makes
//! it a range and Shift+click extends it. The inserted reference is
//! remembered ([`PointRef`]) together with the text it produced, so clicking
//! again, arrows and Shift+arrows replace it. It stays "live" only while the
//! text is unchanged and the caret is still right after it: typing anything
//! or moving the caret ends it, with no bookkeeping of keys. Arrows start
//! pointing on their own in Enter mode (an edit started by typing). Clicking
//! another sheet's tab keeps the edit going in the formula bar, and cells
//! clicked there come in as `'Sheet2'!A1`.
//!
//! **Colours.** Each distinct reference gets a colour slot ([`ref_color`]);
//! the grid draws the referenced cells with that colour's border and a light
//! fill, and the editors paint the reference text in it. Single-line inputs
//! have no styled-runs hook, so the coloured text is painted over the input's
//! own glyphs at the positions the input reports ([`colored_refs`]).

use std::ops::Range;

use gpui_kit::component::input::InputState;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{Tokens, metrics::*, tokens};
use sheets_engine::{CellRange, formula};

use crate::SheetsView;
use crate::cell_editor::{EDITOR_CONTEXT, EditMode, FORMULA_BAR_CONTEXT, step_cell};

actions!(
    sheets,
    [
        CycleReference,
        PointExtendUp,
        PointExtendDown,
        PointExtendLeft,
        PointExtendRight
    ]
);

/// Rows in the function list.
const COMPLETION_LIMIT: usize = 8;

pub(crate) fn bind_keys(cx: &mut App) {
    let editor = format!("{EDITOR_CONTEXT} > Input");
    let bar = format!("{FORMULA_BAR_CONTEXT} > Input");
    for ctx in [editor.as_str(), bar.as_str()] {
        let ctx = Some(ctx);
        cx.bind_keys([
            KeyBinding::new("f4", CycleReference, ctx),
            KeyBinding::new("shift-up", PointExtendUp, ctx),
            KeyBinding::new("shift-down", PointExtendDown, ctx),
            KeyBinding::new("shift-left", PointExtendLeft, ctx),
            KeyBinding::new("shift-right", PointExtendRight, ctx),
        ]);
    }
}

/// The reference last inserted by pointing.
#[derive(Clone, Debug)]
pub(crate) struct PointRef {
    /// The whole edit text right after the insert; any change ends pointing.
    text: String,
    /// Where the reference sits in `text`.
    span: Range<usize>,
    /// The sheet the reference points at, and its corners.
    pub sheet: u32,
    pub anchor: (u32, u32),
    pub end: (u32, u32),
}

/// The function list shown while a name is typed.
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    items: Vec<&'static str>,
    selected: usize,
    /// The typed part of the name, replaced on accept.
    span: Range<usize>,
}

/// Reference colour `slot`, cycling through six theme colours.
pub(crate) fn ref_color(t: &Tokens, slot: usize) -> Hsla {
    let colors = [t.accent, t.info, t.success, t.warning, t.docs, t.slides];
    colors[slot % colors.len()]
}

/// Paints each `(span, colour)` of `input`'s text in its colour, over the
/// input's own text. Must be placed over the input (absolute) and after it,
/// so the input has laid out this frame.
pub(crate) fn colored_refs(
    input: Entity<InputState>,
    spans: Vec<(Range<usize>, Hsla)>,
) -> impl IntoElement {
    let paint = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            if spans.is_empty() {
                return;
            }
            let pieces: Vec<(SharedString, Bounds<Pixels>, Hsla)> = {
                let state = input.read(cx);
                let text = state.value();
                spans
                    .iter()
                    .filter_map(|(range, color)| {
                        let piece = text.get(range.clone())?;
                        let at = state.range_to_bounds(range)?;
                        Some((SharedString::from(piece.to_owned()), at, *color))
                    })
                    .collect()
            };
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for (piece, at, color) in pieces {
                    let mut run = style.to_run(piece.len());
                    run.color = color;
                    let line = window
                        .text_system()
                        .shape_line(piece, font_size, &[run], None);
                    let _ =
                        line.paint(at.origin, at.size.height, TextAlign::Left, None, window, cx);
                }
            });
        },
    )
    .size_full();
    // The inputs are `.small()`, which is `text_sm`; a div passes the text
    // style down to the canvas's paint.
    div().absolute().inset_0().text_sm().child(paint)
}

impl SheetsView {
    /// The editor with focus (the formula bar, else the cell editor).
    pub(crate) fn active_input(&self, window: &Window, cx: &App) -> Entity<InputState> {
        if self.formula.read(cx).focus_handle(cx).is_focused(window) {
            self.formula.clone()
        } else {
            self.cell_input.clone()
        }
    }

    fn formula_bar_focused(&self, window: &Window, cx: &App) -> bool {
        self.formula.read(cx).focus_handle(cx).is_focused(window)
    }

    pub(crate) fn grid_sheet(&self, cx: &App) -> u32 {
        self.table.read(cx).delegate().sheet
    }

    /// The pointed reference, if the text and caret still match it.
    pub(crate) fn live_point(&self, window: &Window, cx: &App) -> Option<PointRef> {
        let point = self.point.as_ref()?;
        let input = self.active_input(window, cx);
        let state = input.read(cx);
        let caret = point.span.end..point.span.end;
        (state.value().as_ref() == point.text && state.selected_range() == caret)
            .then(|| point.clone())
    }

    /// Where a pointed reference would go: over the live one, or at the caret
    /// if a reference can start there.
    fn point_slot(&self, window: &Window, cx: &App) -> Option<Range<usize>> {
        self.editing?;
        if let Some(point) = self.live_point(window, cx) {
            return Some(point.span);
        }
        let input = self.active_input(window, cx);
        let state = input.read(cx);
        let sel = state.selected_range();
        (sel.is_empty() && formula::accepts_reference(&state.value(), sel.start)).then_some(sel)
    }

    /// Whether a click on a cell (or a sheet tab) should point.
    pub(crate) fn can_point(&self, window: &Window, cx: &App) -> bool {
        self.point_slot(window, cx).is_some()
    }

    /// Insert (or replace) the pointed reference to `anchor:end` on the sheet
    /// the grid shows. `false` (and nothing done) when the caret is not where
    /// a reference can go.
    pub(crate) fn point_to(
        &mut self,
        anchor: (u32, u32),
        end: (u32, u32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(slot) = self.point_slot(window, cx) else {
            return false;
        };
        let sheet = self.grid_sheet(cx);
        let sheet_name = (sheet != self.edit_sheet)
            .then(|| {
                let grid = self.table.read(cx).delegate();
                grid.book.sheet_names().get(sheet as usize).cloned()
            })
            .flatten();
        let text =
            formula::format_reference(sheet_name.as_deref(), CellRange::from_corners(anchor, end));
        let input = self.active_input(window, cx);
        self.replace_in(&input, slot.clone(), &text, window, cx);
        self.point = Some(PointRef {
            text: input.read(cx).value().to_string(),
            span: slot.start..slot.start + text.len(),
            sheet,
            anchor,
            end,
        });
        self.completion = None;
        self.refresh_refs(cx);
        cx.notify();
        true
    }

    /// Replace `range` of `input`'s text (caret after it) and mirror it into
    /// the other editor.
    fn replace_in(
        &mut self,
        input: &Entity<InputState>,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        input.update(cx, |state, cx| {
            state.set_selected_range(range, cx);
            state.replace(text, window, cx);
        });
        let other = if *input == self.formula {
            self.cell_input.clone()
        } else {
            self.formula.clone()
        };
        Self::mirror(input, &other, window, cx);
    }

    /// The edit text changed (typed, pasted or set by us): end a stale point,
    /// update the function list and the reference colours.
    pub(crate) fn on_edit_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            return;
        }
        let input = self.active_input(window, cx);
        let (text, caret) = {
            let state = input.read(cx);
            (state.value().to_string(), state.cursor())
        };
        if self.point.as_ref().is_some_and(|p| p.text != text) {
            self.point = None;
        }
        self.completion = formula::function_prefix(&text, caret).and_then(|span| {
            let items = formula::matching_functions(&text[span.clone()], COMPLETION_LIMIT);
            (!items.is_empty()).then_some(Completion {
                items,
                selected: 0,
                span,
            })
        });
        self.refresh_refs(cx);
        cx.notify();
    }

    /// Recompute the references of the edit text for the grid and editors.
    pub(crate) fn refresh_refs(&mut self, cx: &mut Context<Self>) {
        let text = if self.editing.is_some() {
            self.cell_input.read(cx).value().to_string()
        } else {
            String::new()
        };
        let refs = formula::references(&text);
        let slots = formula::reference_colors(&refs);
        let edit_sheet = self.edit_sheet;
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let names = grid.book.sheet_names();
            grid.ref_spans = refs
                .iter()
                .zip(&slots)
                .map(|(r, slot)| (r.span.clone(), *slot))
                .collect();
            grid.ref_marks = refs
                .iter()
                .zip(&slots)
                .filter(|(r, _)| {
                    let target = match &r.sheet {
                        None => Some(edit_sheet),
                        Some(name) => names
                            .iter()
                            .position(|n| n.to_lowercase() == name.to_lowercase())
                            .map(|ix| ix as u32),
                    };
                    target == Some(grid.sheet)
                })
                .map(|(r, slot)| (r.range, *slot))
                .collect();
            cx.notify();
        });
    }

    /// An arrow while editing: move the function list's selection, or point.
    /// `false` when the arrow is not for us.
    pub(crate) fn assist_arrow(
        &mut self,
        step: (i64, i64),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(completion) = self.completion.as_mut()
            && step.1 == 0
        {
            let len = completion.items.len() as i64;
            completion.selected = (completion.selected as i64 + step.0).rem_euclid(len) as usize;
            cx.notify();
            return true;
        }
        let sheet = self.grid_sheet(cx);
        let from = match self.live_point(window, cx) {
            Some(point) if point.sheet == sheet => point.end,
            Some(_) => self.point_origin(sheet, cx),
            None => {
                // Arrows only start pointing in the cell editor in Enter mode.
                if self.formula_bar_focused(window, cx)
                    || self.edit_mode != EditMode::Enter
                    || !self.can_point(window, cx)
                {
                    return false;
                }
                self.point_origin(sheet, cx)
            }
        };
        let cell = self.step_in_grid(from, step, cx);
        self.point_to(cell, cell, window, cx);
        self.reveal_point(cell, window, cx);
        true
    }

    /// Where arrow pointing starts: the edited cell, or `A1` on another sheet.
    fn point_origin(&self, sheet: u32, _: &App) -> (u32, u32) {
        match self.editing {
            Some(cell) if sheet == self.edit_sheet => cell,
            _ => (0, 0),
        }
    }

    fn step_in_grid(&self, from: (u32, u32), step: (i64, i64), cx: &App) -> (u32, u32) {
        let grid = self.table.read(cx).delegate();
        step_cell(from, step, grid.rows, grid.cols)
    }

    /// Scroll a pointed cell into view, but only while typing in the formula
    /// bar: scrolling the edited cell away would unmount the cell editor,
    /// which loses focus and commits.
    fn reveal_point(&mut self, cell: (u32, u32), window: &Window, cx: &mut Context<Self>) {
        if !self.formula_bar_focused(window, cx) {
            return;
        }
        self.table.update(cx, |table, cx| {
            table
                .vertical_scroll_handle
                .scroll_to_item(cell.0 as usize, ScrollStrategy::Nearest);
            table.scroll_to_col(cell.1 as usize + 1, cx);
        });
    }

    /// Shift+arrow: grow the pointed reference into a range, else select text.
    fn point_extend(&mut self, step: (i64, i64), window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.grid_sheet(cx);
        let Some(point) = self.live_point(window, cx).filter(|p| p.sheet == sheet) else {
            cx.propagate();
            return;
        };
        let end = self.step_in_grid(point.end, step, cx);
        self.point_to(point.anchor, end, window, cx);
        self.reveal_point(end, window, cx);
    }

    fn point_extend_up(&mut self, _: &PointExtendUp, window: &mut Window, cx: &mut Context<Self>) {
        self.point_extend((-1, 0), window, cx);
    }

    fn point_extend_down(
        &mut self,
        _: &PointExtendDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.point_extend((1, 0), window, cx);
    }

    fn point_extend_left(
        &mut self,
        _: &PointExtendLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.point_extend((0, -1), window, cx);
    }

    fn point_extend_right(
        &mut self,
        _: &PointExtendRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.point_extend((0, 1), window, cx);
    }

    /// F4: cycle the reference under the caret through `$` forms.
    fn cycle_reference(&mut self, _: &CycleReference, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            cx.propagate();
            return;
        }
        let input = self.active_input(window, cx);
        let (text, caret) = {
            let state = input.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let Some(found) = formula::reference_at(&text, caret) else {
            return;
        };
        let Some(cycled) = formula::cycle_absolute(&text[found.span.clone()]) else {
            return;
        };
        let point = self.live_point(window, cx).filter(|p| p.span == found.span);
        self.replace_in(&input, found.span.clone(), &cycled, window, cx);
        // Keep pointing (a click still replaces the reference) if it was live.
        self.point = point.map(|p| PointRef {
            text: input.read(cx).value().to_string(),
            span: found.span.start..found.span.start + cycled.len(),
            ..p
        });
        self.refresh_refs(cx);
        cx.notify();
    }

    /// A sheet tab was clicked while pointing: keep the edit going in the
    /// formula bar and show `sheet`, so its cells can be pointed at.
    pub(crate) fn point_to_sheet(
        &mut self,
        sheet: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.formula_bar_focused(window, cx) {
            let selection = self.cell_input.read(cx).selected_range();
            self.formula.update(cx, |input, cx| {
                input.set_selected_range(selection, cx);
                input.focus(window, cx);
            });
        }
        self.show_sheet(sheet, cx);
        self.refresh_refs(cx);
        cx.notify();
    }

    /// Enter / Tab with the function list open: insert the chosen name.
    pub(crate) fn accept_completion(
        &mut self,
        ix: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(completion) = self.completion.take() else {
            return false;
        };
        let ix = ix.unwrap_or(completion.selected);
        let Some(name) = completion.items.get(ix) else {
            return false;
        };
        let input = self.active_input(window, cx);
        self.replace_in(&input, completion.span, &format!("{name}("), window, cx);
        self.point = None;
        self.refresh_refs(cx);
        cx.notify();
        true
    }

    /// Escape with the function list open closes just the list.
    pub(crate) fn dismiss_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if self.completion.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// The function list, under the name being typed.
    pub(crate) fn render_completion(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let completion = self.completion.as_ref()?;
        let input = self.active_input(window, cx);
        let at = input.read(cx).range_to_bounds(&completion.span)?;
        let t = tokens(cx);
        let rows = completion.items.iter().enumerate().map(|(ix, name)| {
            div()
                .id(("sheets-fn-completion", ix))
                .h(CONTROL_SM)
                .px(SPACE_4)
                .flex()
                .items_center()
                .rounded(RADIUS_SM)
                .text_color(t.fg)
                .when(ix == completion.selected, |d| d.bg(t.selected))
                .hover(|s| s.bg(t.hover))
                .cursor_pointer()
                .child(SharedString::from(*name))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        // Keep focus (and the edit) in the input.
                        window.prevent_default();
                        cx.stop_propagation();
                        this.accept_completion(Some(ix), window, cx);
                    }),
                )
        });
        Some(deferred(
            anchored()
                .position(at.bottom_left() + point(px(0.), SPACE_2))
                .child(
                    v_flex()
                        .id("sheets-fn-completion")
                        .occlude()
                        .min_w(px(160.))
                        .p(SPACE_2)
                        .bg(t.elevated)
                        .border_1()
                        .border_color(t.border)
                        .rounded(RADIUS_MD)
                        .shadow(t.shadow(Elevation::Popover))
                        .text_size(FONT_SMALL)
                        .children(rows),
                ),
        ))
    }

    /// Register the formula-help actions on the view's root element.
    pub(crate) fn point_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(Self::cycle_reference))
            .on_action(cx.listener(Self::point_extend_up))
            .on_action(cx.listener(Self::point_extend_down))
            .on_action(cx.listener(Self::point_extend_left))
            .on_action(cx.listener(Self::point_extend_right))
    }
}
