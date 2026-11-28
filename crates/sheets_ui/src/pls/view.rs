//! The PLS-SEM model tab: the indicator list (the data's columns) on the
//! left, the path diagram in the middle, the selection and the settings on
//! the right, the results below. See the module docs of [`super`].

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pls::{Algorithm, Measurement, Missing, Model, Options, Progress, Results, Weighting};
use kantor_ui::i18n::{self, t, tf};
use kantor_ui::{metrics::*, tokens};

use super::diagram::{self, Diagram, EditError, Hit, IND_H, IND_W, RX, RY, Side};
use super::labels::{self, Fmt, ResultTable, Section};
use super::{PlsHooks, data};
use crate::SheetsView;

/// What the model tab asks of the Sheets view.
#[derive(Clone, Debug, PartialEq)]
pub enum PlsEvent {
    /// The model changed: store it in the workbook.
    Changed,
    /// Insert these tables as a new worksheet.
    InsertTables(Vec<ResultTable>),
    /// Hand the results to Paparan AI's chat (prompt, files).
    Explain(String, Vec<PathBuf>),
}

/// An estimation running on another thread.
struct Running {
    progress: Arc<Progress>,
    bootstrap: bool,
    _task: Task<()>,
}

/// A mouse drag on the canvas.
#[derive(Clone, Debug)]
enum CanvasDrag {
    /// Moving a construct (`grab`: pointer minus centre; `moved` once it
    /// did, when its undo step was recorded).
    Move {
        name: String,
        grab: (f64, f64),
        moved: bool,
    },
    /// Moving an indicator's box (`grab`: pointer minus the box centre).
    MoveIndicator {
        name: String,
        grab: (f64, f64),
        moved: bool,
    },
    /// Drawing a path from `from` to the pointer.
    Connect { from: String, to: (f64, f64) },
    /// Scrolling the canvas.
    Pan {
        start: (f64, f64),
        scroll: (f64, f64),
    },
}

/// Indicators dragged from the list onto the canvas.
#[derive(Clone)]
struct IndicatorDrag {
    names: Vec<String>,
}

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

/// The diagram's zoom range and the step of the zoom buttons.
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 3.0;
const ZOOM_STEP: f64 = 1.2;
/// Room kept around the diagram by Fit to view (the loadings' labels
/// stick out of the boxes' rectangle).
const FIT_PAD: f64 = 28.0;
/// The results pane's height: default and smallest; the canvas keeps at
/// least `MIN_CANVAS`.
const RESULTS_H: f32 = 300.0;
const MIN_RESULTS_H: f32 = 120.0;
const MIN_CANVAS: f32 = 160.0;

/// One shape the canvas paints, in canvas coordinates.
enum Prim {
    Line {
        a: (f32, f32),
        b: (f32, f32),
        width: f32,
        color: Hsla,
        dashed: bool,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        fill: Option<Hsla>,
        stroke: Option<(Hsla, f32)>,
    },
    Rect {
        r: diagram::Rect,
        fill: Hsla,
        border: Hsla,
        width: f32,
    },
}

pub struct PlsView {
    sheets: WeakEntity<SheetsView>,
    pub(crate) diagram: Diagram,
    /// The data's column names (the indicators to pick from).
    columns: Vec<String>,
    /// Cases in the data range (header excluded), when known.
    cases: Option<usize>,
    checked: BTreeSet<String>,
    selection: Option<Hit>,
    drag: Option<CanvasDrag>,
    /// The canvas's offset (pixels) and scale: a diagram point `p` is drawn
    /// at `p * zoom + scroll`.
    scroll: (f64, f64),
    zoom: f64,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Fit to view waits for the canvas's size (the first paint, or the
    /// results pane appearing).
    fit_pending: Rc<Cell<bool>>,
    /// The canvas size the last fit was for, until a paint confirms it.
    fitted: Rc<Cell<Option<Size<Pixels>>>>,
    /// The results pane's height, and its drag (pointer y and height at
    /// the start).
    results_h: f32,
    resizing: Option<(f32, f32)>,
    focus: FocusHandle,
    name_input: Entity<InputState>,
    rename_input: Entity<InputState>,
    samples_input: Entity<InputState>,
    seed_input: Entity<InputState>,
    missing_input: Entity<InputState>,
    running: Option<Running>,
    results: Option<Arc<Results>>,
    /// The constructs and paths the results are for.
    results_model: Option<Model>,
    /// The data the results were estimated on, as CSV (for Explain).
    results_csv: Option<String>,
    section: Section,
    show_results: bool,
    status: SharedString,
    status_error: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PlsEvent> for PlsView {}

impl PlsView {
    pub fn new(
        sheets: WeakEntity<SheetsView>,
        diagram: Diagram,
        columns: Vec<String>,
        cases: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            let e = cx.new(|cx| InputState::new(window, cx));
            e.update(cx, |i, cx| i.set_value(value, window, cx));
            e
        };
        let opts = diagram.options();
        let boot = opts.bootstrap.clone().unwrap_or_default();
        let name_input = input(diagram.name().to_owned(), window, cx);
        let rename_input = input(String::new(), window, cx);
        let samples_input = input(boot.samples.to_string(), window, cx);
        let seed_input = input(boot.seed.to_string(), window, cx);
        let missing_input = input(
            opts.missing_value
                .map(|v| v.to_string())
                .unwrap_or_default(),
            window,
            cx,
        );
        let mut subs = Vec::new();
        let on_commit = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            move |this: &mut Self,
                  _: &Entity<InputState>,
                  e: &InputEvent,
                  w: &mut Window,
                  cx: &mut Context<Self>| {
                if let InputEvent::PressEnter { .. } | InputEvent::Blur = e {
                    f(this, w, cx);
                }
            }
        };
        subs.push(cx.subscribe_in(&name_input, window, on_commit(Self::apply_name)));
        subs.push(cx.subscribe_in(&rename_input, window, on_commit(Self::apply_rename)));
        for i in [&samples_input, &seed_input, &missing_input] {
            subs.push(cx.subscribe_in(i, window, on_commit(Self::apply_settings_inputs)));
        }
        Self {
            sheets,
            diagram,
            columns,
            cases,
            checked: BTreeSet::new(),
            selection: None,
            drag: None,
            scroll: (0.0, 0.0),
            zoom: 1.0,
            bounds: Rc::new(Cell::new(Bounds::default())),
            fit_pending: Rc::new(Cell::new(true)),
            fitted: Rc::new(Cell::new(None)),
            results_h: kantor_core::settings::Settings::load()
                .sheets
                .pls_results_height
                .map_or(RESULTS_H, |h| (h as f32).max(MIN_RESULTS_H)),
            resizing: None,
            focus: cx.focus_handle(),
            name_input,
            rename_input,
            samples_input,
            seed_input,
            missing_input,
            running: None,
            results: None,
            results_model: None,
            results_csv: None,
            section: Section::Measurement,
            show_results: true,
            status: SharedString::default(),
            status_error: false,
            _subs: subs,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn name(&self) -> String {
        self.diagram.name().to_owned()
    }

    fn say(&mut self, text: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.status_error = error;
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(PlsEvent::Changed);
        cx.notify();
    }

    fn edit_error(&mut self, e: EditError, cx: &mut Context<Self>) {
        let text = match e {
            EditError::EmptyName => t("A construct needs a name").to_owned(),
            EditError::NameTaken(n) => tf("There is already a construct called {}", &[&n]),
            EditError::NoConstruct(n) => tf("There is no construct {}", &[&n]),
            EditError::NoIndicators => t("Check or drag indicators first").to_owned(),
            EditError::SelfPath => t("A construct can't point to itself").to_owned(),
            EditError::PathExists => t("That path is already there").to_owned(),
            EditError::Cycle => {
                t("That path would close a loop: PLS-SEM models must be recursive").to_owned()
            }
        };
        self.say(text, true, cx);
    }

    /// The model's constructs and paths (not its layout or settings).
    fn structure(m: &Model) -> (Vec<kantor_pls::Construct>, Vec<kantor_pls::Path>) {
        (m.constructs.clone(), m.paths.clone())
    }

    /// The results, when they are for the model as it is now.
    fn fresh_results(&self) -> Option<&Arc<Results>> {
        let r = self.results.as_ref()?;
        let m = self.results_model.as_ref()?;
        (Self::structure(m) == Self::structure(&self.diagram.model)).then_some(r)
    }

    fn fmt(&self) -> Fmt {
        let alpha = self.diagram.options().bootstrap.map_or(0.05, |b| b.alpha);
        Fmt {
            comma: i18n::lang() == i18n::Lang::Id,
            one_tailed: self.diagram.one_tailed(),
            alpha,
        }
    }

    // ------------------------------------------------------------ edits

    fn select(&mut self, hit: Option<Hit>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Hit::Construct(name)) = &hit {
            let name = name.clone();
            self.rename_input
                .update(cx, |i, cx| i.set_value(name, window, cx));
        }
        self.selection = hit;
        cx.notify();
    }

