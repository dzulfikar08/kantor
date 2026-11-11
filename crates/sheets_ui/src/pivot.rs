//! Pivot tables in the Sheets view.
//!
//! * The status bar says when the active cell is in one (the chip opens the
//!   field list), and Refresh PivotTable (Alt+F5) / Refresh All
//!   (Ctrl+Alt+F5) recompute them from their source.
//! * **Insert PivotTable…** (toolbar, palette) opens a card over the grid:
//!   the source as reference text (the table the active cell is in, else
//!   the selection, else the current region; checked as it is typed) and
//!   the destination (a new sheet, or a cell typed as `Sheet1!H2`).
//! * **PivotTable Fields** is a side panel beside the grid: the source's
//!   fields with check boxes (a text field goes to Rows, a number to Values
//!   as Sum, as in Excel) and the four areas Filters / Columns / Rows /
//!   Values. Fields are dragged from the list or between areas (GPUI drag
//!   and drop); each chip also has a menu (move up / down, move to another
//!   area, remove, value field settings, filter items) for exact moves.
//!   Report layout, subtotals and grand totals are at the bottom. Every
//!   change lays the pivot out again (`Workbook::set_pivot_layout`), one
//!   undo step each. It works for pivots from files too.
//! * **Delete PivotTable** removes the one at the active cell.
//!
//! The panel and the card are siblings of the grid, never inside it (the
//! grid's key handler would take the inputs' typing).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{
    AreaEntry, AxisEntry, PivotDestination, PivotFunc, PivotLayout, ReportLayout, SubtotalPlace,
    cell_name,
};

use crate::{KEY_CONTEXT, SheetsView};

actions!(
    sheets,
    [
        RefreshPivotTable,
        RefreshAllPivotTables,
        InsertPivotTable,
        PivotTableFields,
        DeletePivotTable
    ]
);

/// Width of the field list panel.
const PANEL_WIDTH: Pixels = px(300.);
const AREAS: [&str; 4] = ["Filters", "Columns", "Rows", "Values"];

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-f5", RefreshPivotTable, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-alt-f5", RefreshAllPivotTables, Some(KEY_CONTEXT)),
    ]);
}

/// The Insert PivotTable card.
pub(crate) struct InsertDialog {
    source: Entity<InputState>,
    dest: Entity<InputState>,
    new_sheet: bool,
    /// The sheet the card was opened on (sources without a sheet name).
    sheet: u32,
    /// What the source holds, or why it can't be used.
    check: Result<String, String>,
    /// Why the last OK failed.
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// The PivotTable Fields panel.
pub(crate) struct FieldPanel {
    sheet: u32,
    /// The pivot shown (names are unique in a workbook).
    name: String,
    /// Its layout, read at `revision`.
    layout: Option<PivotLayout>,
    revision: Option<(u64, u64)>,
    settings: Option<ValueSettings>,
    picker: Option<FilterPicker>,
}

/// Value Field Settings for the k-th value field.
struct ValueSettings {
    k: usize,
    name: Entity<InputState>,
    func: PivotFunc,
    num_fmt: Option<String>,
    _sub: Subscription,
}

/// The item picker of a report filter (or any field's item filter).
struct FilterPicker {
    field: usize,
    checked: Vec<bool>,
}

/// What is dragged in the field list.
#[derive(Clone)]
struct PivotDrag {
    entry: AreaEntry,
    label: SharedString,
}

/// The chip that follows the pointer while dragging.
struct DragChip {
    label: SharedString,
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        div()
            .px(SPACE_4)
            .py(SPACE_2)
            .rounded(RADIUS_SM)
            .bg(t.elevated)
            .border_1()
            .border_color(t.accent)
            .shadow_md()
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .child(self.label.clone())
    }
}

/// Excel's number formats offered for value fields, besides the source's.
const NUM_FMTS: [(&str, &str); 5] = [
    ("Number", "#,##0.00"),
    ("Whole number", "#,##0"),
    ("Percent", "0%"),
    ("Percent (0.00)", "0.00%"),
    ("Scientific", "0.00E+00"),
];

fn layout_label(l: ReportLayout) -> &'static str {
    match l {
        ReportLayout::Compact => "Compact form",
        ReportLayout::Outline => "Outline form",
        ReportLayout::Tabular => "Tabular form",
    }
}

fn subtotal_label(s: SubtotalPlace) -> &'static str {
    match s {
        SubtotalPlace::Off => "Don't show subtotals",
        SubtotalPlace::Top => "Subtotals at the top",
        SubtotalPlace::Bottom => "Subtotals at the bottom",
    }
}

type ChipAction = Box<dyn Fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>)>;

impl SheetsView {
    /// `PivotTable · PivotTable1` when the active cell is in a pivot table
    /// (with why it can't be refreshed, if it can't).
    pub fn pivot_status(&self, cx: &App) -> Option<String> {
        if self.busy.is_some() {
            return None;
        }
        let grid = self.table.read(cx).delegate();
        let (row, col) = self.selected;
        let p = grid.book.pivot_at(grid.sheet, row, col)?;
        Some(match p.refresh_problem {
            None => format!("PivotTable · {}", p.name),
            Some(_) => format!("PivotTable · {} (can't refresh here)", p.name),
        })
    }

