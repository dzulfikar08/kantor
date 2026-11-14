//! Excel tables, sort and AutoFilter in the grid.
//!
//! * Table cells get their table style (bands, header and total rows) under
//!   their own formatting; see `SheetGrid::table_style`.
//! * A filter's header row (a table's with filter buttons, or the sheet's
//!   AutoFilter) has a button in each cell: a chevron, or a funnel on a
//!   filtered column. It opens the filter menu: sort A→Z / Z→A, a condition
//!   (text, number or date, by what the column holds), and a checkbox list of
//!   the column's values with a search box. OK applies, Clear removes the
//!   column's filter.
//! * Sort ascending / descending, Custom Sort (several levels, header row)
//!   and Filter (Ctrl+Shift+L), Clear and Reapply are grid commands, in the
//!   palette and the column header menu.
//!
//! The work is `sheets_engine`'s (`filter`, `sort`, `table`); filtered rows
//! are hidden rows, which the row layout already skips.

use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, t_dyn, tf};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{
    CellRange, ColumnKind, Condition, FilterKind, FilterOp, FilterScope, FilterValue, SortKey,
    Workbook, column_name, date_serial,
};

use crate::SheetsView;

/// Messages of `sheets_engine` (filter and sort) that reach the status bar
/// through [`SheetsView::book_op`], translated there.
#[cfg(test)]
const ENGINE_ERRORS: &[&str] = &[
    i18n::tk("a table without a header row has no filter buttons"),
    i18n::tk("select the data to filter"),
    i18n::tk("nothing to filter"),
    i18n::tk("nothing to sort"),
    i18n::tk("nothing to sort by"),
    i18n::tk("the sort column is outside the range"),
];

/// Values listed at most (search narrows a longer list).
const MAX_LISTED: usize = 500;

/// A condition the menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cond {
    None,
    Equals,
    NotEquals,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Between,
    BeginsWith,
    EndsWith,
    Contains,
    NotContains,
    Before,
    After,
}