    fn create_from_checked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .columns
            .iter()
            .filter(|c| self.checked.contains(*c))
            .cloned()
            .collect();
        match self.diagram.create_construct(&names, None) {
            Ok(name) => {
                self.checked.clear();
                self.select(Some(Hit::Construct(name)), window, cx);
                self.say("", false, cx);
                self.changed(cx);
            }
            Err(e) => self.edit_error(e, cx),
        }
    }

    fn add_checked_to(&mut self, name: &str, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .columns
            .iter()
            .filter(|c| self.checked.contains(*c))
            .cloned()
            .collect();
        match self.diagram.add_indicators(name, &names) {
            Ok(()) => {
                self.checked.clear();
                self.changed(cx);
            }
            Err(e) => self.edit_error(e, cx),
        }
    }

    fn drop_indicators(&mut self, d: &IndicatorDrag, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.to_diagram(window.mouse_position());
        let res = match self.diagram.hit(at) {
            Some(Hit::Construct(name)) => {
                self.diagram.add_indicators(&name, &d.names).map(|_| name)
            }
            _ => self.diagram.create_construct(&d.names, Some(at)),
        };
        match res {
            Ok(name) => {
                self.checked.clear();
                self.select(Some(Hit::Construct(name)), window, cx);
                self.changed(cx);
            }
            Err(e) => self.edit_error(e, cx),
        }
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let done = match self.selection.take() {
            Some(Hit::Construct(n)) => self.diagram.delete_construct(&n),
            Some(Hit::Path(a, b)) => self.diagram.remove_path(&a, &b),
            Some(Hit::Indicator(i)) => self.diagram.remove_indicator(&i),
            None => false,
        };
        if done {
            self.changed(cx);
        }
        cx.notify();
    }

    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diagram.undo() {
            self.after_history(window, cx);
        }
    }

    fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diagram.redo() {
            self.after_history(window, cx);
        }
    }

    fn after_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The selection may be gone.
        let keep = match &self.selection {
            Some(Hit::Construct(n)) => self.diagram.construct(n).is_some(),
            Some(Hit::Path(a, b)) => self.diagram.has_path(a, b),
            Some(Hit::Indicator(i)) => self.diagram.construct_of(i).is_some(),
            None => true,
        };
        if !keep {
            self.select(None, window, cx);
        }
        self.changed(cx);
    }

    fn set_measurement(&mut self, name: &str, m: Measurement, cx: &mut Context<Self>) {
        if self.diagram.set_measurement(name, m) {
            self.changed(cx);
        }
    }

    fn auto_layout(&mut self, cx: &mut Context<Self>) {
        if self.diagram.auto_layout() {
            self.changed(cx);
        }
        self.request_fit(cx);
    }

    fn set_side(&mut self, name: &str, side: Option<Side>, cx: &mut Context<Self>) {
        if self.diagram.set_side(name, side) {
            self.changed(cx);
        }
    }

    fn reset_indicators(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.diagram.reset_indicators(name) {
            self.changed(cx);
        }
    }

    // ---------------------------------------------------- zoom and fit

    /// Fit the diagram to the canvas once its size is known.
    fn request_fit(&mut self, cx: &mut Context<Self>) {
        self.fit_pending.set(true);
        cx.notify();
    }

    /// Scale and centre the diagram in the canvas (at most 100 %).
    fn fit_now(&mut self, size: Size<Pixels>) {
        let (w, h) = (
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
        );
        let Some(b) = self.diagram.bounds() else {
            self.zoom = 1.0;
            self.scroll = (0.0, 0.0);
            return;
        };
        let z = ((w - 2.0 * FIT_PAD) / b.w.max(1.0))
            .min((h - 2.0 * FIT_PAD) / b.h.max(1.0))
            .clamp(MIN_ZOOM, 1.0);
        self.zoom = z;
        self.scroll = ((w - b.w * z) / 2.0 - b.x * z, (h - b.h * z) / 2.0 - b.y * z);
    }

    /// Zoom by `factor` keeping the diagram point under `anchor` (a window
    /// point; the canvas's centre when `None`) where it is.
    fn zoom_by(&mut self, factor: f64, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let b = self.bounds.get();
        let anchor = anchor.unwrap_or_else(|| b.center());
        let before = self.to_diagram(anchor);
        let z = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - self.zoom).abs() < 1e-9 {
            return;
        }
        self.zoom = z;
        let (ax, ay) = (
            f64::from(f32::from(anchor.x - b.origin.x)),
            f64::from(f32::from(anchor.y - b.origin.y)),
        );
        self.scroll = (ax - before.0 * z, ay - before.1 * z);
        cx.notify();
    }

    fn zoom_reset(&mut self, cx: &mut Context<Self>) {
        let z = 1.0 / self.zoom;
        self.zoom_by(z, None, cx);
    }

    fn apply_name(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_owned();
        if name.is_empty() || name == self.diagram.name() {
            return;
        }
        self.diagram.set_name(&name);
        self.changed(cx);
    }

    fn apply_rename(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(Hit::Construct(old)) = self.selection.clone() else {
            return;
        };
        let new = self.rename_input.read(cx).value().trim().to_owned();
        if new == old {
            return;
        }
        match self.diagram.rename(&old, &new) {
            Ok(()) => {
                self.selection = Some(Hit::Construct(new));
                self.changed(cx);
            }
            Err(e) => self.edit_error(e, cx),
        }
    }

    fn update_options(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Options)) {
        let mut o = self.diagram.options();
        let before = o.clone();
        f(&mut o);
        if o != before {
            self.diagram.set_options(o);
            self.changed(cx);
        }
    }

    fn apply_settings_inputs(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.read_settings(cx);
    }

    /// Take the typed settings; false (and a message) when one is wrong.
    fn read_settings(&mut self, cx: &mut Context<Self>) -> bool {
        let samples = self
            .samples_input
            .read(cx)
            .value()
            .trim()
            .replace(['.', ','], "");
        let seed = self.seed_input.read(cx).value().trim().to_owned();
        let missing = self.missing_input.read(cx).value().trim().replace(',', ".");
        let samples = match samples.parse::<usize>() {
            Ok(n) if (10..=100_000).contains(&n) => n,
            _ => {
                self.say(
                    t("Bootstrap subsamples must be a whole number from 10 to 100000"),
                    true,
                    cx,
                );
                return false;
            }
        };
        let Ok(seed) = seed.parse::<u64>() else {
            self.say(t("The seed must be a whole number"), true, cx);
            return false;
        };
        let missing = if missing.is_empty() {
            None
        } else {
            match missing.parse::<f64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    self.say(
                        t("The missing value code must be a number, such as -99"),
                        true,
                        cx,
                    );
                    return false;
                }
            }
        };
        self.update_options(cx, |o| {
            let b = o.bootstrap.get_or_insert_with(Default::default);
            b.samples = samples;
            b.seed = seed;
            o.missing_value = missing;
        });
        true
    }

    // ------------------------------------------------------- the canvas

    /// A window point in diagram coordinates.
    fn to_diagram(&self, p: Point<Pixels>) -> (f64, f64) {
        let b = self.bounds.get();
        (
            (f64::from(f32::from(p.x - b.origin.x)) - self.scroll.0) / self.zoom,
            (f64::from(f32::from(p.y - b.origin.y)) - self.scroll.1) / self.zoom,
        )
    }

    /// The selected construct's handle for drawing paths (right of its
    /// ellipse).
    fn handle_of(&self, name: &str) -> (f64, f64) {
        let (x, y) = self.diagram.position(name);
        (x + RX, y)
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let p = self.to_diagram(ev.position);
        // The selected construct's handle starts a path.
        if let Some(Hit::Construct(sel)) = &self.selection {
            let h = self.handle_of(sel);
            if (p.0 - h.0).hypot(p.1 - h.1) * self.zoom <= 9.0 {
                self.drag = Some(CanvasDrag::Connect {
                    from: sel.clone(),
                    to: p,
                });
                cx.notify();
                return;
            }
        }
        let hit = self.diagram.hit(p);
        match &hit {
            Some(Hit::Construct(name)) if ev.modifiers.shift => {
                self.drag = Some(CanvasDrag::Connect {
                    from: name.clone(),
                    to: p,
                });
            }
            Some(Hit::Construct(name)) => {
                if ev.click_count >= 2 {
                    let name = name.clone();
                    self.select(Some(Hit::Construct(name)), window, cx);
                    self.rename_input.update(cx, |i, cx| i.focus(window, cx));
                    return;
                }
                let c = self.diagram.position(name);
                self.drag = Some(CanvasDrag::Move {
                    name: name.clone(),
                    grab: (p.0 - c.0, p.1 - c.1),
                    moved: false,
                });
            }
            Some(Hit::Indicator(name)) => {
                let center = self.diagram.construct_of(name).and_then(|c| {
                    let c = self.diagram.construct(c)?;
                    self.diagram
                        .indicator_boxes(c)
                        .into_iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, r)| r.center())
                });
                if let Some(c) = center {
                    self.drag = Some(CanvasDrag::MoveIndicator {
                        name: name.clone(),
                        grab: (p.0 - c.0, p.1 - c.1),
                        moved: false,
                    });
                }
            }
            None => {
                self.drag = Some(CanvasDrag::Pan {
                    start: (
                        f64::from(f32::from(ev.position.x)),
                        f64::from(f32::from(ev.position.y)),
                    ),
                    scroll: self.scroll,
                });
            }
            Some(Hit::Path(..)) => {}
        }
        self.select(hit, window, cx);
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let p = self.to_diagram(ev.position);
        match &mut self.drag {
            Some(CanvasDrag::Move { name, grab, moved }) => {
                if !*moved {
                    self.diagram.checkpoint();
                    *moved = true;
                }
                let at = (p.0 - grab.0, p.1 - grab.1);
                let name = name.clone();
                self.diagram.move_construct(&name, at);
                cx.notify();
            }
            Some(CanvasDrag::MoveIndicator { name, grab, moved }) => {
                if !*moved {
                    self.diagram.checkpoint();
                    *moved = true;
                }
                let at = (p.0 - grab.0, p.1 - grab.1);
                let name = name.clone();
                self.diagram.move_indicator(&name, at);
                cx.notify();
            }
            Some(CanvasDrag::Connect { to, .. }) => {
                *to = p;
                cx.notify();
            }
            Some(CanvasDrag::Pan { start, scroll }) => {
                let (x, y) = (
                    f64::from(f32::from(ev.position.x)),
                    f64::from(f32::from(ev.position.y)),
                );
                self.scroll = (scroll.0 + x - start.0, scroll.1 + y - start.1);
                cx.notify();
            }
            None => {}
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        match drag {
            CanvasDrag::Move { moved: true, .. }
            | CanvasDrag::MoveIndicator { moved: true, .. } => self.changed(cx),
            CanvasDrag::Connect { from, .. } => {
                let p = self.to_diagram(ev.position);
                if let Some(Hit::Construct(to)) = self.diagram.hit(p)
                    && to != from
                {
                    match self.diagram.add_path(&from, &to) {
                        Ok(()) => {
                            self.select(Some(Hit::Path(from, to)), window, cx);
                            self.changed(cx);
                        }
                        Err(e) => self.edit_error(e, cx),
                    }
                }
                cx.notify();
            }
            _ => cx.notify(),
        }
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let cmd = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "delete" | "backspace" if !cmd => self.delete_selection(cx),
            "escape" => {
                self.drag = None;
                self.select(None, window, cx);
            }
            "=" | "+" if cmd => self.zoom_by(ZOOM_STEP, None, cx),
            "-" if cmd => self.zoom_by(1.0 / ZOOM_STEP, None, cx),
            "0" if cmd => self.zoom_reset(cx),
            "z" if cmd && k.modifiers.shift => self.redo(window, cx),
            "z" if cmd => self.undo(window, cx),
            "y" if cmd => self.redo(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---------------------------------------------------------- running

    /// Read the data, check the model and estimate it on another thread
    /// (with the bootstrap when `bootstrap`).
    fn run(&mut self, bootstrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        if !self.read_settings(cx) {
            return;
        }
        let (sheet, range) = self.diagram.data_ref();
        let (sheet, range) = (
            sheet.unwrap_or_default().to_owned(),
            range.unwrap_or_default().to_owned(),
        );
        let Some(view) = self.sheets.upgrade() else {
            return;
        };
        let loaded = view.read(cx).read_book(cx, |book, _| {
            let (s, r) = data::locate(book, &sheet, &range).ok_or_else(|| {
                anyhow::anyhow!(tf(
                    "The data range {}!{} isn't in this workbook",
                    &[&sheet, &range]
                ))
            })?;
            data::read(book, s, r)
        });
        let loaded = match loaded {
            Ok(l) => l,
            Err(e) => {
                self.say(format!("{e:#}"), true, cx);
                return;
            }
        };
        self.cases = Some(loaded.data.rows());
        let mut model = self.diagram.model.clone();
        model.layout = None;
        let mut opts = self.diagram.options();
        if !bootstrap {
            opts.bootstrap = None;
        } else {
            opts.bootstrap.get_or_insert_with(Default::default);
        }
        model.options = None;
        let findings = kantor_pls::check(&loaded.data, &model, &opts);
        let comma = self.fmt().comma;
        let errors: Vec<String> = findings
            .iter()
            .filter(|f| f.level == kantor_pls::Level::Error)
            .map(|f| labels::engine_message(f.check, &f.args, &f.message, comma))
            .collect();
        if !errors.is_empty() {
            self.say(
                tf("The model can't be estimated: {}", &[&errors.join("; ")]),
                true,
                cx,
            );
            return;
        }
        let used: BTreeSet<&str> = model
            .constructs
            .iter()
            .flat_map(|c| c.indicators.iter().map(String::as_str))
            .collect();
        let text_note: Vec<String> = loaded
            .text_cells
            .iter()
            .filter(|(c, _)| used.contains(c.as_str()))
            .map(|(c, n)| format!("{c} ({n})"))
            .collect();
        let csv = data_csv(&loaded.data);
        let progress = Arc::new(Progress::new());
        let (tx, rx) = futures::channel::oneshot::channel();
        let worker = {
            let progress = progress.clone();
            let data = loaded.data;
            let model = model.clone();
            let opts = opts.clone();
            std::thread::Builder::new()
                .name("kantor-pls".into())
                .spawn(move || {
                    let r = kantor_pls::estimate_with(&data, &model, &opts, Some(&progress));
                    let _ = tx.send(r);
                })
        };
        if let Err(e) = worker {
            self.say(format!("{e}"), true, cx);
            return;
        }
        let mut rx = rx;
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                match rx.try_recv() {
                    Ok(None) => {
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                    Ok(Some(r)) => {
                        let _ = this.update(cx, |this, cx| this.finished(r, cx));
                        return;
                    }
                    Err(_) => {
                        let _ = this.update(cx, |this, cx| {
                            this.running = None;
                            cx.notify();
                        });
                        return;
                    }
                }
            }
        });
        self.running = Some(Running {
            progress,
            bootstrap,
            _task: task,
        });
        self.results_model = Some(self.diagram.model.clone());
        self.results_csv = Some(csv);
        let note = if text_note.is_empty() {
            String::new()
        } else {
            tf(
                " · text read as missing values in {}",
                &[&text_note.join(", ")],
            )
        };
        self.say(
            format!(
                "{}{note}",
                if bootstrap {
                    t("Bootstrapping…")
                } else {
                    t("Calculating…")
                }
            ),
            false,
            cx,
        );
    }

    fn finished(&mut self, r: Result<Results, kantor_pls::Error>, cx: &mut Context<Self>) {
        let boot = self.running.as_ref().is_some_and(|r| r.bootstrap);
        self.running = None;
        match r {
            Ok(r) => {
                let msg = if boot {
                    tf(
                        "Bootstrap done: {} subsamples, {} cases",
                        &[&r.bootstrap.as_ref().map_or(0, |b| b.samples), &r.n],
                    )
                } else {
                    tf(
                        "Calculated on {} cases in {} iterations",
                        &[&r.n, &r.iterations],
                    )
                };
                let warn = !r.warnings.is_empty();
                // The first results bring the results pane: fit the diagram
                // to the smaller canvas.
                if self.results.is_none() {
                    self.request_fit(cx);
                }
                self.results = Some(Arc::new(r));
                self.show_results = true;
                if warn {
                    self.say(
                        format!("{msg} · {}", t("see the warnings under Model fit")),
                        false,
                        cx,
                    );
                } else {
                    self.say(msg, false, cx);
                }
            }
            Err(kantor_pls::Error::Cancelled) => self.say(t("Stopped"), false, cx),
            Err(e) => self.say(tf("The estimation failed: {}", &[&e]), true, cx),
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = &self.running {
            r.progress.cancel();
            self.say(t("Stopping…"), false, cx);
        }
    }

    // ------------------------------------------------------- the files

    fn save_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = kantor_core::dirs::user_home_dir().unwrap_or_else(|| PathBuf::from("."));
        let name = format!("{}.json", self.diagram.name());
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("json");
            }
            let _ = this.update(cx, |this, cx| {
                match std::fs::write(&path, this.diagram.to_json()) {
                    Ok(()) => this.say(tf("Model saved as {}", &[&path.display()]), false, cx),
                    Err(e) => this.say(tf("Can't save the model: {}", &[&e]), true, cx),
                }
            });
        })
        .detach();
    }

    /// Export the results as a `.docx` report (English or Indonesian) or a
    /// `.xlsx` workbook, written on another thread.
    fn export(&mut self, ext: &str, indonesian: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.fresh_results().cloned() else {
            self.say(t("Calculate the model first"), true, cx);
            return;
        };
        let Some(hooks) = cx.try_global::<PlsHooks>() else {
            self.say(t("Reports can't be written here"), true, cx);
            return;
        };
        let write = hooks.write;
        let dir = kantor_core::dirs::user_home_dir().unwrap_or_else(|| PathBuf::from("."));
        let stem = self.diagram.name().to_owned();
        let name = match (ext, indonesian) {
            ("docx", true) => format!("{stem} - laporan.docx"),
            ("docx", false) => format!("{stem} - report.docx"),
            _ => format!("{stem} - results.{ext}"),
        };
        let ext = ext.to_owned();
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(&ext);
            }
            let _ = this.update(cx, |this, cx| this.say(t("Writing…"), false, cx));
            let target = path.clone();
            let done = cx
                .background_executor()
                .spawn(async move { write(&r, &target, indonesian) })
                .await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(()) => this.say(tf("Saved {}", &[&path.display()]), false, cx),
                Err(e) => this.say(tf("Can't write {}: {}", &[&path.display(), &e]), true, cx),
            });
        })
        .detach();
    }

    fn section_tables(&self) -> Vec<ResultTable> {
        match self.fresh_results() {
            Some(r) => labels::tables(r, self.section, &self.fmt()),
            None => Vec::new(),
        }
    }

    fn copy_table(&mut self, table: &ResultTable, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(table.tsv()));
        self.say(tf("Copied “{}”", &[&table.title]), false, cx);
    }

    fn insert_tables(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.fresh_results() else {
            self.say(t("Calculate the model first"), true, cx);
            return;
        };
        let f = self.fmt();
        let tables: Vec<ResultTable> = Section::ALL
            .iter()
            .flat_map(|s| labels::tables(r, *s, &f))
            .collect();
        cx.emit(PlsEvent::InsertTables(tables));
    }

    /// Write the model, the data and the results' summary to a folder and
    /// hand them to Paparan AI with a prompt asking for the interpretation.
    fn explain(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.fresh_results().cloned() else {
            self.say(t("Calculate the model first"), true, cx);
            return;
        };
        let Some(summary) = cx.try_global::<PlsHooks>().map(|h| h.summary) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "kantor-pls-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        ));
        let write = || -> std::io::Result<Vec<PathBuf>> {
            std::fs::create_dir_all(&dir)?;
            let model = dir.join("model.json");
            std::fs::write(&model, self.diagram.to_json())?;
            let results = dir.join("results.json");
            std::fs::write(&results, summary(&r))?;
            let mut files = vec![model, results];
            if let Some(csv) = &self.results_csv {
                let data = dir.join("data.csv");
                std::fs::write(&data, csv)?;
                files.push(data);
            }
            Ok(files)
        };
        match write() {
            Ok(files) => {
                let prompt = if self.diagram.one_tailed() {
                    t(
                        "Write the interpretation of these PLS-SEM results for the results chapter of a thesis: the measurement model (reliability, convergent and discriminant validity), the structural model (path coefficients with one-tailed p values, R², f², indirect effects) and model fit, with the usual thresholds. model.json is the model, data.csv its data and results.json the results; the pls tool can estimate it again.",
                    )
                } else {
                    t(
                        "Write the interpretation of these PLS-SEM results for the results chapter of a thesis: the measurement model (reliability, convergent and discriminant validity), the structural model (path coefficients with two-tailed p values, R², f², indirect effects) and model fit, with the usual thresholds. model.json is the model, data.csv its data and results.json the results; the pls tool can estimate it again.",
                    )
                };
                cx.emit(PlsEvent::Explain(prompt.to_owned(), files));
                self.say(
                    t("Sent to Paparan AI: press Enter in the chat to start"),
                    false,
                    cx,
                );
            }
            Err(e) => self.say(
                tf("Can't write the files for Paparan AI: {}", &[&e]),
                true,
                cx,
            ),
        }
    }

    /// The data range changed (Use selection): new columns.
    pub(crate) fn set_data(
        &mut self,
        sheet: &str,
        range: &str,
        columns: Vec<String>,
        cases: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.diagram.set_data_ref(sheet, range);
        self.columns = columns;
        self.cases = cases;
        self.checked.clear();
        self.changed(cx);
    }

    fn use_selection(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.sheets.upgrade() else {
            return;
        };
        let found = view.read(cx).pls_data_range(cx);
        match found {
            Some((sheet, range, columns, cases)) => {
                self.set_data(&sheet, &range, columns, Some(cases), cx);
                self.say(tf("The data is now {}!{}", &[&sheet, &range]), false, cx);
            }
            None => self.say(
                t("Select the data with its header row on a worksheet first"),
                true,
                cx,
            ),
        }
    }
}