    /// Refresh the pivot table at the active cell (and those sharing its
    /// data), or every pivot table of the workbook.
    pub fn refresh_pivots_doc(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = if all {
                grid.book.refresh_all_pivots()
            } else {
                grid.book.refresh_pivot(sheet, row, col)
            };
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(0) => "This workbook has no PivotTables to refresh".into(),
            Ok(1) => "PivotTable refreshed".into(),
            Ok(n) => format!("{n} PivotTables refreshed").into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.after_pivot_edit(window, cx);
    }

    fn after_pivot_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.pivot_panel.as_mut() {
            p.revision = None;
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    // -------------------------------------------------------------- insert

    /// Open the Insert PivotTable card.
    pub fn insert_pivot_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (suggestion, sheet, here) = {
            let grid = self.table.read(cx).delegate();
            let sel = grid.range();
            let sheet = grid.sheet;
            let name = grid
                .book
                .sheet_names()
                .get(sheet as usize)
                .cloned()
                .unwrap_or_default();
            let quoted = if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                name
            } else {
                format!("'{}'", name.replace('\'', "''"))
            };
            let (r, c) = self.selected;
            (
                grid.book.pivot_source_suggestion(sheet, sel, (r, c)),
                sheet,
                format!("{quoted}!{}", cell_name(r, c)),
            )
        };
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Table name or range, e.g. Sheet1!A1:D20")
        });
        let dest = cx.new(|cx| InputState::new(window, cx).placeholder("Cell, e.g. Sheet1!H2"));
        source.update(cx, |i, cx| {
            i.set_value(suggestion, window, cx);
            i.focus(window, cx);
        });
        dest.update(cx, |i, cx| i.set_value(here, window, cx));
        let subs = vec![
            cx.subscribe_in(&source, window, Self::on_pivot_dialog_input),
            cx.subscribe_in(&dest, window, Self::on_pivot_dialog_input),
        ];
        self.pivot_dialog = Some(InsertDialog {
            source,
            dest,
            new_sheet: true,
            sheet,
            check: Err(String::new()),
            error: None,
            _subs: subs,
        });
        self.check_pivot_dialog(cx);
        cx.notify();
    }

    fn check_pivot_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.pivot_dialog.as_ref() else {
            return;
        };
        let text = d.source.read(cx).value().to_string();
        let sheet = d.sheet;
        let grid = self.table.read(cx).delegate();
        let check = match grid.book.check_pivot_source(&text, sheet) {
            Ok(c) => {
                let rows = c.range.rows_count().saturating_sub(1);
                let names = grid.book.sheet_names();
                let place = match &c.table {
                    Some(t) => format!("table {t}"),
                    None => format!(
                        "{}!{}",
                        names.get(c.sheet as usize).cloned().unwrap_or_default(),
                        c.range.name()
                    ),
                };
                Ok(format!("{} fields · {rows} rows · {place}", c.fields.len()))
            }
            Err(e) => Err(format!("{e:#}")),
        };
        if let Some(d) = self.pivot_dialog.as_mut() {
            d.check = check;
            d.error = None;
        }
    }

    fn on_pivot_dialog_input(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.check_pivot_dialog(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => self.pivot_dialog_ok(window, cx),
            _ => {}
        }
    }

    fn close_pivot_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pivot_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    fn pivot_dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.pivot_dialog.as_ref() else {
            return;
        };
        let source = d.source.read(cx).value().to_string();
        let dest_text = d.dest.read(cx).value().to_string();
        let (new_sheet, src_sheet) = (d.new_sheet, d.sheet);
        let dest = if new_sheet {
            Ok(PivotDestination::NewSheet)
        } else {
            let grid = self.table.read(cx).delegate();
            parse_cell(&dest_text, &grid.book.sheet_names(), src_sheet)
                .map(|(sheet, row, col)| PivotDestination::Cell { sheet, row, col })
        };
        let result = dest.and_then(|dest| {
            self.table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                let res = grid.book.insert_pivot(&source, src_sheet, dest);
                grid.resize();
                table.refresh(cx);
                res.map_err(|e| format!("{e:#}"))
            })
        });
        match result {
            Ok((sheet, row, col)) => {
                self.pivot_dialog = None;
                let current = self.table.read(cx).delegate().sheet;
                if sheet != current {
                    self.show_sheet(sheet, cx);
                }
                self.select_cell(row, col, cx);
                self.selected = (row, col);
                let name = {
                    let grid = self.table.read(cx).delegate();
                    grid.book.pivot_at(sheet, row, col).map(|p| p.name)
                };
                if let Some(name) = name {
                    self.status = format!("{name}: choose its fields in the panel").into();
                    self.open_panel(sheet, name);
                }
                self.focus_grid(window, cx);
                self.after_pivot_edit(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.pivot_dialog.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn render_pivot_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.pivot_dialog.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let choice = |id: &'static str, label: &'static str, on: bool, new_sheet: bool| {
            let view = view.clone();
            Button::new(id)
                .small()
                .label(label)
                .when_else(on, |b| b.primary(), |b| b.outline())
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if let Some(d) = this.pivot_dialog.as_mut() {
                            d.new_sheet = new_sheet;
                        }
                        cx.notify();
                    });
                })
        };
        let (check_text, check_ok) = match &d.check {
            Ok(s) => (s.clone(), true),
            Err(e) => (e.clone(), false),
        };
        let ok_view = view.clone();
        let cancel_view = view.clone();
        Some(
            v_flex()
                .id("pivot-insert")
                .key_context("KantorPivotDialog")
                .occlude()
                .absolute()
                .top(CONTROL_LG * 2. + SPACE_5)
                .right(SPACE_6)
                .w(px(380.))
                .gap(SPACE_4)
                .p(SPACE_6)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border_strong)
                .shadow_md()
                .text_size(FONT_BODY)
                .text_color(t.fg)
                .on_action(
                    cx.listener(|this, _: &input::Escape, w, cx| this.close_pivot_dialog(w, cx)),
                )
                .child(
                    div()
                        .text_size(FONT_EMPHASIS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Insert PivotTable"),
                )
                .child(kantor_ui::components::section_label("Data to analyse", cx))
                .child(Input::new(&d.source).small())
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(if check_ok { t.fg_muted } else { t.danger })
                        .child(SharedString::from(if check_text.is_empty() {
                            " ".to_owned()
                        } else {
                            check_text
                        })),
                )
                .child(kantor_ui::components::section_label("Put it on", cx))
                .child(
                    h_flex()
                        .gap(SPACE_3)
                        .child(choice("pivot-new-sheet", "New sheet", d.new_sheet, true))
                        .child(choice(
                            "pivot-existing",
                            "Existing sheet",
                            !d.new_sheet,
                            false,
                        )),
                )
                .when(!d.new_sheet, |c| c.child(Input::new(&d.dest).small()))
                .when_some(d.error.clone(), |c, e| {
                    c.child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.danger)
                            .child(SharedString::from(e)),
                    )
                })
                .child(
                    h_flex()
                        .justify_end()
                        .gap(SPACE_3)
                        .child(
                            Button::new("pivot-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .on_click(move |_, w, cx| {
                                    let _ = cancel_view
                                        .update(cx, |this, cx| this.close_pivot_dialog(w, cx));
                                }),
                        )
                        .child(
                            Button::new("pivot-ok")
                                .small()
                                .primary()
                                .label("Create")
                                .disabled(!check_ok)
                                .on_click(move |_, w, cx| {
                                    let _ =
                                        ok_view.update(cx, |this, cx| this.pivot_dialog_ok(w, cx));
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    // -------------------------------------------------------------- delete

    /// Delete the pivot table at the active cell.
    pub fn delete_pivot_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.delete_pivot(sheet, row, col);
            grid.resize();
            table.refresh(cx);
            res
        });
        self.status = match result {
            Ok(name) => {
                if self.pivot_panel.as_ref().is_some_and(|p| p.name == name) {
                    self.pivot_panel = None;
                }
                format!("{name} deleted").into()
            }
            Err(e) => format!("{e:#}").into(),
        };
        self.after_pivot_edit(window, cx);
    }

    // -------------------------------------------------------------- the panel

    fn open_panel(&mut self, sheet: u32, name: String) {
        self.pivot_panel = Some(FieldPanel {
            sheet,
            name,
            layout: None,
            revision: None,
            settings: None,
            picker: None,
        });
    }

    /// Open (or close) the PivotTable Fields panel for the pivot table at
    /// the active cell.
    pub fn pivot_fields_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let found = {
            let grid = self.table.read(cx).delegate();
            grid.book
                .pivot_at(grid.sheet, row, col)
                .map(|p| (grid.sheet, p.name))
        };
        match found {
            Some((sheet, name)) => {
                if self.pivot_panel.as_ref().is_some_and(|p| p.name == name) {
                    self.pivot_panel = None;
                } else {
                    self.open_panel(sheet, name);
                }
            }
            None if self.pivot_panel.is_some() => self.pivot_panel = None,
            None => {
                self.status = "Select a cell in a PivotTable to see its fields".into();
            }
        }
        cx.notify();
    }

    /// The pivot the panel shows: `(sheet, top, left)` of its body now.
    fn panel_anchor(&self, cx: &App) -> Option<(u32, u32, u32)> {
        let p = self.pivot_panel.as_ref()?;
        let grid = self.table.read(cx).delegate();
        let sheets = grid.book.sheet_names().len() as u32;
        // Its sheet first, then the others (an undo can move it).
        std::iter::once(p.sheet)
            .chain((0..sheets).filter(|s| *s != p.sheet))
            .filter(|s| *s < sheets)
            .find_map(|s| {
                grid.book
                    .pivot_tables(s)
                    .into_iter()
                    .find(|i| i.name == p.name)
                    .map(|i| (s, i.area.top, i.area.left))
            })
    }

    /// Bring the panel up to date: follow the active cell into another
    /// pivot, read the layout again after an edit.
    fn sync_panel(&mut self, cx: &mut Context<Self>) {
        if self.pivot_panel.is_none() {
            return;
        }
        let (row, col) = self.selected;
        let (here, revision) = {
            let grid = self.table.read(cx).delegate();
            (
                grid.book
                    .pivot_at(grid.sheet, row, col)
                    .map(|p| (grid.sheet, p.name)),
                grid.book.revision(),
            )
        };
        if let Some((sheet, name)) = here
            && self.pivot_panel.as_ref().is_some_and(|p| p.name != name)
        {
            self.open_panel(sheet, name);
        }
        if self.pivot_panel.as_ref().and_then(|p| p.revision) == Some(revision) {
            return;
        }
        let anchor = self.panel_anchor(cx);
        let layout = anchor.and_then(|(s, r, c)| {
            let grid = self.table.read(cx).delegate();
            grid.book.pivot_layout(s, r, c)
        });
        if let Some(p) = self.pivot_panel.as_mut() {
            if let Some((s, _, _)) = anchor {
                p.sheet = s;
            }
            p.layout = layout;
            p.revision = Some(revision);
            if p.layout.is_none() {
                p.settings = None;
                p.picker = None;
            }
        }
    }

    /// Apply a changed layout to the panel's pivot (one undo step).
    fn apply_layout(&mut self, layout: PivotLayout, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let Some((sheet, row, col)) = self.panel_anchor(cx) else {
            return;
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = grid.book.set_pivot_layout(sheet, row, col, &layout);
            grid.resize();
            table.refresh(cx);
            res
        });
        self.status = match result {
            Ok(()) => format!("{} updated", layout.name).into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.after_pivot_edit(window, cx);
    }

    /// Change the panel's layout with `f` and apply it.
    fn edit_layout(
        &mut self,
        f: impl FnOnce(&mut PivotLayout),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut layout) = self.pivot_panel.as_ref().and_then(|p| p.layout.clone()) else {
            return;
        };
        let before = layout.clone();
        f(&mut layout);
        if layout != before {
            self.apply_layout(layout, window, cx);
        }
    }

    fn pivot_drop(
        &mut self,
        drag: &PivotDrag,
        area: &'static str,
        at: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = drag.entry;
        self.edit_layout(
            |l| {
                let at = at.unwrap_or(usize::MAX);
                match (entry, area) {
                    // From the field list into Values: added (it may stay
                    // on an axis too, as in Excel).
                    (AreaEntry::Field(f), "Values") if l.area_of(f) != Some("Values") => {
                        let n = l.values.len();
                        l.add_value(f);
                        let k = l.values.len() - 1;
                        l.move_entry(AreaEntry::Value(k), "Values", at.min(n));
                    }
                    _ => l.move_entry(entry, area, at),
                }
            },
            window,
            cx,
        );
    }

    fn open_value_settings(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.pivot_panel.as_ref().and_then(|p| p.layout.clone()) else {
            return;
        };
        let Some(v) = layout.values.get(k) else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Custom name"));
        let caption = layout.value_caption(k);
        name.update(cx, |i, cx| {
            i.set_value(caption, window, cx);
            i.focus(window, cx);
        });
        let sub = cx.subscribe_in(&name, window, |this, _, e: &InputEvent, w, cx| {
            if let InputEvent::PressEnter { .. } = e {
                this.apply_value_settings(w, cx);
            }
        });
        let (func, num_fmt) = (v.func, v.num_fmt.clone());
        if let Some(p) = self.pivot_panel.as_mut() {
            p.picker = None;
            p.settings = Some(ValueSettings {
                k,
                name,
                func,
                num_fmt,
                _sub: sub,
            });
        }
        cx.notify();
    }

    fn apply_value_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.pivot_panel.as_mut().and_then(|p| p.settings.take()) else {
            return;
        };
        let typed = s.name.read(cx).value().trim().to_owned();
        self.edit_layout(
            |l| {
                if s.k >= l.values.len() {
                    return;
                }
                let caption_with = |l: &PivotLayout, func: PivotFunc| {
                    let mut probe = l.clone();
                    probe.values[s.k].func = func;
                    probe.values[s.k].name = None;
                    probe.value_caption(s.k)
                };
                let default = caption_with(l, s.func);
                let old_default = caption_with(l, l.values[s.k].func);
                let v = &mut l.values[s.k];
                v.func = s.func;
                v.num_fmt = s.num_fmt.clone();
                // An untouched default name follows the function.
                v.name = if typed.is_empty() || typed == default || typed == old_default {
                    None
                } else {
                    Some(typed.clone())
                };
            },
            window,
            cx,
        );
        self.focus_grid(window, cx);
    }

    fn open_picker(&mut self, field: usize, cx: &mut Context<Self>) {
        let Some(p) = self.pivot_panel.as_mut() else {
            return;
        };
        let Some(l) = p.layout.as_ref() else { return };
        let Some(f) = l.fields.get(field) else { return };
        let chosen = l
            .filters
            .iter()
            .position(|x| *x == field)
            .and_then(|k| l.filter_item.get(k).cloned().flatten());
        let checked = f
            .items
            .iter()
            .map(|it| match &chosen {
                Some(c) => &it.label == c,
                None => !it.hidden,
            })
            .collect();
        p.settings = None;
        p.picker = Some(FilterPicker { field, checked });
        cx.notify();
    }

    fn apply_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pick) = self.pivot_panel.as_mut().and_then(|p| p.picker.take()) else {
            return;
        };
        if !pick.checked.iter().any(|c| *c) {
            self.status = "Choose at least one item".into();
            if let Some(p) = self.pivot_panel.as_mut() {
                p.picker = Some(pick);
            }
            cx.notify();
            return;
        }
        self.edit_layout(
            |l| {
                let f = pick.field;
                let all = pick.checked.iter().all(|c| *c);
                let one = (pick.checked.iter().filter(|c| **c).count() == 1)
                    .then(|| pick.checked.iter().position(|c| *c))
                    .flatten();
                let page = l.filters.iter().position(|x| *x == f);
                let items = &mut l.fields[f].items;
                match (page, one) {
                    // A report filter showing one item.
                    (Some(k), Some(i)) if !all => {
                        let label = items[i].label.clone();
                        items.iter_mut().for_each(|it| it.hidden = false);
                        l.filter_item[k] = Some(label);
                    }
                    _ => {
                        for (it, c) in items.iter_mut().zip(&pick.checked) {
                            it.hidden = !c && !all;
                        }
                        if let Some(k) = page {
                            l.filter_item[k] = None;
                        }
                    }
                }
            },
            window,
            cx,
        );
    }

    pub(crate) fn render_pivot_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_panel(cx);
        let p = self.pivot_panel.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let header = {
            let close = view.clone();
            h_flex()
                .flex_none()
                .gap(SPACE_3)
                .child(
                    div()
                        .flex_1()
                        .text_size(FONT_EMPHASIS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("PivotTable Fields"),
                )
                .child(
                    Button::new("pivot-panel-close")
                        .ghost()
                        .xsmall()
                        .icon(Lucide::X)
                        .tooltip("Close")
                        .on_click(move |_, _, cx| {
                            let _ = close.update(cx, |this, cx| {
                                this.pivot_panel = None;
                                cx.notify();
                            });
                        }),
                )
        };
        let body: AnyElement = match &p.layout {
            None => div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child("Select a cell in a PivotTable to see its fields.")
                .into_any_element(),
            Some(l) => self.render_layout(l, p, cx).into_any_element(),
        };
        Some(
            v_flex()
                .id("pivot-panel")
                .key_context("KantorPivotPanel")
                .flex_none()
                .w(PANEL_WIDTH)
                .h_full()
                .gap(SPACE_4)
                .p(SPACE_5)
                .bg(t.chrome)
                .border_l_1()
                .border_color(t.border)
                .text_size(FONT_BODY)
                .text_color(t.fg)
                .overflow_y_scroll()
                .on_action(cx.listener(|this, _: &input::Escape, w, cx| {
                    if let Some(p) = this.pivot_panel.as_mut() {
                        p.settings = None;
                        p.picker = None;
                    }
                    this.focus_grid(w, cx);
                    cx.notify();
                }))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_layout(&self, l: &PivotLayout, p: &FieldPanel, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let locked = l.refresh_problem.is_some();
        let mut out = v_flex().gap(SPACE_4).child(
            div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(SharedString::from(format!("{} · {}", l.name, l.source))),
        );
        if let Some(why) = &l.refresh_problem {
            out = out.child(div().text_size(FONT_SMALL).text_color(t.warning).child(
                SharedString::from(format!("Can't be rearranged here: {why}")),
            ));
        }
        // The source's fields.
        let mut list = v_flex()
            .id("pivot-field-list")
            .max_h(px(220.))
            .overflow_y_scroll()
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .py(SPACE_2);
        for (f, field) in l.fields.iter().enumerate() {
            let checked = l.area_of(f).is_some();
            let v = view.clone();
            let menu_view = view.clone();
            let label: SharedString = field.name.clone().into();
            let drag = PivotDrag {
                entry: AreaEntry::Field(f),
                label: label.clone(),
            };
            list = list.child(
                h_flex()
                    .id(("pivot-field", f))
                    .px(SPACE_3)
                    .h(CONTROL_MD)
                    .gap(SPACE_3)
                    .hover(|s| s.bg(t.hover))
                    .when(!locked, |d| {
                        d.cursor_grab().on_drag(drag, |d: &PivotDrag, _, _, cx| {
                            cx.new(|_| DragChip {
                                label: d.label.clone(),
                            })
                        })
                    })
                    .child(
                        Checkbox::new(("pivot-field-check", f))
                            .checked(checked)
                            .disabled(locked)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.edit_layout(|l| l.toggle_field(f), w, cx)
                                });
                            }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when_some(l.area_of(f), |d, a| {
                        d.child(div().text_size(FONT_CAPTION).text_color(t.fg_dim).child(a))
                    })
                    .child(
                        Button::new(("pivot-field-menu", f))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .disabled(locked)
                            .tooltip("Add to an area")
                            .dropdown_menu(move |mut m, _, _| {
                                for area in AREAS {
                                    let v = menu_view.clone();
                                    m = m.item(
                                        PopupMenuItem::new(format!("Add to {area}")).on_click(
                                            move |_, w, cx| {
                                                let _ = v.update(cx, |this, cx| {
                                                    let d = PivotDrag {
                                                        entry: AreaEntry::Field(f),
                                                        label: SharedString::default(),
                                                    };
                                                    this.pivot_drop(&d, area, None, w, cx)
                                                });
                                            },
                                        ),
                                    );
                                }
                                m
                            }),
                    ),
            );
        }
        out = out
            .child(kantor_ui::components::section_label(
                "Fields · check, or drag to an area",
                cx,
            ))
            .child(list);
        // The four areas, two by two as in Excel.
        let mut boxes = AREAS
            .iter()
            .map(|area| self.render_area(l, area, locked, cx))
            .collect::<Vec<_>>()
            .into_iter();
        let mut pair = || {
            h_flex()
                .items_start()
                .gap(SPACE_3)
                .children(boxes.next())
                .children(boxes.next())
        };
        out = out.child(pair()).child(pair());
        if let Some(s) = &p.settings {
            out = out.child(self.render_value_settings(l, s, cx));
        }
        if let Some(pick) = &p.picker {
            out = out.child(self.render_picker(l, pick, cx));
        }
        out.child(self.render_options(l, locked, cx))
    }

    fn render_area(
        &self,
        l: &PivotLayout,
        area: &'static str,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let icon = match area {
            "Filters" => Lucide::ListFilter,
            "Columns" => Lucide::Columns2,
            "Rows" => Lucide::Rows2,
            _ => Lucide::Sigma,
        };
        // (entry, label) of each chip.
        let chips: Vec<(AreaEntry, String)> = match area {
            "Filters" => l
                .filters
                .iter()
                .map(|f| (AreaEntry::Field(*f), l.fields[*f].name.clone()))
                .collect(),
            "Columns" | "Rows" => {
                let list = if area == "Rows" { &l.rows } else { &l.columns };
                list.iter()
                    .map(|e| match e {
                        AxisEntry::Field(f) => (AreaEntry::Field(*f), l.fields[*f].name.clone()),
                        AxisEntry::Values => (AreaEntry::Values, "Σ Values".to_owned()),
                    })
                    .collect()
            }
            _ => (0..l.values.len())
                .map(|k| (AreaEntry::Value(k), l.value_caption(k)))
                .collect(),
        };
        let n = chips.len();
        let drop_view = view.clone();
        let (accent, accent_soft) = (t.accent, t.accent_soft);
        let mut b = v_flex()
            .id(SharedString::from(format!("pivot-area-{area}")))
            .flex_1()
            .min_w_0()
            .min_h(px(84.))
            .gap(SPACE_2)
            .p(SPACE_3)
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .drag_over::<PivotDrag>(move |s, _, _, _| s.border_color(accent).bg(accent_soft))
            .on_drop(move |d: &PivotDrag, w, cx| {
                let _ = drop_view.update(cx, |this, cx| this.pivot_drop(d, area, None, w, cx));
            })
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .text_size(FONT_CAPTION)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg_dim)
                    .child(gpui_kit::component::Icon::new(icon).with_size(ICON_SM))
                    .child(area.to_uppercase()),
            );
        for (k, (entry, label)) in chips.into_iter().enumerate() {
            let label: SharedString = label.into();
            let drag = PivotDrag {
                entry,
                label: label.clone(),
            };
            let chip_drop = view.clone();
            let menu_view = view.clone();
            b = b.child(
                h_flex()
                    .id(SharedString::from(format!("pivot-chip-{area}-{k}")))
                    .h(CONTROL_SM)
                    .pl(SPACE_3)
                    .gap(SPACE_1)
                    .rounded(RADIUS_SM)
                    .bg(t.selected)
                    .text_size(FONT_SMALL)
                    .when(!locked, |d| {
                        d.cursor_grab().on_drag(drag, |d: &PivotDrag, _, _, cx| {
                            cx.new(|_| DragChip {
                                label: d.label.clone(),
                            })
                        })
                    })
                    .drag_over::<PivotDrag>(move |s, _, _, _| s.border_t_2().border_color(accent))
                    .on_drop(move |d: &PivotDrag, w, cx| {
                        cx.stop_propagation();
                        let _ = chip_drop
                            .update(cx, |this, cx| this.pivot_drop(d, area, Some(k), w, cx));
                    })
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        Button::new(SharedString::from(format!("pivot-chip-menu-{area}-{k}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .disabled(locked)
                            .dropdown_menu(move |m, _, _| {
                                chip_menu(m, &menu_view, entry, area, k, n)
                            }),
                    ),
            );
        }
        b
    }

    fn render_value_settings(
        &self,
        l: &PivotLayout,
        s: &ValueSettings,
        cx: &mut Context<Self>,
    ) -> Div {
        let view = cx.entity().downgrade();
        let field = l.values.get(s.k).map(|v| v.field);
        let source_fmt = field
            .and_then(|f| l.fields.get(f))
            .and_then(|f| f.num_fmt.clone());
        let func_view = view.clone();
        let fmt_view = view.clone();
        let ok = view.clone();
        let cancel = view.clone();
        let fmt_label = match &s.num_fmt {
            None => "General".to_owned(),
            Some(c) => NUM_FMTS
                .iter()
                .find(|(_, code)| code == c)
                .map(|(n, _)| (*n).to_owned())
                .unwrap_or_else(|| c.clone()),
        };
        let current = s.num_fmt.clone();
        card(cx)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Value Field Settings"),
            )
            .child(kantor_ui::components::section_label("Name", cx))
            .child(Input::new(&s.name).small())
            .child(kantor_ui::components::section_label(
                "Summarize values by",
                cx,
            ))
            .child(
                Button::new("pivot-func")
                    .outline()
                    .small()
                    .label(s.func.label())
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        for f in PivotFunc::ALL {
                            let v = func_view.clone();
                            m = m.item(PopupMenuItem::new(f.label()).on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(s) =
                                        this.pivot_panel.as_mut().and_then(|p| p.settings.as_mut())
                                    {
                                        s.func = f;
                                    }
                                    cx.notify();
                                });
                            }));
                        }
                        m
                    }),
            )
            .child(kantor_ui::components::section_label("Number format", cx))
            .child(
                Button::new("pivot-numfmt")
                    .outline()
                    .small()
                    .label(fmt_label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut m, _, _| {
                        let mut choices: Vec<(String, Option<String>)> =
                            vec![("General".into(), None)];
                        if let Some(c) = &source_fmt {
                            choices.push((format!("As in the source ({c})"), Some(c.clone())));
                        }
                        choices.extend(
                            NUM_FMTS
                                .iter()
                                .map(|(n, c)| ((*n).to_owned(), Some((*c).to_owned()))),
                        );
                        for (label, code) in choices {
                            let v = fmt_view.clone();
                            let on = code == current;
                            m = m.item(PopupMenuItem::new(label).checked(on).on_click(
                                move |_, _, cx| {
                                    let code = code.clone();
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(s) = this
                                            .pivot_panel
                                            .as_mut()
                                            .and_then(|p| p.settings.as_mut())
                                        {
                                            s.num_fmt = code;
                                        }
                                        cx.notify();
                                    });
                                },
                            ));
                        }
                        m
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pivot-vs-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(move |_, _, cx| {
                                let _ = cancel.update(cx, |this, cx| {
                                    if let Some(p) = this.pivot_panel.as_mut() {
                                        p.settings = None;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Button::new("pivot-vs-ok")
                            .primary()
                            .small()
                            .label("Apply")
                            .on_click(move |_, w, cx| {
                                let _ = ok.update(cx, |this, cx| this.apply_value_settings(w, cx));
                            }),
                    ),
            )
    }

    fn render_picker(&self, l: &PivotLayout, pick: &FilterPicker, cx: &mut Context<Self>) -> Div {
        let view = cx.entity().downgrade();
        let Some(field) = l.fields.get(pick.field) else {
            return div();
        };
        let all = pick.checked.iter().all(|c| *c);
        let all_view = view.clone();
        let mut items = v_flex()
            .id("pivot-picker-items")
            .max_h(px(180.))
            .overflow_y_scroll()
            .gap(SPACE_1);
        for (i, it) in field.items.iter().enumerate() {
            let v = view.clone();
            items = items.child(
                Checkbox::new(("pivot-pick", i))
                    .label(SharedString::from(it.label.clone()))
                    .checked(pick.checked.get(i).copied().unwrap_or(false))
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(p) =
                                this.pivot_panel.as_mut().and_then(|p| p.picker.as_mut())
                                && let Some(c) = p.checked.get_mut(i)
                            {
                                *c = on;
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let ok = view.clone();
        let cancel = view.clone();
        card(cx)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("Show items of {}", field.name))),
            )
            .child(
                Checkbox::new("pivot-pick-all")
                    .label("(All)")
                    .checked(all)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = all_view.update(cx, |this, cx| {
                            if let Some(p) =
                                this.pivot_panel.as_mut().and_then(|p| p.picker.as_mut())
                            {
                                p.checked.iter_mut().for_each(|c| *c = on);
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(items)
            .child(
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pivot-pick-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(move |_, _, cx| {
                                let _ = cancel.update(cx, |this, cx| {
                                    if let Some(p) = this.pivot_panel.as_mut() {
                                        p.picker = None;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Button::new("pivot-pick-ok")
                            .primary()
                            .small()
                            .label("Apply")
                            .on_click(move |_, w, cx| {
                                let _ = ok.update(cx, |this, cx| this.apply_picker(w, cx));
                            }),
                    ),
            )
    }

    fn render_options(&self, l: &PivotLayout, locked: bool, cx: &mut Context<Self>) -> Div {
        let view = cx.entity().downgrade();
        let lay_view = view.clone();
        let sub_view = view.clone();
        let row_view = view.clone();
        let col_view = view.clone();
        let (layout, subtotals) = (l.layout, l.subtotals);
        v_flex()
            .gap(SPACE_3)
            .child(kantor_ui::components::section_label("Layout", cx))
            .child(
                Button::new("pivot-report-layout")
                    .outline()
                    .small()
                    .label(layout_label(layout))
                    .dropdown_caret(true)
                    .disabled(locked)
                    .dropdown_menu(move |mut m, _, _| {
                        for r in [
                            ReportLayout::Compact,
                            ReportLayout::Outline,
                            ReportLayout::Tabular,
                        ] {
                            let v = lay_view.clone();
                            m = m.item(
                                PopupMenuItem::new(layout_label(r))
                                    .checked(r == layout)
                                    .on_click(move |_, w, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.edit_layout(|l| l.layout = r, w, cx)
                                        });
                                    }),
                            );
                        }
                        m
                    }),
            )
            .child(
                Button::new("pivot-subtotals")
                    .outline()
                    .small()
                    .label(subtotal_label(subtotals))
                    .dropdown_caret(true)
                    .disabled(locked)
                    .dropdown_menu(move |mut m, _, _| {
                        for s in [
                            SubtotalPlace::Off,
                            SubtotalPlace::Top,
                            SubtotalPlace::Bottom,
                        ] {
                            let v = sub_view.clone();
                            m = m.item(
                                PopupMenuItem::new(subtotal_label(s))
                                    .checked(s == subtotals)
                                    .on_click(move |_, w, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.edit_layout(|l| l.subtotals = s, w, cx)
                                        });
                                    }),
                            );
                        }
                        m
                    }),
            )
            .child(
                Checkbox::new("pivot-grand-row")
                    .label("Grand total row")
                    .checked(l.grand_total_row)
                    .disabled(locked)
                    .on_click(move |on, w, cx| {
                        let on = *on;
                        let _ = row_view.update(cx, |this, cx| {
                            this.edit_layout(|l| l.grand_total_row = on, w, cx)
                        });
                    }),
            )
            .child(
                Checkbox::new("pivot-grand-col")
                    .label("Grand total column")
                    .checked(l.grand_total_column)
                    .disabled(locked)
                    .on_click(move |on, w, cx| {
                        let on = *on;
                        let _ = col_view.update(cx, |this, cx| {
                            this.edit_layout(|l| l.grand_total_column = on, w, cx)
                        });
                    }),
            )
    }

    pub(crate) fn pivot_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &RefreshPivotTable, w, cx| this.refresh_pivots_doc(false, w, cx)),
        )
        .on_action(cx.listener(|this, _: &RefreshAllPivotTables, w, cx| {
            this.refresh_pivots_doc(true, w, cx)
        }))
        .on_action(cx.listener(|this, _: &InsertPivotTable, w, cx| this.insert_pivot_doc(w, cx)))
        .on_action(cx.listener(|this, _: &PivotTableFields, w, cx| this.pivot_fields_doc(w, cx)))
        .on_action(cx.listener(|this, _: &DeletePivotTable, w, cx| this.delete_pivot_doc(w, cx)))
    }
}

/// A small card inside the panel.
fn card(cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .gap(SPACE_3)
        .p(SPACE_4)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border_strong)
}

