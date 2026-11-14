//! A popup list of text items, for drop-downs drawn over the grid (data
//! validation lists; usable for any pick-one list).
//!
//! The list holds no reference to its owner: it renders with a callback that
//! gets a [`PopupEvent`], and the owner applies it ([`PopupList::apply`]) or
//! acts on a pick or a dismissal. Keyboard: Up/Down, Home/End, Page Up/Down,
//! Enter or Tab picks, Escape dismisses, typing filters (Backspace
//! removes a character). The list takes keyboard focus while open; its
//! typed characters never reach the grid (which would start a cell edit),
//! and there is no text input inside (see the gpui-input-overlays note).

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{Icon, IconName, Sizable as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;

actions!(
    popup_list,
    [Up, Down, First, Last, PageUp, PageDown, Confirm, Dismiss]
);

/// The owner's callback.
type Handler = Rc<dyn Fn(PopupEvent, &mut Window, &mut App)>;

const CONTEXT: &str = "KantorPopupList";
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 8;
const ROW: Pixels = CONTROL_SM;

pub(crate) fn bind_keys(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("alt-up", Dismiss, ctx),
        KeyBinding::new("home", First, ctx),
        KeyBinding::new("end", Last, ctx),
        KeyBinding::new("pageup", PageUp, ctx),
        KeyBinding::new("pagedown", PageDown, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("tab", Confirm, ctx),
        KeyBinding::new("escape", Dismiss, ctx),
    ]);
}

/// What happened in the list; the owner decides what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PopupEvent {
    /// Move the highlight by this many rows (clamped).
    Move(isize),
    First,
    Last,
    /// Filter: a typed character, or Backspace (`None`).
    Type(Option<String>),
    /// An item was chosen (its index in the full list).
    Pick(usize),
    /// Enter / Tab on the highlighted item.
    Confirm,
    Dismiss,
}

/// The items, the filter and the highlight (no GPUI state).
struct Filtered {
    items: Arc<Vec<SharedString>>,
    filter: String,
    /// Indices of the items matching the filter.
    shown: Arc<Vec<usize>>,
    /// Highlighted row, in `shown`.
    highlight: usize,
}

impl Filtered {
    fn new(items: Vec<SharedString>, current: Option<usize>) -> Self {
        let n = items.len();
        Self {
            items: Arc::new(items),
            filter: String::new(),
            shown: Arc::new((0..n).collect()),
            highlight: current.filter(|&i| i < n).unwrap_or(0),
        }
    }

    fn highlighted(&self) -> Option<usize> {
        self.shown.get(self.highlight).copied()
    }

    fn apply(&mut self, ev: &PopupEvent) -> bool {
        let last = self.shown.len().saturating_sub(1);
        match ev {
            PopupEvent::Move(d) => {
                self.highlight = self.highlight.saturating_add_signed(*d).min(last);
            }
            PopupEvent::First => self.highlight = 0,
            PopupEvent::Last => self.highlight = last,
            PopupEvent::Type(Some(s)) => {
                self.filter.push_str(s);
                self.refilter();
            }
            PopupEvent::Type(None) => {
                self.filter.pop();
                self.refilter();
            }
            PopupEvent::Pick(_) | PopupEvent::Confirm | PopupEvent::Dismiss => return false,
        }
        true
    }

    /// Items containing the filter (case-insensitively); those starting with
    /// it come first.
    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        let (mut starts, mut contains) = (Vec::new(), Vec::new());
        for (i, item) in self.items.iter().enumerate() {
            let t = item.to_lowercase();
            if t.starts_with(&f) {
                starts.push(i);
            } else if t.contains(&f) {
                contains.push(i);
            }
        }
        starts.extend(contains);
        self.shown = Arc::new(starts);
        self.highlight = 0;
    }
}

