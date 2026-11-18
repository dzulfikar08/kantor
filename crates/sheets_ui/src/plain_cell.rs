//! Plain grid cells drawn without an element tree of their own.
//!
//! Most visible cells are plain: a fill, gridlines and one line of text (no
//! merge, borders, overlays, conditional icons or spilled text). Built from
//! divs, each took five layout nodes (the table's cell box, the cell, the
//! text box and the text, plus a wrapper), laid out on their own every
//! frame, and that was most of what a scrolled frame cost. [`RowCells`]
//! draws a row's plain cells directly and lays out only the others;
//! [`PlainCell`] is one plain cell as an element, where a cell needs one.
//!
//! The text is placed with the device-pixel rounding GPUI's layout applies
//! to the div version ([`crate::look::Content::apply`]), so both paint the
//! same pixels.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::GRID_LINE;

/// A cell's text as [`crate::look::Content::plain`] lays it out: one line
/// in a box padded by `pad` left and right, at the top (padded by `top`),
/// the bottom (lifted by `bottom`) or centred when both are `None`.
#[derive(Default)]
pub(crate) struct CellText {
    pub text: SharedString,
    /// The text box's style (size, line height, font, colour, alignment,
    /// truncation), refined over the inherited one.
    pub style: TextStyleRefinement,
    pub pad: Pixels,
    pub top: Option<Pixels>,
    pub bottom: Option<Pixels>,
}

/// A press or a move over a plain cell, for the grid's own handling
/// (selecting, extending the selection by dragging).
pub(crate) enum CellMouse<'a> {
    Down(&'a MouseDownEvent),
    Move(&'a MouseMoveEvent),
}

/// The grid's handler for [`CellMouse`] events, by `(row_ix, col_ix)`.
pub(crate) type CellMouseHandler = Rc<dyn Fn((usize, usize), CellMouse<'_>, &mut Window, &mut App)>;

/// The press of a click in progress and the cell it went down on, shared
/// by a table's body cells (a div keeps this in its element state; plain
/// cells have none, and a cell may stop being plain between the press and
/// the release, e.g. when the press selects it).
pub(crate) type PendingClick = Rc<RefCell<Option<((usize, usize), MouseDownEvent)>>>;

/// A table's handler for clicks on cell `(row_ix, col_ix)`.
pub(crate) type CellClick = Rc<dyn Fn(&ClickEvent, (usize, usize), &mut Window, &mut App)>;
/// A table's handler for right presses on cell `(row_ix, col_ix)`.
pub(crate) type CellRightDown = Rc<dyn Fn(&MouseDownEvent, (usize, usize), &mut Window, &mut App)>;
/// A row's cells for the shown columns, given the columns reported visible.
type RowItems = Box<dyn Fn(Range<usize>, Range<usize>, &mut Window, &mut App) -> Vec<RowItem>>;

/// What a table does with presses on its body cells.
#[derive(Clone)]
pub(crate) struct CellHandlers {
    pub mouse: Option<CellMouseHandler>,
    pub pending: PendingClick,
    pub click: CellClick,
    pub right_down: CellRightDown,
}

impl CellHandlers {
    /// Listen over `hitbox` for presses, moves and clicks on the cell
    /// `locate` finds under the pointer.
    fn listen(
        &self,
        hitbox: &Hitbox,
        locate: impl Fn(Point<Pixels>) -> Option<(usize, usize)> + Clone + 'static,
        window: &mut Window,
    ) {
        // A press runs the grid's handler before the table's (a div's
        // child before its parent).
        window.on_mouse_event({
            let (this, hitbox, locate) = (self.clone(), hitbox.clone(), locate.clone());
            move |e: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let Some(key) = locate(e.position) else {
                    return;
                };
                match e.button {
                    MouseButton::Left => {
                        if let Some(f) = &this.mouse {
                            f(key, CellMouse::Down(e), window, cx);
                        }
                        *this.pending.borrow_mut() = Some((key, e.clone()));
                    }
                    MouseButton::Right => (this.right_down)(e, key, window, cx),
                    _ => {}
                }
            }
        });
        if let Some(mouse) = self.mouse.clone() {
            let (hitbox, locate) = (hitbox.clone(), locate.clone());
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && hitbox.is_hovered(window)
                    && let Some(key) = locate(e.position)
                {
                    mouse(key, CellMouse::Move(e), window, cx);
                }
            });
        }
        let (this, hitbox) = (self.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble
                && e.button == MouseButton::Left
                && hitbox.is_hovered(window)
                && let Some(key) = locate(e.position)
            {
                this.release(key, e, window, cx);
            }
        });
    }

    /// A left release over cell `key`: a click if the press went down there.
    pub(crate) fn release(
        &self,
        key: (usize, usize),
        up: &MouseUpEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        let down = {
            let mut pending = self.pending.borrow_mut();
            match pending.take() {
                Some((k, down)) if k == key => Some(down),
                _ => None,
            }
        };
        if let Some(down) = down {
            let click = ClickEvent::Mouse(MouseClickEvent {
                down,
                up: up.clone(),
            });
            (self.click)(&click, key, window, cx);
        }
    }
}