/// The data as CSV (empty cells for missing values).
fn data_csv(d: &kantor_pls::Data) -> String {
    let quote = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out: String = d
        .names()
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for r in 0..d.rows() {
        let row: Vec<String> = (0..d.cols())
            .map(|c| {
                let v = d.get(r, c);
                if v.is_finite() {
                    v.to_string()
                } else {
                    String::new()
                }
            })
            .collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------- render

fn seg_button(
    id: impl Into<ElementId>,
    label: &str,
    on: bool,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .xsmall()
        .label(label.to_owned())
        .when_else(on, |b| b.primary(), |b| b.ghost())
        .on_click(move |_, w, cx| f(w, cx))
}

impl PlsView {
    fn render_bar(&self, cx: &mut Context<Self>) -> Div {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let running = self.running.is_some();
        let has_results = self.fresh_results().is_some();
        let v = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let view = view.clone();
            move |_: &ClickEvent, w: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| f(this, w, cx));
            }
        };
        let export_view = view.clone();
        let mut bar = h_flex()
            .flex_none()
            .h(CONTROL_LG + SPACE_3)
            .px(SPACE_4)
            .gap(SPACE_3)
            .bg(t_.chrome)
            .border_b_1()
            .border_color(t_.border)
            .child(gpui_kit::component::Icon::new(Lucide::Network).small())
            .child(
                div()
                    .w(px(170.))
                    .child(Input::new(&self.name_input).small()),
            )
            .child(
                Button::new("pls-calc")
                    .small()
                    .primary()
                    .icon(Lucide::Play)
                    .label(t("Calculate"))
                    .tooltip(t("Estimate the model (PLS algorithm)"))
                    .disabled(running)
                    .on_click(v(|this, w, cx| this.run(false, w, cx))),
            )
            .child(
                Button::new("pls-boot")
                    .small()
                    .icon(Lucide::Sigma)
                    .label(t("Bootstrap"))
                    .tooltip(t("Estimate with bootstrap standard errors, t and p values"))
                    .disabled(running)
                    .on_click(v(|this, w, cx| this.run(true, w, cx))),
            );
        if let Some(r) = &self.running {
            let (done, total) = r.progress.get();
            let frac = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            let stop = view.clone();
            bar = bar
                .child(
                    div()
                        .w(px(120.))
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(t_.border)
                        .child(
                            div()
                                .h_full()
                                .w(px(120. * frac.clamp(0.0, 1.0)))
                                .rounded(px(3.))
                                .bg(t_.accent),
                        ),
                )
                .when(total > 0, |d| {
                    d.child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t_.fg_muted)
                            .child(format!("{done}/{total}")),
                    )
                })
                .child(
                    Button::new("pls-stop")
                        .small()
                        .ghost()
                        .icon(Lucide::Square)
                        .label(t("Stop"))
                        .on_click(move |_, _, cx| {
                            let _ = stop.update(cx, |this, cx| this.stop(cx));
                        }),
                );
        }
        bar.child(div().w(px(1.)).h(CONTROL_MD).bg(t_.border))
            .child(
                Button::new("pls-layout")
                    .small()
                    .ghost()
                    .icon(Lucide::LayoutGrid)
                    .tooltip(t("Auto layout: left to right by path depth"))
                    .on_click(v(|this, _, cx| this.auto_layout(cx))),
            )
            .child(
                Button::new("pls-fit")
                    .small()
                    .ghost()
                    .icon(Lucide::Maximize)
                    .tooltip(t("Fit to view"))
                    .on_click(v(|this, _, cx| this.request_fit(cx))),
            )
            .child(
                Button::new("pls-zoom-out")
                    .small()
                    .ghost()
                    .icon(Lucide::ZoomOut)
                    .tooltip(t("Zoom out (Ctrl+wheel)"))
                    .disabled(self.zoom <= MIN_ZOOM + 1e-9)
                    .on_click(v(|this, _, cx| this.zoom_by(1.0 / ZOOM_STEP, None, cx))),
            )
            .child(
                Button::new("pls-zoom-reset")
                    .small()
                    .ghost()
                    .label(format!("{:.0}%", self.zoom * 100.0))
                    .tooltip(t("Zoom to 100%"))
                    .on_click(v(|this, _, cx| this.zoom_reset(cx))),
            )
            .child(
                Button::new("pls-zoom-in")
                    .small()
                    .ghost()
                    .icon(Lucide::ZoomIn)
                    .tooltip(t("Zoom in (Ctrl+wheel)"))
                    .disabled(self.zoom >= MAX_ZOOM - 1e-9)
                    .on_click(v(|this, _, cx| this.zoom_by(ZOOM_STEP, None, cx))),
            )
            .child(
                Button::new("pls-undo")
                    .small()
                    .ghost()
                    .icon(Lucide::Undo2)
                    .tooltip(t("Undo (Ctrl+Z)"))
                    .disabled(!self.diagram.can_undo())
                    .on_click(v(|this, w, cx| this.undo(w, cx))),
            )
            .child(
                Button::new("pls-redo")
                    .small()
                    .ghost()
                    .icon(Lucide::Redo2)
                    .tooltip(t("Redo (Ctrl+Y)"))
                    .disabled(!self.diagram.can_redo())
                    .on_click(v(|this, w, cx| this.redo(w, cx))),
            )
            .child(
                Button::new("pls-save-model")
                    .small()
                    .ghost()
                    .icon(Lucide::Save)
                    .tooltip(t("Save the model as .json"))
                    .on_click(v(|this, w, cx| this.save_model(w, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("pls-explain")
                    .small()
                    .ghost()
                    .icon(Lucide::Sparkles)
                    .label(t("Explain results"))
                    .tooltip(t("Ask Paparan AI to write the interpretation"))
                    .disabled(!has_results)
                    .on_click(v(|this, _, cx| this.explain(cx))),
            )
            .child(
                Button::new("pls-export")
                    .small()
                    .ghost()
                    .icon(Lucide::FileDown)
                    .label(t("Export"))
                    .dropdown_caret(true)
                    .disabled(!has_results)
                    .dropdown_menu(move |menu, _, _| {
                        let items: [(&str, &str, bool); 3] = [
                            (t("Report in English (.docx)"), "docx", false),
                            (t("Report in Indonesian (.docx)"), "docx", true),
                            (t("All results (.xlsx)"), "xlsx", false),
                        ];
                        let mut menu = menu;
                        for (label, ext, id) in items {
                            let v = export_view.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                                    let _ = v.update(cx, |this, cx| this.export(ext, id, w, cx));
                                }));
                        }
                        menu
                    }),
            )
    }

    fn render_indicators(&self, cx: &mut Context<Self>) -> Div {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let (sheet, range) = self.diagram.data_ref();
        let data_text = format!(
            "{}!{}{}",
            sheet.unwrap_or("?"),
            range.unwrap_or("?"),
            self.cases
                .map(|n| tf(" · {} cases", &[&n]))
                .unwrap_or_default()
        );
        let mut list = v_flex()
            .id("pls-indicators")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t_.border)
            .bg(t_.surface)
            .py(SPACE_2);
        for (i, col) in self.columns.iter().enumerate() {
            let owner = self.diagram.construct_of(col).map(str::to_owned);
            let checked = self.checked.contains(col);
            let names: Vec<String> = if checked {
                self.columns
                    .iter()
                    .filter(|c| self.checked.contains(*c))
                    .cloned()
                    .collect()
            } else {
                vec![col.clone()]
            };
            let label: SharedString = if names.len() > 1 {
                tf("{} indicators", &[&names.len()]).into()
            } else {
                col.clone().into()
            };
            let drag = IndicatorDrag { names };
            let v = view.clone();
            let name = col.clone();
            list = list.child(
                h_flex()
                    .id(("pls-ind", i))
                    .px(SPACE_3)
                    .h(CONTROL_MD)
                    .gap(SPACE_3)
                    .cursor_grab()
                    .hover(|s| s.bg(t_.hover))
                    .on_drag(drag, move |_, _, _, cx| {
                        let label = label.clone();
                        cx.new(|_| DragChip { label })
                    })
                    .child(
                        Checkbox::new(("pls-ind-check", i))
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if !this.checked.remove(&name) {
                                        this.checked.insert(name.clone());
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if owner.is_some() { t_.fg_muted } else { t_.fg })
                            .child(col.clone()),
                    )
                    .when_some(owner, |d, o| {
                        d.child(
                            div()
                                .max_w(px(70.))
                                .truncate()
                                .text_size(FONT_CAPTION)
                                .text_color(t_.fg_dim)
                                .child(o),
                        )
                    }),
            );
        }
        let create = view.clone();
        let take = view.clone();
        let any_checked = !self.checked.is_empty();
        v_flex()
            .flex_none()
            .w(px(210.))
            .h_full()
            .p(SPACE_4)
            .gap(SPACE_3)
            .bg(t_.chrome)
            .border_r_1()
            .border_color(t_.border)
            .child(kantor_ui::components::section_label(t("Data"), cx))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t_.fg_muted)
                    .truncate()
                    .child(data_text),
            )
            .child(
                Button::new("pls-use-selection")
                    .xsmall()
                    .ghost()
                    .label(t("Use the selected cells"))
                    .tooltip(t(
                        "Take the data (header row first) from the selection on the worksheet",
                    ))
                    .on_click(move |_, _, cx| {
                        let _ = take.update(cx, |this, cx| this.use_selection(cx));
                    }),
            )
            .child(kantor_ui::components::section_label(
                t("Indicators · check or drag onto the diagram"),
                cx,
            ))
            .child(list)
            .child(
                Button::new("pls-create")
                    .small()
                    .icon(Lucide::Plus)
                    .label(t("Create construct"))
                    .disabled(!any_checked)
                    .on_click(move |_, w, cx| {
                        let _ = create.update(cx, |this, cx| this.create_from_checked(w, cx));
                    }),
            )
    }

    /// The canvas's shapes and the labels over them.
    fn scene(&self, t_: &kantor_ui::Tokens) -> (Vec<Prim>, Vec<AnyElement>) {
        let d = &self.diagram;
        let (sx, sy) = self.scroll;
        let z = self.zoom;
        let zf = z as f32;
        let at = |p: (f64, f64)| ((p.0 * z + sx) as f32, (p.1 * z + sy) as f32);
        let fresh = self.fresh_results();
        let shown = fresh.map(|r| labels::diagram_labels(r, &self.fmt()));
        let mut prims = Vec::new();
        let mut texts: Vec<AnyElement> = Vec::new();
        let caption = FONT_CAPTION * zf;
        let chip = |x: f32, y: f32, text: String, color: Hsla, bg: Hsla| {
            div()
                .absolute()
                .left(px(x - 50. * zf))
                .top(px(y - 9. * zf))
                .w(px(100. * zf))
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(SPACE_2 * zf)
                        .rounded(RADIUS_SM)
                        .bg(bg)
                        .text_size(caption)
                        .text_color(color)
                        .child(text),
                )
                .into_any_element()
        };
        // Indicator links first, so the boxes of a staggered second row
        // cover the links running behind them.
        let boxes: Vec<(&kantor_pls::Construct, Vec<(String, diagram::Rect)>)> = d
            .model
            .constructs
            .iter()
            .map(|c| (c, d.indicator_boxes(c)))
            .collect();
        for (c, list) in &boxes {
            let formative = c.measurement == Measurement::Composite;
            for (_, r) in list {
                let (a, b) = d.indicator_link(c, r);
                // Reflective: the construct points to its indicators;
                // formative: the indicators point to the construct.
                let (from, to) = if formative { (a, b) } else { (b, a) };
                prims.push(Prim::Line {
                    a: at(from),
                    b: at(to),
                    width: 1.0,
                    color: t_.fg_dim,
                    dashed: false,
                });
                let [l, rr] = diagram::arrow_head(from, to, 7.0);
                prims.push(Prim::Polygon {
                    points: vec![at(to), at(l), at(rr)],
                    fill: Some(t_.fg_dim),
                    stroke: None,
                });
            }
        }
        for (c, list) in &boxes {
            for (k, (name, r)) in list.iter().enumerate() {
                let selected = self.selection == Some(Hit::Indicator(name.clone()));
                let (x, y) = at((r.x, r.y));
                let moved = diagram::Rect {
                    x: f64::from(x),
                    y: f64::from(y),
                    w: r.w * z,
                    h: r.h * z,
                };
                prims.push(Prim::Rect {
                    r: moved,
                    fill: t_.surface,
                    border: if selected {
                        t_.accent
                    } else {
                        t_.border_strong
                    },
                    width: if selected { 2.0 } else { 1.0 },
                });
                texts.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(IND_W as f32 * zf))
                        .h(px(IND_H as f32 * zf))
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(SPACE_1 * zf)
                        .text_size(caption)
                        .text_color(t_.fg)
                        .child(div().truncate().child(name.clone()))
                        .into_any_element(),
                );
                if let Some((text, low)) = shown.as_ref().and_then(|s| s.outer.get(name)) {
                    let (x, y) = at(d.loading_point(c, k, r));
                    texts.push(chip(
                        x,
                        y,
                        text.clone(),
                        if *low { t_.danger } else { t_.fg_muted },
                        t_.bg,
                    ));
                }
            }
        }
        // Paths.
        for p in &d.model.paths {
            let (a, b) = d.path_line(&p.from, &p.to);
            let selected = self.selection == Some(Hit::Path(p.from.clone(), p.to.clone()));
            let label = shown
                .as_ref()
                .and_then(|s| s.paths.get(&(p.from.clone(), p.to.clone())));
            let not_sig = label.is_some_and(|(_, s)| *s == Some(false));
            let color = if selected {
                t_.accent
            } else if label.is_some_and(|(_, s)| *s == Some(true)) {
                t_.success
            } else {
                t_.fg_muted
            };
            prims.push(Prim::Line {
                a: at(a),
                b: at(b),
                width: if selected { 2.5 } else { 1.5 },
                color,
                dashed: not_sig,
            });
            let [l, r] = diagram::arrow_head(a, b, 10.0);
            prims.push(Prim::Polygon {
                points: vec![at(b), at(l), at(r)],
                fill: Some(color),
                stroke: None,
            });
            if let Some((text, _)) = label {
                let (x, y) = at(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
                texts.push(chip(x, y, text.clone(), t_.fg, t_.chrome));
            }
        }
        // Constructs.
        for c in &d.model.constructs {
            let center = d.position(&c.name);
            let selected = self.selection == Some(Hit::Construct(c.name.clone()));
            let formative = c.measurement == Measurement::Composite;
            let points: Vec<(f32, f32)> = (0..48)
                .map(|i| {
                    let a = i as f64 / 48.0 * std::f64::consts::TAU;
                    at((center.0 + RX * a.cos(), center.1 + RY * a.sin()))
                })
                .collect();
            prims.push(Prim::Polygon {
                points,
                fill: Some(if formative {
                    t_.accent_soft
                } else {
                    t_.elevated
                }),
                stroke: Some((
                    if selected { t_.accent } else { t_.sheets },
                    if selected { 2.5 } else { 1.5 },
                )),
            });
            let (x, y) = at((center.0 - RX, center.1 - RY));
            let r2 = shown.as_ref().and_then(|s| s.r2.get(&c.name)).cloned();
            texts.push(
                v_flex()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(2.0 * RX as f32 * zf))
                    .h(px(2.0 * RY as f32 * zf))
                    .items_center()
                    .justify_center()
                    .px(SPACE_4 * zf)
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(FONT_SMALL * zf)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t_.fg)
                            .child(c.name.clone()),
                    )
                    .when_some(r2, |d, r2| {
                        d.child(div().text_size(caption).text_color(t_.fg_muted).child(r2))
                    })
                    .into_any_element(),
            );
            if selected {
                let h = at(self.handle_of(&c.name));
                prims.push(Prim::Polygon {
                    points: (0..16)
                        .map(|i| {
                            let a = i as f32 / 16.0 * std::f32::consts::TAU;
                            (h.0 + 5.0 * a.cos(), h.1 + 5.0 * a.sin())
                        })
                        .collect(),
                    fill: Some(t_.accent),
                    stroke: None,
                });
            }
        }
        if let Some(CanvasDrag::Connect { from, to }) = &self.drag {
            let a = diagram::ellipse_point(d.position(from), *to);
            prims.push(Prim::Line {
                a: at(a),
                b: at(*to),
                width: 1.5,
                color: t_.accent,
                dashed: true,
            });
        }
        (prims, texts)
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t_ = tokens(cx);
        let (prims, texts) = self.scene(&t_);
        let bounds = self.bounds.clone();
        let pending = self.fit_pending.clone();
        let fitted = self.fitted.clone();
        let empty = self.diagram.model.constructs.is_empty();
        let accent = t_.accent;
        let menu_view = cx.entity().downgrade();
        let me = cx.entity().downgrade();
        let painter = canvas(
            move |b, window, _| {
                bounds.set(b);
                // Fit to view needs this size: render again, fitted (and
                // once more if the size changed after the fit). A refresh
                // asked for during this draw would be lost, so on the next
                // frame.
                let again = if pending.get() {
                    true
                } else if let Some(size) = fitted.take()
                    && size != b.size
                {
                    pending.set(true);
                    true
                } else {
                    false
                };
                if again && b.size.width > px(1.) && b.size.height > px(1.) {
                    let me = me.clone();
                    window.on_next_frame(move |_, cx| {
                        if let Some(me) = me.upgrade() {
                            me.update(cx, |_, cx| cx.notify());
                        }
                    });
                }
            },
            move |b, _, window, _| {
                let o = b.origin;
                let pt = |(x, y): (f32, f32)| point(o.x + px(x), o.y + px(y));
                window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                    for prim in &prims {
                        match prim {
                            Prim::Line {
                                a,
                                b,
                                width,
                                color,
                                dashed,
                            } => {
                                let mut p = PathBuilder::stroke(px(*width));
                                if *dashed {
                                    p = p.dash_array(&[px(5.), px(4.)]);
                                }
                                p.move_to(pt(*a));
                                p.line_to(pt(*b));
                                if let Ok(path) = p.build() {
                                    window.paint_path(path, *color);
                                }
                            }
                            Prim::Polygon {
                                points,
                                fill,
                                stroke,
                            } => {
                                let pts: Vec<Point<Pixels>> =
                                    points.iter().map(|p| pt(*p)).collect();
                                if let Some(c) = fill {
                                    let mut p = PathBuilder::fill();
                                    p.add_polygon(&pts, true);
                                    if let Ok(path) = p.build() {
                                        window.paint_path(path, *c);
                                    }
                                }
                                if let Some((c, w)) = stroke {
                                    let mut p = PathBuilder::stroke(px(*w));
                                    p.add_polygon(&pts, true);
                                    if let Ok(path) = p.build() {
                                        window.paint_path(path, *c);
                                    }
                                }
                            }
                            Prim::Rect {
                                r,
                                fill: f,
                                border,
                                width,
                            } => {
                                let bounds = Bounds::new(
                                    pt((r.x as f32, r.y as f32)),
                                    size(px(r.w as f32), px(r.h as f32)),
                                );
                                window.paint_quad(
                                    fill(bounds, *f)
                                        .corner_radii(px(3.))
                                        .border_widths(px(*width))
                                        .border_color(*border),
                                );
                            }
                        }
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        div()
            .id("pls-canvas")
            .key_context("KantorPlsCanvas")
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(t_.bg)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                if ev.modifiers.control || ev.modifiers.platform {
                    // Ctrl+wheel zooms around the pointer.
                    let dy = f64::from(f32::from(d.y));
                    if dy.abs() > 0.01 {
                        let factor = (dy / 120.0).exp().clamp(0.5, 2.0);
                        this.zoom_by(factor, Some(ev.position), cx);
                    }
                    return;
                }
                this.scroll = (
                    this.scroll.0 + f64::from(f32::from(d.x)),
                    this.scroll.1 + f64::from(f32::from(d.y)),
                );
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, ev: &PinchEvent, _, cx| {
                this.zoom_by(f64::from(1.0 + ev.delta), Some(ev.position), cx);
            }))
            .drag_over::<IndicatorDrag>(move |s, _, _, _| s.border_2().border_color(accent))
            .on_drop(cx.listener(|this, d: &IndicatorDrag, w, cx| this.drop_indicators(d, w, cx)))
            .child(painter)
            .children(texts)
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(SPACE_8)
                        .child(
                            div()
                                .max_w(px(360.))
                                .text_center()
                                .text_size(FONT_SMALL)
                                .text_color(t_.fg_dim)
                                .child(t(
                                    "Drag indicators here (or check them and press Create construct) to make a construct",
                                )),
                        ),
                )
            })
            .context_menu(move |menu, window, cx| {
                let Some(v) = menu_view.upgrade() else {
                    return menu;
                };
                let target = {
                    let this = v.read(cx);
                    let p = this.to_diagram(window.mouse_position());
                    match this.diagram.hit(p) {
                        Some(Hit::Construct(n)) => Some(n),
                        Some(Hit::Indicator(i)) => {
                            this.diagram.construct_of(&i).map(str::to_owned)
                        }
                        _ => None,
                    }
                };
                let mut menu = menu;
                if let Some(name) = target {
                    let (chosen, moved) = {
                        let this = v.read(cx);
                        let moved = this.diagram.construct(&name).is_some_and(|c| {
                            c.indicators.iter().any(|i| this.diagram.indicator_moved(i))
                        });
                        (this.diagram.side_chosen(&name), moved)
                    };
                    let sides: [(&str, Option<Side>); 5] = [
                        (t("Indicators on the left"), Some(Side::Left)),
                        (t("Indicators on the right"), Some(Side::Right)),
                        (t("Indicators above"), Some(Side::Top)),
                        (t("Indicators below"), Some(Side::Bottom)),
                        (t("Indicators placed automatically"), None),
                    ];
                    for (label, side) in sides {
                        let v = v.downgrade();
                        let n = name.clone();
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(chosen == side && !moved)
                                .on_click(move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| this.set_side(&n, side, cx));
                                }),
                        );
                    }
                    let v2 = v.downgrade();
                    let n = name.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new(t("Put moved indicators back"))
                            .disabled(!moved)
                            .on_click(move |_, _, cx| {
                                let _ = v2.update(cx, |this, cx| this.reset_indicators(&n, cx));
                            }),
                    );
                    menu = menu.separator();
                }
                let (fit, layout) = (v.downgrade(), v.downgrade());
                menu.item(
                    PopupMenuItem::new(t("Fit to view")).on_click(move |_, _, cx| {
                        let _ = fit.update(cx, |this, cx| this.request_fit(cx));
                    }),
                )
                .item(PopupMenuItem::new(t("Auto layout")).on_click(
                    move |_, _, cx| {
                        let _ = layout.update(cx, |this, cx| this.auto_layout(cx));
                    },
                ))
            })
    }

    fn render_side(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let mut side = v_flex()
            .id("pls-side")
            .flex_none()
            .w(px(260.))
            .h_full()
            .p(SPACE_4)
            .gap(SPACE_3)
            .bg(t_.chrome)
            .border_l_1()
            .border_color(t_.border)
            .overflow_y_scroll();
        // The selection.
        match &self.selection {
            Some(Hit::Construct(name)) => {
                if let Some(c) = self.diagram.construct(name) {
                    let reflective = c.measurement == Measurement::Reflective;
                    let (a, b, del, add) = (view.clone(), view.clone(), view.clone(), view.clone());
                    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
                    side = side
                        .child(kantor_ui::components::section_label(t("Construct"), cx))
                        .child(Input::new(&self.rename_input).small())
                        .child(
                            h_flex()
                                .gap(SPACE_2)
                                .child(seg_button(
                                    "pls-refl",
                                    t("Reflective"),
                                    reflective,
                                    move |_, cx| {
                                        let _ = a.update(cx, |this, cx| {
                                            this.set_measurement(&n1, Measurement::Reflective, cx)
                                        });
                                    },
                                ))
                                .child(seg_button(
                                    "pls-form",
                                    t("Formative"),
                                    !reflective,
                                    move |_, cx| {
                                        let _ = b.update(cx, |this, cx| {
                                            this.set_measurement(&n2, Measurement::Composite, cx)
                                        });
                                    },
                                )),
                        )
                        .child(div().text_size(FONT_CAPTION).text_color(t_.fg_dim).child(
                            if reflective {
                                t("Mode A: the construct causes its indicators")
                            } else {
                                t("Mode B: the indicators form the construct")
                            },
                        ));
                    // Where its indicators are drawn.
                    let chosen = self.diagram.side_chosen(name);
                    let mut sides = h_flex().gap(SPACE_1).flex_wrap();
                    let options: [(&str, Option<Side>); 5] = [
                        (t("Auto"), None),
                        (t("Left"), Some(Side::Left)),
                        (t("Right"), Some(Side::Right)),
                        (t("Above"), Some(Side::Top)),
                        (t("Below"), Some(Side::Bottom)),
                    ];
                    for (k, (label, which)) in options.into_iter().enumerate() {
                        let v = view.clone();
                        let n = name.clone();
                        sides = sides.child(seg_button(
                            ("pls-side", k),
                            label,
                            chosen == which,
                            move |_, cx| {
                                let _ = v.update(cx, |this, cx| this.set_side(&n, which, cx));
                            },
                        ));
                    }
                    side = side
                        .child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t_.fg_muted)
                                .child(t("Indicators")),
                        )
                        .child(sides);
                    for (i, ind) in c.indicators.iter().enumerate() {
                        let v = view.clone();
                        let ind2 = ind.clone();
                        side = side.child(
                            h_flex()
                                .gap(SPACE_2)
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_size(FONT_SMALL)
                                        .child(ind.clone()),
                                )
                                .child(
                                    Button::new(("pls-rm-ind", i))
                                        .xsmall()
                                        .ghost()
                                        .icon(Lucide::X)
                                        .tooltip(t("Remove from the construct"))
                                        .on_click(move |_, _, cx| {
                                            let _ = v.update(cx, |this, cx| {
                                                if this.diagram.remove_indicator(&ind2) {
                                                    this.changed(cx);
                                                }
                                            });
                                        }),
                                ),
                        );
                    }
                    side = side
                        .child(
                            Button::new("pls-add-checked")
                                .xsmall()
                                .ghost()
                                .icon(Lucide::Plus)
                                .label(t("Add the checked indicators"))
                                .disabled(self.checked.is_empty())
                                .on_click(move |_, _, cx| {
                                    let _ = add.update(cx, |this, cx| this.add_checked_to(&n3, cx));
                                }),
                        )
                        .child(
                            Button::new("pls-del-construct")
                                .xsmall()
                                .ghost()
                                .icon(Lucide::Trash)
                                .label(t("Delete construct"))
                                .on_click(move |_, _, cx| {
                                    let _ = del.update(cx, |this, cx| this.delete_selection(cx));
                                }),
                        );
                }
            }
            Some(Hit::Path(a, b)) => {
                let del = view.clone();
                side = side
                    .child(kantor_ui::components::section_label(t("Path"), cx))
                    .child(div().text_size(FONT_BODY).child(format!("{a} → {b}")))
                    .child(
                        Button::new("pls-del-path")
                            .xsmall()
                            .ghost()
                            .icon(Lucide::Trash)
                            .label(t("Delete path"))
                            .on_click(move |_, _, cx| {
                                let _ = del.update(cx, |this, cx| this.delete_selection(cx));
                            }),
                    );
            }
            Some(Hit::Indicator(i)) => {
                let del = view.clone();
                let owner = self.diagram.construct_of(i).unwrap_or_default().to_owned();
                side = side
                    .child(kantor_ui::components::section_label(t("Indicator"), cx))
                    .child(div().text_size(FONT_BODY).child(format!("{i} · {owner}")))
                    .child(
                        Button::new("pls-del-ind")
                            .xsmall()
                            .ghost()
                            .icon(Lucide::X)
                            .label(t("Remove from the construct"))
                            .on_click(move |_, _, cx| {
                                let _ = del.update(cx, |this, cx| this.delete_selection(cx));
                            }),
                    );
            }
            None => {
                // How to edit, here rather than over the diagram.
                let tips = [
                    t(
                        "Shift+drag from one construct to another, or drag the ● of the selected one, to draw a path",
                    ),
                    t(
                        "Drag a construct or an indicator box to move it; right-click a construct to put its indicators on another side",
                    ),
                    t("Ctrl+wheel or pinch zooms; drag the background to pan"),
                    t("Del removes the selection; double-click a construct to rename it"),
                ];
                side = side.child(kantor_ui::components::section_label(t("Diagram"), cx));
                for tip in tips {
                    side = side.child(
                        div()
                            .text_size(FONT_CAPTION)
                            .text_color(t_.fg_dim)
                            .child(tip),
                    );
                }
            }
        }
        // The settings.
        let o = self.diagram.options();
        let boot = o.bootstrap.clone().unwrap_or_default();
        let one_tailed = self.diagram.one_tailed();
        // The significance levels with the locale's decimal separator.
        let comma = self.fmt().comma;
        let level = |s: &str| {
            if comma {
                s.replace('.', ",")
            } else {
                s.to_owned()
            }
        };
        let opt = |id: &'static str, label: &str, on: bool, f: fn(&mut Options)| -> Button {
            let v = view.clone();
            seg_button(id, label, on, move |_, cx| {
                let _ = v.update(cx, |this, cx| this.update_options(cx, f));
            })
        };
        let tail = |id: &'static str, label: &str, on: bool, value: bool| -> Button {
            let v = view.clone();
            seg_button(id, label, on, move |_, cx| {
                let _ = v.update(cx, |this, cx| {
                    if this.diagram.one_tailed() != value {
                        this.diagram.set_one_tailed(value);
                        this.changed(cx);
                    }
                });
            })
        };
        let row = |label: &str| {
            div()
                .text_size(FONT_SMALL)
                .text_color(t_.fg_muted)
                .child(label.to_owned())
        };
        side.child(kantor_ui::components::section_label(t("Settings"), cx))
            .child(row(t("Algorithm")))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(opt(
                        "pls-algo-pls",
                        "PLS",
                        o.algorithm == Algorithm::Pls,
                        |o| o.algorithm = Algorithm::Pls,
                    ))
                    .child(opt(
                        "pls-algo-plsc",
                        "PLSc",
                        o.algorithm == Algorithm::Plsc,
                        |o| o.algorithm = Algorithm::Plsc,
                    )),
            )
            .child(row(t("Weighting scheme")))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(opt(
                        "pls-w-path",
                        t("Path"),
                        o.weighting == Weighting::Path,
                        |o| o.weighting = Weighting::Path,
                    ))
                    .child(opt(
                        "pls-w-centroid",
                        t("Centroid"),
                        o.weighting == Weighting::Centroid,
                        |o| o.weighting = Weighting::Centroid,
                    ))
                    .child(opt(
                        "pls-w-factorial",
                        t("Factorial"),
                        o.weighting == Weighting::Factorial,
                        |o| o.weighting = Weighting::Factorial,
                    )),
            )
            .child(row(t("Bootstrap subsamples")))
            .child(Input::new(&self.samples_input).small())
            .child(row(t("Seed")))
            .child(Input::new(&self.seed_input).small())
            .child(row(t("Test type")))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(tail("pls-two", t("Two-tailed"), !one_tailed, false))
                    .child(tail("pls-one", t("One-tailed"), one_tailed, true)),
            )
            .child(row(t("Significance level")))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(opt(
                        "pls-a01",
                        &level("0.01"),
                        (boot.alpha - 0.01).abs() < 1e-9,
                        |o| o.bootstrap.get_or_insert_with(Default::default).alpha = 0.01,
                    ))
                    .child(opt(
                        "pls-a05",
                        &level("0.05"),
                        (boot.alpha - 0.05).abs() < 1e-9,
                        |o| o.bootstrap.get_or_insert_with(Default::default).alpha = 0.05,
                    ))
                    .child(opt(
                        "pls-a10",
                        &level("0.10"),
                        (boot.alpha - 0.10).abs() < 1e-9,
                        |o| o.bootstrap.get_or_insert_with(Default::default).alpha = 0.10,
                    )),
            )
            .child(row(t("Missing values")))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(opt(
                        "pls-m-list",
                        t("Delete rows"),
                        o.missing == Missing::Listwise,
                        |o| o.missing = Missing::Listwise,
                    ))
                    .child(opt(
                        "pls-m-mean",
                        t("Mean replacement"),
                        o.missing == Missing::Mean,
                        |o| o.missing = Missing::Mean,
                    )),
            )
            .child(row(t("Missing value code (optional, e.g. -99)")))
            .child(Input::new(&self.missing_input).small())
    }

    fn render_results(&self, cx: &mut Context<Self>) -> Option<Div> {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let stale = self.results.is_some() && self.fresh_results().is_none();
        self.results.as_ref()?;
        let hide = view.clone();
        let insert = view.clone();
        let mut tabs = h_flex().gap(SPACE_2);
        for (i, s) in Section::ALL.into_iter().enumerate() {
            let v = view.clone();
            tabs = tabs.child(seg_button(
                ("pls-sec", i),
                s.label(),
                s == self.section && self.show_results,
                move |_, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.section = s;
                        this.show_results = true;
                        cx.notify();
                    });
                },
            ));
        }
        let header = h_flex()
            .flex_none()
            .gap(SPACE_3)
            .px(SPACE_4)
            .h(CONTROL_LG)
            .border_b_1()
            .border_color(t_.border)
            .child(
                div()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t("Results")),
            )
            .child(tabs)
            .child(div().flex_1())
            .when(stale, |d| {
                d.child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t_.warning)
                        .child(t("The model changed: calculate again")),
                )
            })
            .child(
                Button::new("pls-insert-sheet")
                    .xsmall()
                    .ghost()
                    .icon(Lucide::Table)
                    .label(t("Insert as new sheet"))
                    .disabled(stale)
                    .on_click(move |_, _, cx| {
                        let _ = insert.update(cx, |this, cx| this.insert_tables(cx));
                    }),
            )
            .child(
                Button::new("pls-results-toggle")
                    .xsmall()
                    .ghost()
                    .icon(if self.show_results {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronUp
                    })
                    .tooltip(if self.show_results {
                        t("Hide the results")
                    } else {
                        t("Show the results")
                    })
                    .on_click(move |_, _, cx| {
                        let _ = hide.update(cx, |this, cx| {
                            this.show_results = !this.show_results;
                            cx.notify();
                        });
                    }),
            );
        let mut body = v_flex()
            .id("pls-results-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(SPACE_4)
            .gap(SPACE_6);
        if self.show_results && !stale {
            for (k, table) in self.section_tables().into_iter().enumerate() {
                body = body.child(self.render_table(k, table, cx));
            }
        }
        let open = self.show_results && !stale;
        // The handle between the diagram and the results: drag to resize.
        let handle = div()
            .id("pls-results-resize")
            .absolute()
            .top(px(-3.))
            .left_0()
            .right_0()
            .h(px(6.))
            .cursor_row_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.resizing = Some((f32::from(ev.position.y), this.results_h));
                    cx.stop_propagation();
                }),
            );
        Some(
            v_flex()
                .relative()
                .flex_none()
                .w_full()
                .when(open, |d| d.h(px(self.results_h)))
                .bg(t_.chrome)
                .border_t_1()
                .border_color(t_.border)
                .child(header)
                .when(open, |d| d.child(body).child(handle)),
        )
    }

    fn render_table(&self, k: usize, table: ResultTable, cx: &mut Context<Self>) -> Div {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let n = table.columns.len();
        let width = |j: usize| if j == 0 { px(150.) } else { px(96.) };
        // Number columns are right-aligned, text columns left.
        let numeric: Vec<bool> = (0..n)
            .map(|j| {
                table
                    .rows
                    .iter()
                    .any(|r| r.get(j).is_some_and(|c| c.value.is_some()))
            })
            .collect();
        let mut grid = v_flex()
            .border_1()
            .border_color(t_.border)
            .rounded(RADIUS_SM)
            .bg(t_.surface);
        let mut head = h_flex().bg(t_.hover).border_b_1().border_color(t_.border);
        for (j, c) in table.columns.iter().enumerate() {
            head = head.child(
                div()
                    .w(width(j))
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .truncate()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(numeric[j], |d| d.text_right())
                    .child(c.clone()),
            );
        }
        grid = grid.child(head);
        for row in &table.rows {
            let mut r = h_flex();
            for (j, cell) in row.iter().enumerate().take(n) {
                r = r.child(
                    div()
                        .w(width(j))
                        .px(SPACE_3)
                        .py(SPACE_1)
                        .truncate()
                        .text_size(FONT_SMALL)
                        .when(cell.value.is_some(), |d| d.text_right())
                        .when(cell.flag, |d| {
                            d.text_color(t_.danger).font_weight(FontWeight::SEMIBOLD)
                        })
                        .child(cell.text.clone()),
                );
            }
            grid = grid.child(r);
        }
        let copy = table.clone();
        v_flex()
            .gap(SPACE_2)
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .text_size(FONT_BODY)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(table.title.clone()),
                    )
                    .child(
                        Button::new(("pls-copy-table", k))
                            .xsmall()
                            .ghost()
                            .icon(Lucide::Copy)
                            .label(t("Copy table"))
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| this.copy_table(&copy, cx));
                            }),
                    ),
            )
            .child(
                div()
                    .id(("pls-table-scroll", k))
                    .overflow_x_scroll()
                    .child(grid),
            )
            .when_some(table.note.clone(), |d, note| {
                d.child(
                    div()
                        .text_size(FONT_CAPTION)
                        .text_color(t_.fg_dim)
                        .child(note),
                )
            })
    }
}