pub(crate) struct PopupList {
    state: Filtered,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl PopupList {
    /// A list highlighting `current` (an index into `items`), if given.
    pub(crate) fn new(items: Vec<SharedString>, current: Option<usize>, cx: &mut App) -> Self {
        let list = Self {
            state: Filtered::new(items, current),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        list.reveal();
        list
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The highlighted item (index in the full list).
    pub(crate) fn highlighted(&self) -> Option<usize> {
        self.state.highlighted()
    }

    /// Apply a navigation or filter event. Picks and dismissals are the
    /// owner's to handle; this returns `false` for them.
    pub(crate) fn apply(&mut self, ev: &PopupEvent) -> bool {
        let applied = self.state.apply(ev);
        if applied {
            self.reveal();
        }
        applied
    }

    fn reveal(&self) {
        self.scroll
            .scroll_to_item(self.state.highlight, ScrollStrategy::Nearest);
    }

    /// The list, at least `min_width` wide. `on_event` gets every key and
    /// click.
    pub(crate) fn render(
        &self,
        min_width: Pixels,
        on_event: impl Fn(PopupEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> Stateful<Div> {
        let t = tokens(cx);
        let on_event: Handler = Rc::new(on_event);
        let send = |ev: PopupEvent| {
            let on_event = on_event.clone();
            move |window: &mut Window, cx: &mut App| on_event(ev.clone(), window, cx)
        };
        let items = self.state.items.clone();
        let shown = self.state.shown.clone();
        let highlight = self.state.highlight;
        let rows = shown.len().clamp(1, VISIBLE_ROWS);
        let pick = on_event.clone();
        let list = uniform_list("popup-list-items", shown.len(), move |range, _, _| {
            range
                .map(|row| {
                    let ix = shown[row];
                    let pick = pick.clone();
                    div()
                        .id(("popup-list-item", ix))
                        .w_full()
                        .h(ROW)
                        .px(SPACE_4)
                        .flex()
                        .items_center()
                        .rounded(RADIUS_SM)
                        .text_color(t.fg)
                        .when(row == highlight, |d| d.bg(t.selected))
                        .hover(|s| s.bg(t.hover))
                        .cursor_pointer()
                        .child(div().min_w_0().truncate().child(items[ix].clone()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            // Not a click on the cell under the list.
                            window.prevent_default();
                            cx.stop_propagation();
                            pick(PopupEvent::Pick(ix), window, cx);
                        })
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .h(ROW * rows as f32);
        let keys = on_event.clone();
        let (up, down, first, last, page_up, page_down, confirm, dismiss) = (
            send(PopupEvent::Move(-1)),
            send(PopupEvent::Move(1)),
            send(PopupEvent::First),
            send(PopupEvent::Last),
            send(PopupEvent::Move(-(VISIBLE_ROWS as isize))),
            send(PopupEvent::Move(VISIBLE_ROWS as isize)),
            send(PopupEvent::Confirm),
            send(PopupEvent::Dismiss),
        );
        v_flex()
            .id("popup-list")
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .on_action(move |_: &Up, w, cx| up(w, cx))
            .on_action(move |_: &Down, w, cx| down(w, cx))
            .on_action(move |_: &First, w, cx| first(w, cx))
            .on_action(move |_: &Last, w, cx| last(w, cx))
            .on_action(move |_: &PageUp, w, cx| page_up(w, cx))
            .on_action(move |_: &PageDown, w, cx| page_down(w, cx))
            .on_action(move |_: &Confirm, w, cx| confirm(w, cx))
            .on_action(move |_: &Dismiss, w, cx| dismiss(w, cx))
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                let ks = &e.keystroke;
                let m = &ks.modifiers;
                if m.control || m.platform || m.alt {
                    return;
                }
                let ev = match (ks.key.as_str(), ks.key_char.as_deref()) {
                    ("backspace", _) => PopupEvent::Type(None),
                    (_, Some(ch)) if !ch.is_empty() && !ch.chars().any(char::is_control) => {
                        PopupEvent::Type(Some(ch.to_owned()))
                    }
                    _ => return,
                };
                // Typing filters the list; it must not start a cell edit.
                cx.stop_propagation();
                keys(ev, window, cx);
            })
            .occlude()
            .min_w(min_width)
            .max_w(px(360.))
            .p(SPACE_2)
            .gap(SPACE_1)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_MD)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .when(!self.state.filter.is_empty(), |d| {
                d.child(
                    div()
                        .h(ROW)
                        .px(SPACE_4)
                        .flex()
                        .items_center()
                        .gap(SPACE_3)
                        .border_b_1()
                        .border_color(t.border)
                        .text_color(t.fg_muted)
                        .child(Icon::new(IconName::Search).with_size(ICON_SM))
                        .child(SharedString::from(self.state.filter.clone())),
                )
            })
            .when(self.state.shown.is_empty(), |d| {
                d.child(
                    div()
                        .h(ROW)
                        .px(SPACE_4)
                        .flex()
                        .items_center()
                        .text_color(t.fg_dim)
                        .child(kantor_ui::i18n::t("No matches")),
                )
            })
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn moves_and_filters() {
        let items = ["Open", "Pending", "Closed", "Reopened"];
        let mut l = Filtered::new(
            items.iter().map(|s| SharedString::from(*s)).collect(),
            Some(1),
        );
        assert_eq!(l.highlighted(), Some(1));
        l.apply(&PopupEvent::Move(10));
        assert_eq!(l.highlighted(), Some(3));
        l.apply(&PopupEvent::Move(-10));
        assert_eq!(l.highlighted(), Some(0));
        l.apply(&PopupEvent::Last);
        assert_eq!(l.highlighted(), Some(3));
        // "op": Open starts with it, Reopened contains it.
        l.apply(&PopupEvent::Type(Some("O".into())));
        l.apply(&PopupEvent::Type(Some("p".into())));
        assert_eq!(l.filter, "Op");
        assert_eq!(*l.shown, vec![0, 3]);
        assert_eq!(l.highlighted(), Some(0));
        l.apply(&PopupEvent::Move(1));
        assert_eq!(l.highlighted(), Some(3));
        l.apply(&PopupEvent::Type(Some("x".into())));
        assert_eq!(l.highlighted(), None);
        l.apply(&PopupEvent::Type(None));
        assert_eq!(l.shown.len(), 2);
        assert!(!l.apply(&PopupEvent::Confirm));
    }
}