/// One plain cell: its text, fill and gridlines.
pub(crate) struct PlainCell {
    text: CellText,
    fill: Option<Hsla>,
    /// Gridlines along the right and bottom edges.
    grid: (bool, bool),
    grid_color: Hsla,
    /// As an element: its width (its column's), and the handlers and cell
    /// for its presses.
    width: Option<Pixels>,
    handlers: Option<(CellHandlers, (usize, usize))>,
}

/// A plain cell's shaped text: its lines, box, line height and alignment.
pub(crate) struct ShapedText {
    lines: Vec<WrappedLine>,
    text_box: Bounds<Pixels>,
    line_height: Pixels,
    align: TextAlign,
}

/// `v` in device pixels, rounded as GPUI's layout rounds lengths.
fn dp(v: Pixels, scale: f32) -> f32 {
    let v = f32::from(v) * scale;
    (v.abs() - 0.5).ceil().copysign(v)
}

/// A border width in device pixels, as GPUI's layout rounds it.
fn stroke_dp(v: Pixels, scale: f32) -> f32 {
    if v == px(0.) {
        0.
    } else {
        dp(v.max(px(0.)), scale).max(1.)
    }
}

impl PlainCell {
    pub(crate) fn new(
        text: CellText,
        fill: Option<Hsla>,
        grid: (bool, bool),
        grid_color: Hsla,
    ) -> Self {
        Self {
            text,
            fill,
            grid,
            grid_color,
            width: None,
            handlers: None,
        }
    }

    /// As an element: as wide as `width`, and handling presses on cell `key`.
    pub(crate) fn element(
        mut self,
        width: Option<Pixels>,
        handlers: CellHandlers,
        key: (usize, usize),
    ) -> Self {
        self.width = width;
        self.handlers = Some((handlers, key));
        self
    }

    fn border_widths(&self) -> Edges<Pixels> {
        Edges {
            right: if self.grid.0 { GRID_LINE } else { px(0.) },
            bottom: if self.grid.1 { GRID_LINE } else { px(0.) },
            ..Default::default()
        }
    }