impl Cond {
    fn label(self) -> &'static str {
        match self {
            Cond::None => t("No condition"),
            Cond::Equals => t("Equals"),
            Cond::NotEquals => t("Does not equal"),
            Cond::Greater => t("Greater than"),
            Cond::GreaterEq => t("Greater than or equal to"),
            Cond::Less => t("Less than"),
            Cond::LessEq => t("Less than or equal to"),
            Cond::Between => t("Between"),
            Cond::BeginsWith => t("Begins with"),
            Cond::EndsWith => t("Ends with"),
            Cond::Contains => t("Contains"),
            Cond::NotContains => t("Does not contain"),
            Cond::Before => t("Before"),
            Cond::After => t("After"),
        }
    }

    fn for_kind(kind: ColumnKind) -> &'static [Cond] {
        match kind {
            ColumnKind::Text => &[
                Cond::None,
                Cond::Equals,
                Cond::NotEquals,
                Cond::BeginsWith,
                Cond::EndsWith,
                Cond::Contains,
                Cond::NotContains,
            ],
            ColumnKind::Number => &[
                Cond::None,
                Cond::Equals,
                Cond::NotEquals,
                Cond::Greater,
                Cond::GreaterEq,
                Cond::Less,
                Cond::LessEq,
                Cond::Between,
            ],
            ColumnKind::Date => &[
                Cond::None,
                Cond::Equals,
                Cond::Before,
                Cond::After,
                Cond::Between,
            ],
        }
    }

    /// The filter for this condition with `a` (and `b` for between), or why
    /// not.
    fn filter(self, kind: ColumnKind, a: &str, b: &str) -> Result<Option<FilterKind>, String> {
        let a = a.trim();
        let b = b.trim();
        if self == Cond::None {
            return Ok(None);
        }
        if a.is_empty() {
            return Err(t("Type a value for the condition").into());
        }
        let value = |s: &str| -> Result<String, String> {
            match kind {
                ColumnKind::Date => date_serial(s)
                    .map(|n| n.to_string())
                    .ok_or_else(|| tf("{} is not a date (use 2025-03-01)", &[&format!("{s:?}")])),
                ColumnKind::Number => s
                    .replace(',', "")
                    .parse::<f64>()
                    .map(|n| n.to_string())
                    .map_err(|_| tf("{} is not a number", &[&format!("{s:?}")])),
                ColumnKind::Text => Ok(s.to_owned()),
            }
        };
        let one = |op: FilterOp, v: String| {
            Ok(Some(FilterKind::Custom {
                and: false,
                conditions: vec![Condition { op, value: v }],
            }))
        };
        let esc = |s: &str| s.replace('~', "~~").replace('*', "~*").replace('?', "~?");
        match self {
            Cond::None => Ok(None),
            Cond::Equals => one(FilterOp::Equal, value(a)?),
            Cond::NotEquals => one(FilterOp::NotEqual, value(a)?),
            Cond::Greater | Cond::After => one(FilterOp::GreaterThan, value(a)?),
            Cond::GreaterEq => one(FilterOp::GreaterThanOrEqual, value(a)?),
            Cond::Less | Cond::Before => one(FilterOp::LessThan, value(a)?),
            Cond::LessEq => one(FilterOp::LessThanOrEqual, value(a)?),
            Cond::BeginsWith => one(FilterOp::Equal, format!("{}*", esc(a))),
            Cond::EndsWith => one(FilterOp::Equal, format!("*{}", esc(a))),
            Cond::Contains => one(FilterOp::Equal, format!("*{}*", esc(a))),
            Cond::NotContains => one(FilterOp::NotEqual, format!("*{}*", esc(a))),
            Cond::Between => {
                if b.is_empty() {
                    return Err(t("Type both ends of the range").into());
                }
                Ok(Some(FilterKind::Custom {
                    and: true,
                    conditions: vec![
                        Condition {
                            op: FilterOp::GreaterThanOrEqual,
                            value: value(a)?,
                        },
                        Condition {
                            op: FilterOp::LessThanOrEqual,
                            value: value(b)?,
                        },
                    ],
                }))
            }
        }
    }

    /// The condition (and its values as typed) a column filter shows as.
    fn from_filter(kind: &FilterKind, column: ColumnKind) -> Option<(Cond, String, String)> {
        let FilterKind::Custom { and, conditions } = kind else {
            return None;
        };
        let shown = |v: &str| match column {
            ColumnKind::Date => v
                .parse::<f64>()
                .ok()
                .map(|n| {
                    let (y, m, d) = sheets_engine::ymd(n);
                    format!("{y:04}-{m:02}-{d:02}")
                })
                .unwrap_or_else(|| v.to_owned()),
            _ => v.to_owned(),
        };
        match conditions.as_slice() {
            [a, b]
                if *and
                    && a.op == FilterOp::GreaterThanOrEqual
                    && b.op == FilterOp::LessThanOrEqual =>
            {
                Some((Cond::Between, shown(&a.value), shown(&b.value)))
            }
            [c] => {
                let v = &c.value;
                let inner = |s: &str| s.replace("~*", "*").replace("~?", "?").replace("~~", "~");
                let wild = |s: &str| s.starts_with('*') && s.ends_with('*') && s.len() > 1;
                let cond = match c.op {
                    FilterOp::Equal if wild(v) => {
                        return Some((Cond::Contains, inner(&v[1..v.len() - 1]), String::new()));
                    }
                    FilterOp::NotEqual if wild(v) => {
                        return Some((Cond::NotContains, inner(&v[1..v.len() - 1]), String::new()));
                    }
                    FilterOp::Equal if v.ends_with('*') && !v.ends_with("~*") => {
                        return Some((Cond::BeginsWith, inner(&v[..v.len() - 1]), String::new()));
                    }
                    FilterOp::Equal if v.starts_with('*') => {
                        return Some((Cond::EndsWith, inner(&v[1..]), String::new()));
                    }
                    FilterOp::Equal => Cond::Equals,
                    FilterOp::NotEqual => Cond::NotEquals,
                    FilterOp::GreaterThan if column == ColumnKind::Date => Cond::After,
                    FilterOp::GreaterThan => Cond::Greater,
                    FilterOp::GreaterThanOrEqual => Cond::GreaterEq,
                    FilterOp::LessThan if column == ColumnKind::Date => Cond::Before,
                    FilterOp::LessThan => Cond::Less,
                    FilterOp::LessThanOrEqual => Cond::LessEq,
                };
                Some((cond, shown(v), String::new()))
            }
            _ => None,
        }
    }
}