impl PlsView {
    fn resize_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((y0, h0)) = self.resizing else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.resize_end(cx);
            return;
        }
        let canvas = f32::from(self.bounds.get().size.height);
        let max = (self.results_h + canvas - MIN_CANVAS).max(MIN_RESULTS_H);
        let h = (h0 + y0 - f32::from(ev.position.y)).clamp(MIN_RESULTS_H, max);
        if (h - self.results_h).abs() >= 0.5 {
            self.results_h = h;
            cx.notify();
        }
    }

    /// The resize ended: remember the height for the next model tabs.
    fn resize_end(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_none() {
            return;
        }
        let h = self.results_h.round() as u32;
        cx.background_spawn(async move {
            let mut s = kantor_core::settings::Settings::load();
            if s.sheets.pls_results_height != Some(h) {
                s.sheets.pls_results_height = Some(h);
                if let Err(e) = s.save() {
                    log::error!("saving settings: {e:#}");
                }
            }
        })
        .detach();
        cx.notify();
    }
}

impl Render for PlsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t_ = tokens(cx);
        // A fit asked for once the canvas has a size.
        let b = self.bounds.get();
        if self.fit_pending.get() && b.size.width > px(1.) && b.size.height > px(1.) {
            self.fit_pending.set(false);
            self.fit_now(b.size);
            self.fitted.set(Some(b.size));
        }
        let status = (!self.status.is_empty()).then(|| {
            div()
                .flex_none()
                .px(SPACE_4)
                .py(SPACE_2)
                .text_size(FONT_SMALL)
                .text_color(if self.status_error {
                    t_.danger
                } else {
                    t_.fg_muted
                })
                .border_b_1()
                .border_color(t_.border)
                .child(self.status.clone())
        });
        v_flex()
            .id("pls-view")
            .size_full()
            .bg(t_.bg)
            .text_color(t_.fg)
            .text_size(FONT_BODY)
            .on_mouse_move(cx.listener(Self::resize_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.resize_end(cx)),
            )
            .child(self.render_bar(cx))
            .children(status)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_indicators(cx))
                    .child(self.render_canvas(cx))
                    .child(self.render_side(cx)),
            )
            .children(self.render_results(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn data_as_csv() {
        let d = kantor_pls::Data::new(
            vec!["a".into(), "b,c".into()],
            vec![vec![1.0, f64::NAN], vec![2.5, 3.0]],
        )
        .unwrap();
        assert_eq!(data_csv(&d), "a,\"b,c\"\n1,\n2.5,3\n");
    }
}