    /// Shape the text and place its box in `bounds` (the cell), as the
    /// div version's flex layout would.
    pub(crate) fn shape(
        &self,
        bounds: Bounds<Pixels>,
        raw_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<ShapedText> {
        let t = &self.text;
        if t.text.is_empty() {
            return None;
        }
        let scale = window.scale_factor();
        let rem = window.rem_size();
        let mut style = window.text_style();
        style.refine(&t.style);
        let font_size = style.font_size.to_pixels(rem);
        let line_height = window.pixel_snap(style.line_height.to_pixels(font_size.into(), rem));
        // The text node's height, rounded up to device pixels as GPUI does
        // with measured sizes.
        let line_dp = (f32::from(line_height) * scale).ceil();

        // The text box relative to the cell, in device pixels, rounded as
        // the layout rounds it: the table lays each cell out as a root of
        // its own, so positions inside it are rounded relative to its
        // origin. The height is the one the cell was laid out in
        // (`raw_height`, which may be off whole device pixels by float
        // error: centred text rounds with it).
        let round = |v: f32| (v.abs() - 0.5).ceil().copysign(v);
        let w = (f32::from(bounds.size.width) * scale).round();
        let h = f32::from(raw_height) * scale;
        let pad = dp(t.pad, scale);
        let border = self.border_widths();
        let content_h = h - stroke_dp(border.bottom, scale);
        let x0 = pad;
        let x1 = w - stroke_dp(border.right, scale) - pad;
        let y0 = round(match (t.top, t.bottom) {
            (Some(pt), _) => dp(pt, scale),
            (None, Some(pb)) => content_h - dp(pb, scale) - line_dp,
            (None, None) => (content_h - line_dp) / 2.,
        });
        let text_box = Bounds::new(
            bounds.origin + point(px(x0 / scale), px(y0 / scale)),
            size(px((x1 - x0).max(0.) / scale), px(line_dp / scale)),
        );

        let runs = [style.to_run(t.text.len())];
        let system = window.text_system().clone();
        let mut lines = system
            .shape_text(t.text.clone(), font_size, &runs, None, None)
            .ok()?
            .into_vec();
        // Truncate with an ellipsis as the text element does, when the
        // shaped line doesn't fit.
        let width = text_box.size.width;
        if let Some(TextOverflow::Truncate(affix)) = style.text_overflow.clone()
            && lines.iter().any(|l| l.size(line_height).width > width)
        {
            let mut wrapper = cx.text_system().line_wrapper(style.font(), font_size);
            let (text, runs) =
                wrapper.truncate_line(t.text.clone(), width, &affix, &runs, TruncateFrom::End);
            lines = system
                .shape_text(text, font_size, &runs, None, None)
                .ok()?
                .into_vec();
        }
        Some(ShapedText {
            lines,
            text_box,
            line_height,
            align: style.text_align,
        })
    }

    /// Paint the fill, the text and the gridlines, in the order a div
    /// paints its background, its children and its border.
    pub(crate) fn paint(
        &self,
        bounds: Bounds<Pixels>,
        text: Option<&ShapedText>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(fill) = self.fill.filter(|f| !f.is_transparent()) {
            let mut edge = fill;
            edge.a = 0.;
            window.paint_quad(quad(
                bounds,
                Corners::default(),
                fill,
                Edges::default(),
                edge,
                BorderStyle::default(),
            ));
        }
        if let Some(t) = text {
            window.with_content_mask(Some(ContentMask { bounds: t.text_box }), |window| {
                let mut origin = t.text_box.origin;
                for line in &t.lines {
                    let _ =
                        line.paint(origin, t.line_height, t.align, Some(t.text_box), window, cx);
                    origin.y += line.size(t.line_height).height;
                }
            });
        }
        if (self.grid.0 || self.grid.1) && !self.grid_color.is_transparent() {
            let mut background = self.grid_color;
            background.a = 0.;
            window.paint_quad(quad(
                bounds,
                Corners::default(),
                background,
                self.border_widths(),
                self.grid_color,
                BorderStyle::default(),
            ));
        }
    }
}

impl IntoElement for PlainCell {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PlainCell {
    type RequestLayoutState = ();
    type PrepaintState = (Hitbox, Option<ShapedText>);

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
        let style = Style {
            size: size(
                self.width.map_or(relative(1.).into(), |w| w.into()),
                relative(1.).into(),
            ),
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> (Hitbox, Option<ShapedText>) {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        (hitbox, self.shape(bounds, bounds.size.height, window, cx))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        (hitbox, text): &mut (Hitbox, Option<ShapedText>),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((handlers, key)) = self.handlers.take() {
            handlers.listen(hitbox, move |_| Some(key), window);
        }
        PlainCell::paint(self, bounds, text.as_ref(), window, cx);
    }
}

/// A cell of a [`RowCells`] row.
// Not boxed: most cells are plain, and a box each would be an allocation
// per cell and frame.
#[allow(clippy::large_enum_variant)]
pub(crate) enum RowItem {
    Plain(PlainCell),
    Element(AnyElement),
}

/// The visible cells of row `row_ix` in a table's scrolling columns,
/// placed as [`gpui_kit::base::h_virtual_list`] places them (the table's
/// horizontal scroll offset, column widths from `widths`): plain cells are
/// drawn directly, with one hitbox for the row, and only the other cells
/// are laid out, each on its own.
pub(crate) struct RowCells {
    row_ix: usize,
    /// Widths of the scrolling columns; the first is column `first_col`.
    widths: Rc<Vec<Pixels>>,
    /// Where each starts.
    origins: Rc<Vec<Pixels>>,
    first_col: usize,
    /// The table's horizontal scroll (read at prepaint, after the header
    /// row has applied this frame's scroll requests).
    scroll: ScrollHandle,
    offset: Point<Pixels>,
    /// The cells of the shown columns, given the columns reported as
    /// visible (two more).
    items: RowItems,
    handlers: CellHandlers,
}

pub(crate) struct RowState {
    hitbox: Hitbox,
    /// The drawn cells: plain ones with their bounds and text.
    plain: Vec<(usize, Bounds<Pixels>, PlainCell, Option<ShapedText>)>,
    /// The others, laid out, with their column.
    elements: Vec<(usize, AnyElement)>,
}

impl RowCells {
    pub(crate) fn new(
        row_ix: usize,
        (widths, origins): (Rc<Vec<Pixels>>, Rc<Vec<Pixels>>),
        first_col: usize,
        scroll: ScrollHandle,
        handlers: CellHandlers,
        items: impl Fn(Range<usize>, Range<usize>, &mut Window, &mut App) -> Vec<RowItem> + 'static,
    ) -> Self {
        Self {
            row_ix,
            widths,
            origins,
            first_col,
            scroll,
            offset: Point::default(),
            items: Box::new(items),
            handlers,
        }
    }