/// The open filter menu of one column.
pub(crate) struct FilterMenu {
    at: Point<Pixels>,
    sheet: u32,
    scope: FilterScope,
    col: u32,
    title: String,
    kind: ColumnKind,
    values: Vec<FilterValue>,
    checked: Vec<bool>,
    search: Entity<InputState>,
    cond: Cond,
    value1: Entity<InputState>,
    value2: Entity<InputState>,
    /// A filter Kantor keeps but can't edit (top 10, colours...).
    other: bool,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

/// Levels of the Custom Sort panel.
pub(crate) struct SortPanel {
    sheet: u32,
    range: CellRange,
    header: bool,
    levels: Vec<SortKey>,
}

impl SheetsView {
    /// Open the filter menu of column `col` of `scope`, at a window position.
    pub(crate) fn open_filter_menu(
        &mut self,
        scope: FilterScope,
        col: u32,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let grid = self.table.read(cx).delegate();
        let (book, sheet) = (&grid.book, grid.sheet);
        let Some(info) = book.filter(sheet, &scope) else {
            return;
        };
        let values = book.filter_values(sheet, &scope, col);
        let kind = book.filter_value_kind(sheet, &scope, col);
        let current = info.column(col).cloned();
        let title = header_title(book, sheet, info.area.top, col);
        let checked = values.iter().map(|v| v.checked).collect();
        let (cond, a, b) = current
            .as_ref()
            .and_then(|k| Cond::from_filter(k, kind))
            .unwrap_or((Cond::None, String::new(), String::new()));
        let other = matches!(current, Some(FilterKind::Other(_)));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("Search")));
        let value1 = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder(kind)));
        let value2 = cx.new(|cx| InputState::new(window, cx).placeholder(t("and")));
        value1.update(cx, |i, cx| i.set_value(a, window, cx));
        value2.update(cx, |i, cx| i.set_value(b, window, cx));
        let subs = vec![
            cx.subscribe_in(&search, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
                let _ = this;
            }),
            cx.subscribe_in(&value1, window, Self::on_filter_input),
            cx.subscribe_in(&value2, window, Self::on_filter_input),
        ];
        self.filter_menu = Some(FilterMenu {
            at,
            sheet,
            scope,
            col,
            title,
            kind,
            values,
            checked,
            search,
            cond,
            value1,
            value2,
            other,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn on_filter_input(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { .. } = event {
            self.apply_filter_menu(window, cx);
        }
    }

    pub(crate) fn close_filter_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_menu.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// OK: the condition if one is chosen, else the ticked values.
    fn apply_filter_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.filter_menu.as_mut() else {
            return;
        };
        let a = menu.value1.read(cx).value().to_string();
        let b = menu.value2.read(cx).value().to_string();
        let filter = if menu.cond != Cond::None {
            match menu.cond.filter(menu.kind, &a, &b) {
                Ok(f) => f,
                Err(e) => {
                    menu.error = Some(e.into());
                    cx.notify();
                    return;
                }
            }
        } else if menu.other && menu.checked.iter().all(|c| *c) {
            // Untouched: keep the filter Kantor can't edit.
            self.close_filter_menu(window, cx);
            return;
        } else {
            let query = menu.search.read(cx).value().to_lowercase();
            // With a search, only the values found count (as in Excel).
            let shown: Vec<(&FilterValue, bool)> = menu
                .values
                .iter()
                .zip(menu.checked.iter().copied())
                .filter(|(v, _)| query.is_empty() || matches_query(v, &query))
                .collect();
            if query.is_empty() && shown.iter().all(|(_, c)| *c) {
                None
            } else {
                Some(FilterKind::Values {
                    values: shown
                        .iter()
                        .filter(|(v, c)| *c && !v.text.is_empty())
                        .map(|(v, _)| v.text.clone())
                        .collect(),
                    blanks: shown.iter().any(|(v, c)| *c && v.text.is_empty()),
                    dates: Vec::new(),
                })
            }
        };
        let (sheet, scope, col) = (menu.sheet, menu.scope.clone(), menu.col);
        self.filter_menu = None;
        self.book_op(window, cx, move |b, _| {
            let (shown, total) = b.set_column_filter(sheet, &scope, col, filter)?;
            Ok(tf("{} of {} rows shown", &[&shown, &total]))
        });
        self.focus_grid(window, cx);
    }

    fn clear_filter_menu_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.filter_menu.take() else {
            return;
        };
        let (sheet, scope, col) = (menu.sheet, menu.scope, menu.col);
        self.book_op(window, cx, move |b, _| {
            let (shown, total) = b.set_column_filter(sheet, &scope, col, None)?;
            Ok(tf("Filter cleared: {} of {} rows shown", &[&shown, &total]))
        });
        self.focus_grid(window, cx);
    }

    fn sort_from_menu(&mut self, descending: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.filter_menu.take() else {
            return;
        };
        let (sheet, scope, col) = (menu.sheet, menu.scope, menu.col);
        self.book_op(window, cx, move |b, _| {
            let n = match &scope {
                FilterScope::Table(name) => b.sort_table(sheet, name, col, descending)?,
                FilterScope::Sheet => {
                    let f = b
                        .filter(sheet, &scope)
                        .ok_or_else(|| anyhow::anyhow!("no filter"))?;
                    b.sort_range(sheet, f.area, &[SortKey { col, descending }], true)?
                }
            };
            Ok(sorted_status(n, descending))
        });
        self.focus_grid(window, cx);
    }

    /// Run an edit of the workbook and refresh the grid, the formula bar and
    /// the status.
    pub(crate) fn book_op(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Workbook, u32) -> anyhow::Result<String>,
    ) {
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = op(&mut grid.book, sheet);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(msg) => msg.into(),
            Err(err) => t_dyn(&format!("{err:#}")).to_owned().into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ commands

    /// Sort ascending / descending by the active cell's column: a table's
    /// data, else the selection (several cells), else the data region around
    /// the active cell, with Excel's guess of a header row.
    pub(crate) fn sort_doc(
        &mut self,
        descending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let range = self.range(cx);
        let (row, col) = self.selected;
        self.book_op(window, cx, move |b, sheet| {
            if let Some(t) = b.table_at(sheet, row, col) {
                let n = b.sort_table(sheet, &t.name, col, descending)?;
                return Ok(sorted_status(n, descending));
            }
            let target = if range.is_single() {
                b.current_region(sheet, row, col)
            } else {
                range
            };
            let header = b.guess_header(sheet, target);
            let n = b.sort_range(sheet, target, &[SortKey { col, descending }], header)?;
            Ok(sorted_status(n, descending))
        });
    }

    /// Filter buttons on or off (Ctrl+Shift+L).
    pub(crate) fn toggle_filter_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let range = self.range(cx);
        self.book_op(window, cx, move |b, sheet| {
            Ok(if b.toggle_filter(sheet, range)? {
                t("Filter buttons on: click one to filter or sort").into()
            } else {
                t("Filter off: every row shown").into()
            })
        });
    }

    /// Clear (or reapply) every filter on the sheet.
    pub(crate) fn clear_filters_doc(
        &mut self,
        reapply: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        self.book_op(window, cx, move |b, sheet| {
            let filters = b.filters(sheet);
            if filters.is_empty() {
                anyhow::bail!(t("No filter on this sheet"));
            }
            let (mut shown, mut total) = (0, 0);
            for f in filters {
                let (s, t) = if reapply {
                    b.reapply_filter(sheet, &f.scope)?
                } else {
                    b.clear_filter(sheet, &f.scope)?
                };
                shown += s;
                total += t;
            }
            Ok(if reapply {
                tf("Filter reapplied: {} of {} rows shown", &[&shown, &total])
            } else {
                t("Filters cleared").into()
            })
        });
    }

    /// The Custom Sort panel for the selection (or the region around it).
    pub(crate) fn open_sort_panel(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let range = self.range(cx);
        let (row, col) = self.selected;
        let grid = self.table.read(cx).delegate();
        let (book, sheet) = (&grid.book, grid.sheet);
        let (range, header) = match book.table_at(sheet, row, col) {
            Some(t) => (
                CellRange {
                    top: t.header_row().unwrap_or(t.area.top),
                    bottom: t.data_rows().1,
                    ..t.area
                },
                t.header_rows > 0,
            ),
            None => {
                let r = if range.is_single() {
                    book.current_region(sheet, row, col)
                } else {
                    range
                };
                (r, book.guess_header(sheet, r))
            }
        };
        self.sort_panel = Some(SortPanel {
            sheet,
            range,
            header,
            levels: vec![SortKey {
                col: col.clamp(range.left, range.right),
                descending: false,
            }],
        });
        cx.notify();
    }

    fn apply_sort_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.sort_panel.take() else {
            return;
        };
        let SortPanel {
            sheet,
            range,
            header,
            levels,
        } = p;
        self.book_op(window, cx, move |b, _| {
            let n = b.sort_range(sheet, range, &levels, header)?;
            Ok(tf("Sorted {} rows by {} levels", &[&n, &levels.len()]))
        });
        self.focus_grid(window, cx);
    }

    // ------------------------------------------------------------ rendering

    pub(crate) fn render_filter_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.filter_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let query = menu.search.read(cx).value().to_lowercase();
        let (az, za) = match menu.kind {
            ColumnKind::Number => (
                i18n::t("Sort smallest to largest"),
                i18n::t("Sort largest to smallest"),
            ),
            ColumnKind::Date => (
                i18n::t("Sort oldest to newest"),
                i18n::t("Sort newest to oldest"),
            ),
            ColumnKind::Text => (i18n::t("Sort A to Z"), i18n::t("Sort Z to A")),
        };
        let item = |id: &'static str, label: &'static str, desc: bool| {
            h_flex()
                .id(id)
                .gap(SPACE_3)
                .px(SPACE_2)
                .h(CONTROL_SM)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(t.accent_soft))
                .child(
                    gpui_kit::component::Icon::new(if desc {
                        gpui_kit::assets::IconName::ArrowDown
                    } else {
                        gpui_kit::assets::IconName::ArrowUp
                    })
                    .size(px(14.))
                    .text_color(t.fg_muted),
                )
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| this.sort_from_menu(desc, w, cx)))
        };
        // The condition: a dropdown and one or two values.
        let cond_view = view.clone();
        let cond_button = Button::new("filter-cond")
            .outline()
            .small()
            .label(menu.cond.label())
            .dropdown_caret(true)
            .dropdown_menu({
                let kind = menu.kind;
                let current = menu.cond;
                move |mut m, _, _| {
                    for c in Cond::for_kind(kind) {
                        let c = *c;
                        let v = cond_view.clone();
                        m = m.item(
                            PopupMenuItem::new(c.label())
                                .checked(c == current)
                                .on_click(move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(menu) = this.filter_menu.as_mut() {
                                            menu.cond = c;
                                            menu.error = None;
                                        }
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    m
                }
            });
        let cond_label = match menu.kind {
            ColumnKind::Number => i18n::t("Number filter"),
            ColumnKind::Date => i18n::t("Date filter"),
            ColumnKind::Text => i18n::t("Text filter"),
        };
        let condition = v_flex()
            .gap(SPACE_2)
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(cond_label),
            )
            .child(cond_button)
            .when(menu.cond != Cond::None, |d| {
                d.child(Input::new(&menu.value1).small())
            })
            .when(menu.cond == Cond::Between, |d| {
                d.child(Input::new(&menu.value2).small())
            });
        // The values, ticked or not.
        let listed: Vec<usize> = (0..menu.values.len())
            .filter(|&i| query.is_empty() || matches_query(&menu.values[i], &query))
            .collect();
        let all_checked = listed.iter().all(|&i| menu.checked[i]);
        let all_view = view.clone();
        let shown_ix: Vec<usize> = listed.iter().copied().take(MAX_LISTED).collect();
        let more = listed.len().saturating_sub(MAX_LISTED);
        let rows = shown_ix.into_iter().map(|i| {
            let v = view.clone();
            let label = if menu.values[i].text.is_empty() {
                i18n::t("(Blanks)").to_owned()
            } else {
                menu.values[i].text.clone()
            };
            Checkbox::new(("filter-value", i))
                .label(label)
                .checked(menu.checked[i])
                .on_click(move |on, _, cx| {
                    let on = *on;
                    let _ = v.update(cx, |this, cx| {
                        if let Some(menu) = this.filter_menu.as_mut() {
                            menu.checked[i] = on;
                            menu.cond = Cond::None;
                        }
                        cx.notify();
                    });
                })
        });
        let list = v_flex()
            .id("filter-values")
            .gap(SPACE_1)
            .max_h(px(220.))
            .overflow_y_scroll()
            .p(SPACE_2)
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .child(
                Checkbox::new("filter-all")
                    .label(if query.is_empty() {
                        i18n::t("(Select All)")
                    } else {
                        i18n::t("(Select All Search Results)")
                    })
                    .checked(all_checked)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = all_view.update(cx, |this, cx| {
                            if let Some(menu) = this.filter_menu.as_mut() {
                                let q = menu.search.read(cx).value().to_lowercase();
                                for i in 0..menu.values.len() {
                                    if q.is_empty() || matches_query(&menu.values[i], &q) {
                                        menu.checked[i] = on;
                                    }
                                }
                                menu.cond = Cond::None;
                            }
                            cx.notify();
                        });
                    }),
            )
            .children(rows)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_dim)
                        .child(i18n::tf("{} more: search to narrow", &[&more])),
                )
            });
        let filtered = self
            .table
            .read(cx)
            .delegate()
            .book
            .filter(menu.sheet, &menu.scope)
            .is_some_and(|f| f.is_filtered(menu.col));
        let buttons = h_flex()
            .gap(SPACE_2)
            .child(
                Button::new("filter-clear")
                    .ghost()
                    .small()
                    .label(i18n::t("Clear filter"))
                    .disabled(!filtered)
                    .on_click(cx.listener(|this, _, w, cx| this.clear_filter_menu_column(w, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("filter-cancel")
                    .ghost()
                    .small()
                    .label(i18n::t("Cancel"))
                    .on_click(cx.listener(|this, _, w, cx| this.close_filter_menu(w, cx))),
            )
            .child(
                Button::new("filter-ok")
                    .primary()
                    .small()
                    .label(i18n::t("OK"))
                    .on_click(cx.listener(|this, _, w, cx| this.apply_filter_menu(w, cx))),
            );
        let panel = v_flex()
            .id("sheets-filter-menu")
            .occlude()
            .w(px(280.))
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_BODY)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, w, cx| this.close_filter_menu(w, cx)))
            .on_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, w, cx| {
                this.close_filter_menu(w, cx)
            }))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(SharedString::from(menu.title.clone())),
            )
            .child(
                v_flex()
                    .child(item("filter-sort-az", az, false))
                    .child(item("filter-sort-za", za, true)),
            )
            .child(div().h(GRID_LINE).bg(t.border))
            .child(condition)
            .child(div().h(GRID_LINE).bg(t.border))
            .when(menu.other, |d| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_dim)
                        .child(i18n::t("This column has a filter Kantor keeps but can't edit (top 10, colour or dynamic). Choosing values replaces it.")),
                )
            })
            .child(Input::new(&menu.search).small())
            .child(list)
            .when_some(menu.error.clone(), |d, e| {
                d.child(div().text_size(FONT_SMALL).text_color(t.danger).child(e))
            })
            .child(buttons);
        Some(
            deferred(anchored().position(menu.at).snap_to_window().child(panel)).into_any_element(),
        )
    }

    pub(crate) fn render_sort_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.sort_panel.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let grid = self.table.read(cx).delegate();
        let book = &grid.book;
        let col_label = |c: u32| -> String {
            if p.header {
                let h = book.display_value(p.sheet, p.range.top, c);
                if !h.is_empty() {
                    return h;
                }
            }
            i18n::tf("Column {}", &[&column_name(c)])
        };
        let columns: Vec<(u32, String)> = (p.range.left..=p.range.right.min(p.range.left + 200))
            .map(|c| (c, col_label(c)))
            .collect();
        let levels = p.levels.iter().enumerate().map(|(i, k)| {
            let cols = columns.clone();
            let v = view.clone();
            let v2 = view.clone();
            let v3 = view.clone();
            let key = *k;
            h_flex()
                .gap(SPACE_2)
                .child(
                    div()
                        .w(px(64.))
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .child(if i == 0 {
                            i18n::t("Sort by")
                        } else {
                            i18n::t("Then by")
                        }),
                )
                .child(
                    Button::new(("sort-col", i))
                        .outline()
                        .small()
                        .flex_1()
                        .label(col_label(key.col))
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut m, _, _| {
                            for (c, name) in cols.iter().cloned() {
                                let v = v.clone();
                                m = m.item(
                                    PopupMenuItem::new(name).checked(c == key.col).on_click(
                                        move |_, _, cx| {
                                            let _ = v.update(cx, |this, cx| {
                                                if let Some(p) = this.sort_panel.as_mut() {
                                                    p.levels[i].col = c;
                                                }
                                                cx.notify();
                                            });
                                        },
                                    ),
                                );
                            }
                            m
                        }),
                )
                .child(
                    Button::new(("sort-dir", i))
                        .outline()
                        .small()
                        .label(if key.descending { "Z → A" } else { "A → Z" })
                        .on_click(move |_, _, cx| {
                            let _ = v2.update(cx, |this, cx| {
                                if let Some(p) = this.sort_panel.as_mut() {
                                    p.levels[i].descending = !p.levels[i].descending;
                                }
                                cx.notify();
                            });
                        }),
                )
                .child(
                    Button::new(("sort-remove", i))
                        .ghost()
                        .small()
                        .label(i18n::t("Remove"))
                        .disabled(i == 0)
                        .on_click(move |_, _, cx| {
                            let _ = v3.update(cx, |this, cx| {
                                if let Some(p) = this.sort_panel.as_mut() {
                                    p.levels.remove(i);
                                }
                                cx.notify();
                            });
                        }),
                )
        });
        let header_view = view.clone();
        let add_view = view.clone();
        Some(
            v_flex()
                .id("sheets-sort-panel")
                .occlude()
                .absolute()
                .top(CONTROL_LG * 2. + SPACE_5)
                .right(SPACE_6)
                .w(px(420.))
                .gap(SPACE_3)
                .p(SPACE_4)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border_strong)
                .shadow_md()
                .text_size(FONT_BODY)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.sort_panel = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .child(SharedString::from(i18n::tf("Sort {}", &[&p.range.name()]))),
                )
                .children(levels)
                .child(
                    h_flex()
                        .gap(SPACE_3)
                        .child(
                            Button::new("sort-add")
                                .ghost()
                                .small()
                                .label(i18n::t("Add level"))
                                .disabled(p.levels.len() >= 8)
                                .on_click(move |_, _, cx| {
                                    let _ = add_view.update(cx, |this, cx| {
                                        if let Some(p) = this.sort_panel.as_mut() {
                                            let next = p.levels.last().map_or(p.range.left, |k| {
                                                (k.col + 1).min(p.range.right)
                                            });
                                            p.levels.push(SortKey {
                                                col: next,
                                                descending: false,
                                            });
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Checkbox::new("sort-header")
                                .label(i18n::t("My data has headers"))
                                .checked(p.header)
                                .on_click(move |on, _, cx| {
                                    let on = *on;
                                    let _ = header_view.update(cx, |this, cx| {
                                        if let Some(p) = this.sort_panel.as_mut() {
                                            p.header = on;
                                        }
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap(SPACE_2)
                        .child(div().flex_1())
                        .child(
                            Button::new("sort-cancel")
                                .ghost()
                                .small()
                                .label(i18n::t("Cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sort_panel = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("sort-ok")
                                .primary()
                                .small()
                                .label(i18n::t("Sort"))
                                .on_click(
                                    cx.listener(|this, _, w, cx| this.apply_sort_panel(w, cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn matches_query(v: &FilterValue, query: &str) -> bool {
    let text = if v.text.is_empty() {
        return t("(Blanks)").to_lowercase().contains(query);
    } else {
        &v.text
    };
    text.to_lowercase().contains(query)
}

fn placeholder(kind: ColumnKind) -> &'static str {
    match kind {
        ColumnKind::Date => "2025-03-01",
        ColumnKind::Number => t("Number"),
        ColumnKind::Text => t("Text (* and ? match any)"),
    }
}

fn header_title(book: &Workbook, sheet: u32, row: u32, col: u32) -> String {
    let h = book.display_value(sheet, row, col);
    if h.is_empty() {
        tf("Column {}", &[&column_name(col)])
    } else {
        h
    }
}

fn sorted_status(n: usize, descending: bool) -> String {
    if descending {
        tf("Sorted {} rows descending", &[&n])
    } else {
        tf("Sorted {} rows ascending", &[&n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn conditions_become_filters_and_back() {
        let f = Cond::Contains
            .filter(ColumnKind::Text, "a*b", "")
            .unwrap()
            .unwrap();
        assert_eq!(
            Cond::from_filter(&f, ColumnKind::Text),
            Some((Cond::Contains, "a*b".into(), String::new()))
        );
        let f = Cond::Between
            .filter(ColumnKind::Number, "10", "1,000")
            .unwrap()
            .unwrap();
        assert_eq!(
            Cond::from_filter(&f, ColumnKind::Number),
            Some((Cond::Between, "10".into(), "1000".into()))
        );
        let f = Cond::Before
            .filter(ColumnKind::Date, "2025-03-01", "")
            .unwrap()
            .unwrap();
        assert_eq!(
            Cond::from_filter(&f, ColumnKind::Date),
            Some((Cond::Before, "2025-03-01".into(), String::new()))
        );
        assert!(Cond::Greater.filter(ColumnKind::Number, "x", "").is_err());
        assert_eq!(Cond::None.filter(ColumnKind::Text, "", "").unwrap(), None);
    }

    #[std::prelude::v1::test]
    fn engine_errors_have_indonesian() {
        for e in ENGINE_ERRORS {
            assert!(kantor_ui::i18n::indonesian(e).is_some(), "{e:?}");
        }
    }
}