/// The menu of a chip in an area.
fn chip_menu(
    mut m: PopupMenu,
    view: &WeakEntity<SheetsView>,
    entry: AreaEntry,
    area: &'static str,
    k: usize,
    n: usize,
) -> PopupMenu {
    let act = |m: PopupMenu, label: String, f: ChipAction| {
        let v = view.clone();
        m.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| f(this, w, cx));
        }))
    };
    if k > 0 {
        m = act(
            m,
            "Move Up".into(),
            Box::new(move |this, w, cx| {
                this.edit_layout(|l| l.move_entry(entry, area, k - 1), w, cx)
            }),
        );
    }
    if k + 1 < n {
        m = act(
            m,
            "Move Down".into(),
            Box::new(move |this, w, cx| {
                this.edit_layout(|l| l.move_entry(entry, area, k + 1), w, cx)
            }),
        );
    }
    m = m.separator();
    for to in AREAS {
        if to == area || (entry == AreaEntry::Values && !matches!(to, "Rows" | "Columns")) {
            continue;
        }
        m = act(
            m,
            format!("Move to {to}"),
            Box::new(move |this, w, cx| {
                let d = PivotDrag {
                    entry,
                    label: SharedString::default(),
                };
                this.pivot_drop(&d, to, None, w, cx)
            }),
        );
    }
    match entry {
        AreaEntry::Value(k) => {
            m = act(
                m.separator(),
                "Value Field Settings…".into(),
                Box::new(move |this, w, cx| this.open_value_settings(k, w, cx)),
            );
            m = act(
                m,
                "Remove".into(),
                Box::new(move |this, w, cx| this.edit_layout(|l| l.remove_value(k), w, cx)),
            );
        }
        AreaEntry::Field(f) => {
            m = act(
                m.separator(),
                if area == "Filters" {
                    "Choose Items…".into()
                } else {
                    "Filter Items…".into()
                },
                Box::new(move |this, _, cx| this.open_picker(f, cx)),
            );
            m = act(
                m,
                "Remove".into(),
                Box::new(move |this, w, cx| this.edit_layout(|l| l.remove_field(f), w, cx)),
            );
        }
        AreaEntry::Values => {}
    }
    m
}