    /// The scrolling columns (indexes into `widths`) that
    /// [`gpui_kit::base::h_virtual_list`] reports (and draws) in `width`
    /// past the scroll offset: those inside and two more.
    pub(crate) fn visible(&self, width: Pixels) -> Range<usize> {
        let (o, w) = (&self.origins, &self.widths);
        let count = o.len().min(w.len());
        // Column ends grow with the index: the first one ending past
        // `edge` is a partition point.
        let ends_before = |edge: Pixels| {
            let (mut low, mut high) = (0, count);
            while low < high {
                let mid = low + (high - low) / 2;
                if o[mid] + w[mid] <= edge {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            low
        };
        let first = ends_before(-self.offset.x);
        let past_end = ends_before(-self.offset.x + width);
        let last = if past_end == count {
            count
        } else {
            (past_end + 2).min(count)
        };
        first..last.max(first)
    }
}

impl IntoElement for RowCells {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RowCells {
    type RequestLayoutState = ();
    type PrepaintState = RowState;

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
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> RowState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.offset = self.scroll.offset();
        let reported = self.visible(bounds.size.width);
        // The columns `h_virtual_list` would draw that show: past this
        // row's right edge, under the table's last (empty) column, the
        // cells still show up to where the row's box clips them.
        let clip = window.content_mask().bounds.right();
        let range = reported.start
            ..reported.start
                + self.origins[reported.clone()]
                    .iter()
                    .take_while(|&&x| bounds.left() + x + self.offset.x < clip)
                    .count();
        let items = (self.items)(range.clone(), reported, window, cx);
        let mut plain = Vec::new();
        let mut elements = Vec::new();
        let scale = window.scale_factor();
        // Lengths as the layout would give a cell laid out on its own, in
        // the height `h_virtual_list` gives it (its content box's, with the
        // same float error).
        let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
        let height = Bounds::from_corners(bounds.origin, bounds.bottom_right())
            .size
            .height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (item, ix) in items.into_iter().zip(range) {
                let origin = bounds.origin + point(self.origins[ix] + self.offset.x, self.offset.y);
                let col_ix = ix + self.first_col;
                match item {
                    RowItem::Plain(cell) => {
                        let cell_bounds =
                            Bounds::new(origin, size(snap(self.widths[ix]), snap(height)));
                        let text = cell.shape(cell_bounds, height, window, cx);
                        plain.push((col_ix, cell_bounds, cell, text));
                    }
                    RowItem::Element(mut el) => {
                        let space = size(
                            AvailableSpace::Definite(self.widths[ix]),
                            AvailableSpace::Definite(height),
                        );
                        el.layout_as_root(space, window, cx);
                        el.prepaint_at(origin, window, cx);
                        elements.push((col_ix, el));
                    }
                }
            }
        });
        RowState {
            hitbox,
            plain,
            elements,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        state: &mut RowState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Presses on the plain cells; the others have their own handlers
        // (and hitboxes over this one).
        let spans: Rc<Vec<(Pixels, Pixels, usize)>> = Rc::new(
            state
                .plain
                .iter()
                .map(|(col, b, _, _)| (b.left(), b.right(), *col))
                .collect(),
        );
        let row_ix = self.row_ix;
        self.handlers.listen(
            &state.hitbox,
            move |p| {
                spans
                    .iter()
                    .find(|(l, r, _)| p.x >= *l && p.x < *r)
                    .map(|&(_, _, col)| (row_ix, col))
            },
            window,
        );
        {
            // Column order, as the cells were laid out.
            let mut plain = state.plain.iter().peekable();
            let mut elements = state.elements.iter_mut().peekable();
            loop {
                let next_plain = plain.peek().map(|p| p.0);
                let next_el = elements.peek().map(|e| e.0);
                match (next_plain, next_el) {
                    (Some(p), Some(e)) if p < e => {
                        let (_, b, cell, text) = plain.next().unwrap();
                        cell.paint(*b, text.as_ref(), window, cx);
                    }
                    (Some(_), None) => {
                        let (_, b, cell, text) = plain.next().unwrap();
                        cell.paint(*b, text.as_ref(), window, cx);
                    }
                    (_, Some(_)) => elements.next().unwrap().1.paint(window, cx),
                    (None, None) => break,
                }
            }
        }
    }
}
