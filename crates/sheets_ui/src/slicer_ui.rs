//! Slicers and timelines over the grid (see `sheets_scene::slicer`), as in
//! Excel:
//!
//! * a slicer button selects that item alone (Ctrl+click or Multi-Select
//!   adds or removes it), Multi-Select toggles, Clear Filter selects them
//!   all; the frame and header still select and move the slicer;
//! * a timeline period selects that period, a drag over periods (or
//!   Shift+click) selects the range (shown as it grows, filtering when the
//!   button comes up), Clear Filter shows all periods, the time level
//!   (YEARS, QUARTERS, MONTHS, DAYS) opens a menu, and the scroll bar pages
//!   through the periods or scrolls with its thumb dragged;
//! * Insert Slicer (PivotTable and Table menus, a cell's right-click menu,
//!   the palette) lists the fields with check boxes, one slicer each;
//!   Insert Timeline (PivotTable menu, right-click, palette) the same with
//!   the pivot table's date fields;
//! * Slicer Settings (right-click a slicer, the palette): caption, header,
//!   sorting, items with no data, style, columns and button height;
//!   Timeline Settings (right-click a timeline, the palette): caption,
//!   header, selection label, scroll bar, time level and style.

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    radio::Radio,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, t_dyn, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::slicer::{LEVELS, slicer_layout, timeline_layout};
use sheets_engine::slicer_edit::{
    SLICER_STYLES, SlicerSettings, SlicerSource, TIMELINE_STYLES, TimelineSettings,
};

use crate::SheetsView;
use crate::dialog::{self, note, row};

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

/// A drag over a timeline's periods: where it started.
#[derive(Clone, Debug)]
pub(crate) struct TimelineDrag {
    pub(crate) key: DrawingKey,
    name: String,
    origin: Point<Pixels>,
    size: (f32, f32),
    /// The first day of the period pressed.
    start: f64,
}

/// A drag of a timeline's scroll bar thumb.
#[derive(Clone, Debug)]
pub(crate) struct TimelineScroll {
    key: DrawingKey,
    name: String,
    /// The pointer's x when pressed.
    press_x: f32,
    /// The first period shown then.
    before: usize,
    /// How far the thumb can travel (px), and the periods that scrolls
    /// through.
    track: f32,
    range: usize,
}

/// The open Insert Timeline dialog.
pub(crate) struct InsertTimelineDialog {
    sheet: u32,
    pivot: String,
    fields: Vec<(String, bool)>,
    error: Option<String>,
}