/// `Sheet1!H2`, `'My sheet'!$H$2` or `H2` (on `sheet`) as `(sheet, row,
/// col)`.
fn parse_cell(text: &str, names: &[String], sheet: u32) -> Result<(u32, u32, u32), String> {
    let text = text.trim().trim_start_matches('=');
    let (s, r) = match text.rsplit_once('!') {
        Some((s, r)) => {
            let s = s.trim();
            let s = s
                .strip_prefix('\'')
                .and_then(|x| x.strip_suffix('\''))
                .map(|x| x.replace("''", "'"))
                .unwrap_or_else(|| s.to_owned());
            let k = names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(&s))
                .ok_or_else(|| format!("there is no sheet {s:?}"))?;
            (k as u32, r)
        }
        None => (sheet, text),
    };
    let range = sheets_engine::formula::parse_range(&r.replace('$', ""))
        .ok_or_else(|| format!("{text:?} is not a cell: type one like Sheet1!H2"))?;
    Ok((s, range.top, range.left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn destination_cells_are_read() {
        let names = vec!["Sheet1".to_owned(), "My data".to_owned()];
        assert_eq!(parse_cell("H2", &names, 0), Ok((0, 1, 7)));
        assert_eq!(parse_cell("'My data'!$B$3", &names, 0), Ok((1, 2, 1)));
        assert_eq!(parse_cell("sheet1!A1:B2", &names, 1), Ok((0, 0, 0)));
        assert!(parse_cell("Nope!A1", &names, 0).is_err());
        assert!(parse_cell("hello", &names, 0).is_err());
    }
}