/// The open Timeline Settings dialog.
pub(crate) struct TimelineSettingsDialog {
    key: DrawingKey,
    settings: TimelineSettings,
    caption: Entity<InputState>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// The open time level menu of a timeline.
pub(crate) struct TimelineMenu {
    key: DrawingKey,
    name: String,
    at: Point<Pixels>,
    level: u32,
}

/// The open Insert Slicer dialog.
pub(crate) struct InsertSlicerDialog {
    source: SlicerSource,
    /// The fields and whether each is checked.
    fields: Vec<(String, bool)>,
    error: Option<String>,
}

/// The open Slicer Settings dialog.
pub(crate) struct SlicerSettingsDialog {
    key: DrawingKey,
    settings: SlicerSettings,
    caption: Entity<InputState>,
    columns: Entity<InputState>,
    height: Entity<InputState>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// Style names as the gallery shows them: `Light 1`, `Dark 6`, `Other 2`.
fn style_label(name: &str) -> String {
    let s = name
        .trim_start_matches("TimeSlicerStyle")
        .trim_start_matches("SlicerStyle");
    let at = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    let family = match &s[..at] {
        "Dark" => t("Dark"),
        "Other" => t("Other"),
        _ => t("Light"),
    };
    format!("{family} {}", &s[at..])
}

/// A time level's name in the menu.
fn level_label(level: usize) -> &'static str {
    match level {
        0 => t("Years"),
        1 => t("Quarters"),
        2 => t("Months"),
        _ => t("Days"),
    }
}

impl SheetsView {
    /// A click at `(x, y)` px inside slicer or timeline `kind` drawn into
    /// `w` × `h` px at `origin` on screen. Returns whether it hit a part
    /// that does something (else the click selects the object, as on any
    /// drawing).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slicer_click(
        &mut self,
        key: DrawingKey,
        kind: &DrawingKind,
        (x, y): (f32, f32),
        (w, h): (f32, f32),
        (ctrl, shift, origin): (bool, bool, Point<Pixels>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Laid out at 100 %, drawn at the sheet's zoom.
        let z = self.sheet_zoom(cx);
        let (x, y, w, h) = (x / z, y / z, w / z, h / z);
        let name = match kind {
            DrawingKind::Slicer { name } => name.clone(),
            DrawingKind::Timeline { name } => {
                return self.timeline_click(key, name, (x, y), (w, h), shift, origin, window, cx);
            }
            _ => return false,
        };
        let Some(view) = self
            .table
            .read(cx)
            .delegate()
            .book
            .slicer_view(key.sheet_id, &name)
        else {
            return false;
        };
        let l = slicer_layout(&view, w, h);
        if l.multi.is_some_and(|r| inside(r, x, y)) {
            let mut layer = self.drawings.borrow_mut();
            if !layer.slicer_multi.remove(&key) {
                layer.slicer_multi.insert(key);
            }
            drop(layer);
            cx.notify();
            return true;
        }
        let current: Vec<bool> = view.items.iter().map(|i| i.selected).collect();
        let wanted: Vec<bool> = if l.clear.is_some_and(|r| inside(r, x, y)) {
            if !view.filtered {
                return true;
            }
            vec![true; current.len()]
        } else if let Some(k) = l.buttons.iter().position(|r| inside(*r, x, y)) {
            let multi = ctrl || self.drawings.borrow().slicer_multi.contains(&key);
            let mut next = current.clone();
            if multi {
                next[k] = !next[k];
                if !next.iter().any(|s| *s) {
                    return true;
                }
            } else {
                next = (0..current.len()).map(|i| i == k).collect();
            }
            next
        } else {
            return false;
        };
        if wanted == current {
            return true;
        }
        if let Some(why) = &view.problem {
            self.status = tf(
                "The slicer {} can't filter: {}",
                &[&view.caption, &t_dyn(why)],
            )
            .into();
            cx.notify();
            return true;
        }
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let r = grid.book.slicer_select(key.sheet_id, &name, &wanted);
            grid.resize();
            table.refresh(cx);
            r
        });
        self.status = match res {
            Ok(()) if wanted.iter().all(|s| *s) => {
                tf("{}: filter cleared", &[&view.caption]).into()
            }
            Ok(()) => {
                let n = wanted.iter().filter(|s| **s).count();
                tf("{}: {} selected", &[&view.caption, &n]).into()
            }
            Err(e) => t_dyn(&format!("{e:#}")).to_owned().into(),
        };
        self.after_pivot_edit(window, cx);
        true
    }

    /// A click on a timeline (see [`Self::slicer_click`]).
    #[allow(clippy::too_many_arguments)]
    fn timeline_click(
        &mut self,
        key: DrawingKey,
        name: &str,
        (x, y): (f32, f32),
        (w, h): (f32, f32),
        shift: bool,
        origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(view) = self
            .table
            .read(cx)
            .delegate()
            .book
            .timeline_view(key.sheet_id, name)
        else {
            return false;
        };
        let l = timeline_layout(&view, w, h);
        let z = self.sheet_zoom(cx);
        if l.clear.is_some_and(|r| inside(r, x, y)) {
            if view.filtered {
                self.timeline_select(key, name, None, window, cx);
            }
            return true;
        }
        if view.level.is_some() && inside(l.level, x, y) {
            self.timeline_menu = Some(TimelineMenu {
                key,
                name: name.to_owned(),
                at: point(
                    origin.x + px((l.level[0] + l.level[2]) * z - 120.0),
                    origin.y + px((l.level[1] + l.level[3]) * z),
                ),
                level: view.level_index,
            });
            cx.notify();
            return true;
        }
        if let Some((bar, thumb)) = l.scroll
            && inside(bar, x, y)
        {
            let page = l.periods.len().max(1) as i32;
            let by = if x < thumb[0] {
                -page
            } else if x > thumb[0] + thumb[2] {
                page
            } else {
                // The thumb: dragged, it scrolls with the pointer.
                let mut layer = self.drawings.borrow_mut();
                layer.selected = Some(key);
                layer.timeline_scroll = Some(TimelineScroll {
                    key,
                    name: name.to_owned(),
                    press_x: f32::from(origin.x) + x * z,
                    before: view.before,
                    track: ((bar[2] - thumb[2]) * z).max(1.0),
                    range: view.last_scroll(l.periods.len()),
                });
                drop(layer);
                cx.notify();
                return true;
            };
            let _ = self.table.update(cx, |table, cx| {
                let r = table
                    .delegate_mut()
                    .book
                    .timeline_scroll(key.sheet_id, name, by);
                cx.notify();
                r
            });
            self.drawings.borrow_mut().selected = Some(key);
            cx.notify();
            return true;
        }
        let Some(k) = l.periods.iter().position(|r| inside(*r, x, y)) else {
            return false;
        };
        let p = &view.periods[k];
        let range = match (shift, self.timeline_anchor(key, name, cx)) {
            (true, Some(a)) => (a.min(p.start), a.max(p.end - 1.0)),
            _ => (p.start, p.end - 1.0),
        };
        self.timeline_select(key, name, Some(range), window, cx);
        self.drawings.borrow_mut().timeline_drag = Some(TimelineDrag {
            key,
            name: name.to_owned(),
            origin,
            size: (w, h),
            start: range.0,
        });
        true
    }

    /// The first day selected on a timeline, for Shift+click.
    fn timeline_anchor(&self, key: DrawingKey, name: &str, cx: &App) -> Option<f64> {
        let view = self
            .table
            .read(cx)
            .delegate()
            .book
            .timeline_view(key.sheet_id, name)?;
        view.periods.iter().find(|p| p.selected).map(|p| p.start)
    }

    /// The pointer moved while a timeline's scroll bar thumb is held: the
    /// periods scroll with it.
    pub(crate) fn timeline_scroll_move(
        &mut self,
        s: TimelineScroll,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let dx = f32::from(at.x) - s.press_x;
        let first = (s.before as f32 + dx / s.track * s.range as f32)
            .round()
            .clamp(0.0, s.range as f32) as usize;
        let changed = self.table.update(cx, |table, cx| {
            let book = &mut table.delegate_mut().book;
            let was = book
                .timeline_view(s.key.sheet_id, &s.name)
                .map(|v| v.before);
            if was == Some(first) {
                return false;
            }
            let _ = book.timeline_scroll_to(s.key.sheet_id, &s.name, first);
            cx.notify();
            true
        });
        if changed {
            cx.notify();
        }
    }

    /// The pointer moved during a drag across a timeline's periods: the
    /// range it would select shows on the timeline.
    pub(crate) fn timeline_drag_move(
        &mut self,
        d: TimelineDrag,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let range = self.timeline_drag_range(&d, at, cx);
        let mut layer = self.drawings.borrow_mut();
        let now = range.map(|r| (d.key, r));
        if layer.timeline_preview != now {
            layer.timeline_preview = now;
            drop(layer);
            cx.notify();
        }
    }

    /// The dates a drag across a timeline's periods from `d` to `at`
    /// selects: the periods from the pressed one to the one under the
    /// pointer.
    fn timeline_drag_range(
        &self,
        d: &TimelineDrag,
        at: Point<Pixels>,
        cx: &App,
    ) -> Option<(f64, f64)> {
        let view = self
            .table
            .read(cx)
            .delegate()
            .book
            .timeline_view(d.key.sheet_id, &d.name)?;
        let l = timeline_layout(&view, d.size.0, d.size.1);
        let x = f32::from(at.x - d.origin.x) / self.sheet_zoom(cx);

        // The period under the pointer, else the nearest end.
        let k = l
            .periods
            .iter()
            .position(|r| x >= r[0] && x <= r[0] + r[2] + 2.0)
            .or_else(|| {
                let first = l.periods.first()?;
                Some(if x < first[0] { 0 } else { l.periods.len() - 1 })
            });
        let p = &view.periods[k?];
        let range = (d.start.min(p.start), d.start.max(p.end - 1.0));
        Some(if p.start < d.start {
            // Dragged back: to the end of the period pressed.
            let pressed = view
                .periods
                .iter()
                .find(|q| q.start <= d.start && d.start < q.end)
                .map_or(range.1, |q| q.end - 1.0);
            (p.start, pressed)
        } else {
            range
        })
    }

    /// The button came up after a press on a timeline period: the periods
    /// from the pressed one to the one under the pointer filter.
    pub(crate) fn timeline_drag_end(
        &mut self,
        d: TimelineDrag,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.timeline_drag_range(&d, at, cx);
        self.drawings.borrow_mut().timeline_preview = None;
        if let Some(range) = range {
            self.timeline_select(d.key, &d.name, Some(range), window, cx);
        }
        cx.notify();
    }

    fn timeline_select(
        &mut self,
        key: DrawingKey,
        name: &str,
        range: Option<(f64, f64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let r = grid.book.timeline_select(key.sheet_id, name, range);
            grid.resize();
            table.refresh(cx);
            r
        });
        let label = self
            .table
            .read(cx)
            .delegate()
            .book
            .timeline_view(key.sheet_id, name)
            .map(|v| (v.caption, v.label.unwrap_or_default()));
        self.status = match (res, label) {
            (Err(e), _) => t_dyn(&format!("{e:#}")).to_owned().into(),
            (Ok(()), Some((caption, _))) if range.is_none() => {
                tf("{}: filter cleared", &[&caption]).into()
            }
            (Ok(()), Some((caption, label))) => format!("{caption}: {label}").into(),
            (Ok(()), None) => SharedString::default(),
        };
        self.after_pivot_edit(window, cx);
    }

    fn set_timeline_level(&mut self, level: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.timeline_menu.take() else {
            return;
        };
        let res = self.table.update(cx, |table, cx| {
            let r = table
                .delegate_mut()
                .book
                .timeline_set_level(m.key.sheet_id, &m.name, level);
            table.refresh(cx);
            r
        });
        if let Err(e) = res {
            self.status = t_dyn(&format!("{e:#}")).to_owned().into();
        }
        self.focus_grid(window, cx);
        cx.notify();
    }

    pub(crate) fn render_timeline_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.timeline_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let mut list = v_flex()
            .id("timeline-levels")
            .occlude()
            .w(px(120.))
            .p(SPACE_1)
            .rounded(RADIUS_SM)
            .bg(t.surface)
            .border_1()
            .border_color(t.border_strong)
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.timeline_menu = None;
                cx.notify();
            }));
        for i in 0..LEVELS.len() {
            let v = view.clone();
            list = list.child(
                Button::new(("timeline-level", i))
                    .small()
                    .w_full()
                    .label(level_label(i))
                    .when_else(i as u32 == m.level, |b| b.primary(), |b| b.ghost())
                    .on_click(move |_, w, cx| {
                        let _ = v.update(cx, |this, cx| this.set_timeline_level(i as u32, w, cx));
                    }),
            );
        }
        Some(kantor_ui::overlay::popover(anchored().position(m.at).child(list)).into_any_element())
    }

    // ------------------------------------------------------- Insert Slicer

    /// What Insert Slicer works on: the pivot table at the active cell,
    /// else its table.
    fn slicer_source_here(&self, cx: &App) -> Option<SlicerSource> {
        let grid = self.table.read(cx).delegate();
        let (r, c) = self.selected;
        if let Some(p) = grid.book.pivot_at(grid.sheet, r, c) {
            return Some(SlicerSource::Pivot {
                sheet: grid.sheet,
                name: p.name,
            });
        }
        grid.book
            .table_at(grid.sheet, r, c)
            .map(|t| SlicerSource::Table(t.name))
    }

    /// Insert Slicer for the pivot table or table at the active cell.
    pub fn insert_slicer_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let Some(source) = self.slicer_source_here(cx) else {
            self.status = t("Select a cell in a PivotTable or a table to insert a slicer.").into();
            cx.notify();
            return;
        };
        let fields = self.table.read(cx).delegate().book.slicer_fields(&source);
        match fields {
            Ok(fields) => {
                self.insert_slicer = Some(InsertSlicerDialog {
                    source,
                    fields: fields.into_iter().map(|f| (f, false)).collect(),
                    error: None,
                });
            }
            Err(e) => self.status = t_dyn(&format!("{e:#}")).to_owned().into(),
        }
        cx.notify();
    }

    fn close_insert_slicer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.insert_slicer.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn apply_insert_slicer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.insert_slicer.as_ref() else {
            return;
        };
        let source = d.source.clone();
        let fields: Vec<String> = d
            .fields
            .iter()
            .filter(|(_, on)| *on)
            .map(|(f, _)| f.clone())
            .collect();
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let (sheet, at) = grid
                .book
                .slicer_place(&source)
                .unwrap_or((grid.sheet, self.selected));
            let r = grid.book.insert_slicers(sheet, &source, &fields, at);
            grid.resize();
            table.refresh(cx);
            r
        });
        match res {
            Ok(keys) => {
                self.insert_slicer = None;
                self.drawings.borrow_mut().selected = keys.first().copied();
                self.status = tf("{} slicer(s) inserted", &[&keys.len()]).into();
                self.after_pivot_edit(window, cx);
                self.focus_grid(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.insert_slicer.as_mut() {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_insert_slicer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.insert_slicer.as_ref()?;
        let view = cx.entity().downgrade();
        let tk = tokens(cx);
        let mut list = v_flex().gap(SPACE_2);
        for (i, (field, on)) in d.fields.iter().enumerate() {
            let v = view.clone();
            list = list.child(
                Checkbox::new(("insert-slicer-field", i))
                    .label(SharedString::from(field.clone()))
                    .checked(*on)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.insert_slicer.as_mut()
                                && let Some(f) = d.fields.get_mut(i)
                            {
                                f.1 = on;
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let any = d.fields.iter().any(|(_, on)| *on);
        let card = dialog::card("insert-slicer", t("Insert Slicers"), px(320.), cx)
            .key_context("KantorInsertSlicer")
            .on_action(
                cx.listener(|this, _: &input::Escape, w, cx| this.close_insert_slicer(w, cx)),
            )
            .child(
                div()
                    .id("insert-slicer-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .p(SPACE_3)
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(tk.border)
                    .bg(tk.bg)
                    .child(list),
            )
            .children(
                d.error
                    .clone()
                    .map(|e| note(t_dyn(&e).to_owned(), true, cx)),
            )
            .child(dialog::buttons(
                "insert-slicer-buttons",
                t("OK"),
                any,
                &view,
                Self::apply_insert_slicer,
                Self::close_insert_slicer,
            ));
        Some(dialog::place(card, self, cx).into_any_element())
    }

    // ------------------------------------------------------ Slicer Settings

    /// Slicer Settings for the selected slicer.
    pub fn slicer_settings_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let key = self.drawings.borrow().selected;
        let book = &self.table.read(cx).delegate().book;
        let found = key.and_then(|k| {
            let name = book.slicer_of(k)?;
            Some((k, book.slicer_settings(k.sheet_id, &name)?))
        });
        let Some((key, settings)) = found else {
            self.status = t("Select a slicer to change its settings.").into();
            cx.notify();
            return;
        };
        let input = |text: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(text, window, cx);
                s
            })
        };
        let caption = input(settings.caption.clone(), window, cx);
        let columns = input(settings.columns.to_string(), window, cx);
        let height = input(format!("{}", settings.button_height.round()), window, cx);
        let subs = [&caption, &columns, &height]
            .into_iter()
            .map(|e| {
                cx.subscribe_in(e, window, |this, _, e: &InputEvent, w, cx| {
                    if let InputEvent::PressEnter { .. } = e {
                        this.apply_slicer_settings(w, cx);
                    }
                })
            })
            .collect();
        self.slicer_settings = Some(SlicerSettingsDialog {
            key,
            settings,
            caption,
            columns,
            height,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn close_slicer_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slicer_settings.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn apply_slicer_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.slicer_settings.as_ref() else {
            return;
        };
        let mut s = d.settings.clone();
        s.caption = d.caption.read(cx).value().to_string();
        let columns = d.columns.read(cx).value().trim().parse::<u32>().ok();
        let height = dialog::parse_number(&d.height.read(cx).value());
        let (Some(columns), Some(height)) = (columns, height) else {
            if let Some(d) = self.slicer_settings.as_mut() {
                d.error =
                    Some(t("Type a whole number of columns and a button height in px.").into());
            }
            cx.notify();
            return;
        };
        s.columns = columns;
        s.button_height = height as f32;
        let key = d.key;
        let name = s.name.clone();
        let res = self.table.update(cx, |table, cx| {
            let r = table
                .delegate_mut()
                .book
                .set_slicer_settings(key.sheet_id, &name, &s);
            table.refresh(cx);
            r
        });
        match res {
            Ok(()) => {
                self.slicer_settings = None;
                self.status = tf("{}: settings changed", &[&s.caption]).into();
                self.focus_grid(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.slicer_settings.as_mut() {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_slicer_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.slicer_settings.as_ref()?;
        let view = cx.entity().downgrade();
        let s = &d.settings;
        let set = |f: fn(&mut SlicerSettings, bool)| {
            let v = view.clone();
            move |on: &bool, _: &mut Window, cx: &mut App| {
                let on = *on;
                let _ = v.update(cx, |this, cx| {
                    if let Some(d) = this.slicer_settings.as_mut() {
                        f(&mut d.settings, on);
                    }
                    cx.notify();
                });
            }
        };
        let sort = |id: &'static str, label: &'static str, descending: bool| {
            let v = view.clone();
            Radio::new(id)
                .label(label)
                .checked(s.descending == descending)
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(d) = this.slicer_settings.as_mut() {
                            d.settings.descending = descending;
                        }
                        cx.notify();
                    });
                })
        };
        let current = s.style.clone();
        let sv = view.clone();
        let style = Button::new("slicer-style")
            .outline()
            .small()
            .label(style_label(&current))
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for name in SLICER_STYLES {
                    let v = sv.clone();
                    m = m.item(
                        PopupMenuItem::new(style_label(name))
                            .checked(name == current)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(d) = this.slicer_settings.as_mut() {
                                        d.settings.style = name.to_owned();
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        let card = dialog::card("slicer-settings", t("Slicer Settings"), px(440.), cx)
            .key_context("KantorSlicerSettings")
            .on_action(
                cx.listener(|this, _: &input::Escape, w, cx| this.close_slicer_settings(w, cx)),
            )
            .child(note(
                tf(
                    "Source Name: {} · Name to use in formulas: {}",
                    &[&s.source_name, &s.cache_name],
                ),
                false,
                cx,
            ))
            .child(row(t("Caption:"), Input::new(&d.caption).small(), cx))
            .child(
                Checkbox::new("slicer-header")
                    .label(t("Display header"))
                    .checked(s.show_caption)
                    .on_click(set(|s, on| s.show_caption = on)),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_6)
                    .child(
                        v_flex()
                            .gap(SPACE_3)
                            .child(kantor_ui::components::section_label(
                                t("Item Sorting and Filtering"),
                                cx,
                            ))
                            .child(sort("slicer-asc", t("Ascending (A to Z)"), false))
                            .child(sort("slicer-desc", t("Descending (Z to A)"), true)),
                    )
                    .child(
                        v_flex()
                            .gap(SPACE_3)
                            .child(
                                Checkbox::new("slicer-hide")
                                    .label(t("Hide items with no data"))
                                    .checked(s.hide_no_data)
                                    .on_click(set(|s, on| s.hide_no_data = on)),
                            )
                            .child(
                                Checkbox::new("slicer-mark")
                                    .label(t("Visually indicate items with no data"))
                                    .checked(s.mark_no_data)
                                    .on_click(set(|s, on| s.mark_no_data = on)),
                            )
                            .child(
                                Checkbox::new("slicer-last")
                                    .label(t("Show items with no data last"))
                                    .checked(s.no_data_last)
                                    .on_click(set(|s, on| s.no_data_last = on)),
                            ),
                    ),
            )
            .child(row(t("Style:"), style, cx))
            .child(row(t("Columns:"), Input::new(&d.columns).small(), cx))
            .child(row(
                t("Button height (px):"),
                Input::new(&d.height).small(),
                cx,
            ))
            .children(
                d.error
                    .clone()
                    .map(|e| note(t_dyn(&e).to_owned(), true, cx)),
            )
            .child(dialog::buttons(
                "slicer-settings-buttons",
                t("OK"),
                true,
                &view,
                Self::apply_slicer_settings,
                Self::close_slicer_settings,
            ));
        Some(dialog::place(card, self, cx).into_any_element())
    }

    // ----------------------------------------------------- Insert Timeline

    /// Insert Timeline for the pivot table at the active cell.
    pub fn insert_timeline_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let grid = self.table.read(cx).delegate();
        let (r, c) = self.selected;
        let Some(p) = grid.book.pivot_at(grid.sheet, r, c) else {
            self.status = t("Select a cell in a PivotTable to insert a timeline.").into();
            cx.notify();
            return;
        };
        let sheet = grid.sheet;
        match grid.book.timeline_fields(sheet, &p.name) {
            Ok(fields) => {
                let one = fields.len() == 1;
                self.insert_timeline = Some(InsertTimelineDialog {
                    sheet,
                    pivot: p.name,
                    fields: fields.into_iter().map(|f| (f, one)).collect(),
                    error: None,
                });
            }
            Err(e) => self.status = t_dyn(&format!("{e:#}")).to_owned().into(),
        }
        cx.notify();
    }

    fn close_insert_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.insert_timeline.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn apply_insert_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.insert_timeline.as_ref() else {
            return;
        };
        let (sheet, pivot) = (d.sheet, d.pivot.clone());
        let fields: Vec<String> = d
            .fields
            .iter()
            .filter(|(_, on)| *on)
            .map(|(f, _)| f.clone())
            .collect();
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let (sheet, at) = grid
                .book
                .timeline_place(sheet, &pivot)
                .unwrap_or((sheet, self.selected));
            let r = grid.book.insert_timelines(sheet, &pivot, &fields, at);
            grid.resize();
            table.refresh(cx);
            r
        });
        match res {
            Ok(keys) => {
                self.insert_timeline = None;
                self.drawings.borrow_mut().selected = keys.first().copied();
                self.status = tf("{} timeline(s) inserted", &[&keys.len()]).into();
                self.after_pivot_edit(window, cx);
                self.focus_grid(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.insert_timeline.as_mut() {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_insert_timeline(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.insert_timeline.as_ref()?;
        let view = cx.entity().downgrade();
        let tk = tokens(cx);
        let mut list = v_flex().gap(SPACE_2);
        for (i, (field, on)) in d.fields.iter().enumerate() {
            let v = view.clone();
            list = list.child(
                Checkbox::new(("insert-timeline-field", i))
                    .label(SharedString::from(field.clone()))
                    .checked(*on)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.insert_timeline.as_mut()
                                && let Some(f) = d.fields.get_mut(i)
                            {
                                f.1 = on;
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let any = d.fields.iter().any(|(_, on)| *on);
        let card = dialog::card("insert-timeline", t("Insert Timelines"), px(320.), cx)
            .key_context("KantorInsertTimeline")
            .on_action(
                cx.listener(|this, _: &input::Escape, w, cx| this.close_insert_timeline(w, cx)),
            )
            .child(
                div()
                    .id("insert-timeline-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .p(SPACE_3)
                    .rounded(RADIUS_SM)
                    .border_1()
                    .border_color(tk.border)
                    .bg(tk.bg)
                    .child(list),
            )
            .children(
                d.error
                    .clone()
                    .map(|e| note(t_dyn(&e).to_owned(), true, cx)),
            )
            .child(dialog::buttons(
                "insert-timeline-buttons",
                t("OK"),
                any,
                &view,
                Self::apply_insert_timeline,
                Self::close_insert_timeline,
            ));
        Some(dialog::place(card, self, cx).into_any_element())
    }

    // --------------------------------------------------- Timeline Settings

    /// Timeline Settings for the selected timeline.
    pub fn timeline_settings_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let key = self.drawings.borrow().selected;
        let book = &self.table.read(cx).delegate().book;
        let found = key.and_then(|k| {
            let name = book.timeline_of(k)?;
            Some((k, book.timeline_settings(k.sheet_id, &name)?))
        });
        let Some((key, settings)) = found else {
            self.status = t("Select a timeline to change its settings.").into();
            cx.notify();
            return;
        };
        let caption = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(settings.caption.clone(), window, cx);
            s
        });
        let subs = vec![
            cx.subscribe_in(&caption, window, |this, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.apply_timeline_settings(w, cx);
                }
            }),
        ];
        self.timeline_settings = Some(TimelineSettingsDialog {
            key,
            settings,
            caption,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn close_timeline_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.timeline_settings.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn apply_timeline_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.timeline_settings.as_ref() else {
            return;
        };
        let mut s = d.settings.clone();
        s.caption = d.caption.read(cx).value().to_string();
        let key = d.key;
        let name = s.name.clone();
        let res = self.table.update(cx, |table, cx| {
            let r = table
                .delegate_mut()
                .book
                .set_timeline_settings(key.sheet_id, &name, &s);
            table.refresh(cx);
            r
        });
        match res {
            Ok(()) => {
                self.timeline_settings = None;
                self.status = tf("{}: settings changed", &[&s.caption]).into();
                self.focus_grid(window, cx);
            }
            Err(e) => {
                if let Some(d) = self.timeline_settings.as_mut() {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_timeline_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.timeline_settings.as_ref()?;
        let view = cx.entity().downgrade();
        let s = &d.settings;
        let set = |f: fn(&mut TimelineSettings, bool)| {
            let v = view.clone();
            move |on: &bool, _: &mut Window, cx: &mut App| {
                let on = *on;
                let _ = v.update(cx, |this, cx| {
                    if let Some(d) = this.timeline_settings.as_mut() {
                        f(&mut d.settings, on);
                    }
                    cx.notify();
                });
            }
        };
        let current = s.style.clone();
        let sv = view.clone();
        let style = Button::new("timeline-style")
            .outline()
            .small()
            .label(style_label(&current))
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for name in TIMELINE_STYLES {
                    let v = sv.clone();
                    m = m.item(
                        PopupMenuItem::new(style_label(name))
                            .checked(name == current)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(d) = this.timeline_settings.as_mut() {
                                        d.settings.style = name.to_owned();
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        let card = dialog::card("timeline-settings", t("Timeline Settings"), px(400.), cx)
            .key_context("KantorTimelineSettings")
            .on_action(
                cx.listener(|this, _: &input::Escape, w, cx| this.close_timeline_settings(w, cx)),
            )
            .child(note(
                tf(
                    "Source Name: {} · Name to use in formulas: {}",
                    &[&s.source_name, &s.cache_name],
                ),
                false,
                cx,
            ))
            .child(row(t("Caption:"), Input::new(&d.caption).small(), cx))
            .child(kantor_ui::components::section_label(t("Show"), cx))
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_6)
                    .child(
                        v_flex()
                            .gap(SPACE_3)
                            .child(
                                Checkbox::new("timeline-header")
                                    .label(t("Header"))
                                    .checked(s.show_header)
                                    .on_click(set(|s, on| s.show_header = on)),
                            )
                            .child(
                                Checkbox::new("timeline-label")
                                    .label(t("Selection Label"))
                                    .checked(s.show_selection_label)
                                    .on_click(set(|s, on| s.show_selection_label = on)),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(SPACE_3)
                            .child(
                                Checkbox::new("timeline-scrollbar")
                                    .label(t("Scrollbar"))
                                    .checked(s.show_scrollbar)
                                    .on_click(set(|s, on| s.show_scrollbar = on)),
                            )
                            .child(
                                Checkbox::new("timeline-level")
                                    .label(t("Time Level"))
                                    .checked(s.show_time_level)
                                    .on_click(set(|s, on| s.show_time_level = on)),
                            ),
                    ),
            )
            .child(row(t("Style:"), style, cx))
            .children(
                d.error
                    .clone()
                    .map(|e| note(t_dyn(&e).to_owned(), true, cx)),
            )
            .child(dialog::buttons(
                "timeline-settings-buttons",
                t("OK"),
                true,
                &view,
                Self::apply_timeline_settings,
                Self::close_timeline_settings,
            ));
        Some(dialog::place(card, self, cx).into_any_element())
    }
}
