//! The Find & Replace bar shared by Docs, Sheets and Slides: a compact
//! panel at the top right of the document area, like VS Code's.
//!
//! The bar only collects the query and options and reports what the user
//! asked for as [`FindBarEvent`]s; the app searches its own content (with
//! [`kantor_core::find`]) and reports back the match count with
//! [`FindBar::set_result`].
//!
//! Keys inside the bar: Enter / Shift+Enter (and F3 / Shift+F3) go to the
//! next / previous match, Enter in the replace field replaces, Escape
//! closes, Tab moves between the two fields. Apps bind Ctrl+F / Ctrl+H to
//! [`OpenFind`] / [`OpenReplace`] and F3 / Shift+F3 to [`FindNext`] /
//! [`FindPrev`] in their own key context (see [`bind_app_keys`]).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::find::FindOptions;

use crate::components::{IconButtonExt as _, icon_button};
use crate::i18n::{self, tf};
use crate::metrics::*;
use crate::tokens::{Elevation, tokens};

/// Key context of the bar.
pub const KEY_CONTEXT: &str = "FindBar";

actions!(
    find,
    [
        /// Open the bar (Ctrl+F), seeded with the selected text.
        OpenFind,
        /// Open the bar with the replace row (Ctrl+H).
        OpenReplace,
        FindNext,
        FindPrev,
        CloseFind,
        /// Tab inside the bar: the other field.
        SwitchField,
    ]
);

/// Bind the bar's own keys. Call once at startup.
pub fn init(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("f3", FindNext, c),
        KeyBinding::new("shift-f3", FindPrev, c),
        KeyBinding::new("escape", CloseFind, c),
        KeyBinding::new("tab", SwitchField, c),
        KeyBinding::new("shift-tab", SwitchField, c),
    ]);
}

/// Bind Ctrl+F, Ctrl+H (and Super variants) and F3 / Shift+F3 in an app's
/// key `context`.
pub fn bind_app_keys(context: &'static str, cx: &mut App) {
    let c = Some(context);
    cx.bind_keys([
        KeyBinding::new(crate::keys::primary("ctrl-f"), OpenFind, c),
        KeyBinding::new("super-f", OpenFind, c),
        KeyBinding::new(crate::keys::primary("ctrl-h"), OpenReplace, c),
        KeyBinding::new("super-h", OpenReplace, c),
        KeyBinding::new("f3", FindNext, c),
        KeyBinding::new("shift-f3", FindPrev, c),
    ]);
}

/// What the user asked the bar for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindBarEvent {
    /// The query or an option changed: search again.
    Search,
    Next,
    Prev,
    Replace,
    ReplaceAll,
    /// The bar closed; the app takes focus back.
    Closed,
}

impl EventEmitter<FindBarEvent> for FindBar {}

/// The Find & Replace bar. Render it as a child of a `relative()` document
/// area while [`FindBar::is_open`]; it places itself at the top right.
pub struct FindBar {
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    options: FindOptions,
    /// `Some` when the app can also search formulas (Sheets).
    formulas: Option<bool>,
    replace: bool,
    open: bool,
    current: Option<usize>,
    total: usize,
    _subscriptions: Vec<Subscription>,
}

impl FindBar {
    /// `formulas`: offer a "Look in formulas" toggle.
    pub fn new(formulas: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Find")));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Replace")));
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |_, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => cx.emit(FindBarEvent::Search),
                InputEvent::PressEnter { shift: true, .. } => cx.emit(FindBarEvent::Prev),
                InputEvent::PressEnter { .. } => cx.emit(FindBarEvent::Next),
                _ => {}
            }),
            cx.subscribe_in(&replacement, window, |_, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { secondary, .. } = ev {
                    cx.emit(if *secondary {
                        FindBarEvent::ReplaceAll
                    } else {
                        FindBarEvent::Replace
                    });
                }
            }),
        ];
        Self {
            query,
            replacement,
            options: FindOptions::default(),
            formulas: formulas.then_some(false),
            replace: false,
            open: false,
            current: None,
            total: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Open the bar (with the replace row when `replace`) and focus the
    /// query, selected so typing replaces it. A single-line `seed` (the
    /// selected text) becomes the query.
    pub fn show(
        &mut self,
        replace: bool,
        seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open = true;
        self.replace = replace;
        let seed = seed.filter(|s| !s.is_empty() && !s.contains(['\n', '\r', '\u{2028}']));
        if let Some(seed) = seed
            && seed != self.query.read(cx).value().as_ref()
        {
            self.query.update(cx, |q, cx| q.set_value(seed, window, cx));
        }
        self.query.update(cx, |q, cx| {
            q.focus(window, cx);
            q.select_all(window, cx);
        });
        // `set_value` doesn't emit `Change`: search for what's there now.
        cx.emit(FindBarEvent::Search);
        cx.notify();
    }

    /// Close the bar; the app gets [`FindBarEvent::Closed`] to take focus back.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.emit(FindBarEvent::Closed);
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The query, if the bar is open and it isn't empty.
    pub fn active_query(&self, cx: &App) -> Option<String> {
        let q = self.query(cx);
        (self.open && !q.is_empty()).then_some(q)
    }

    pub fn query(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    pub fn replacement(&self, cx: &App) -> String {
        self.replacement.read(cx).value().to_string()
    }

    pub fn options(&self) -> FindOptions {
        self.options
    }

    /// Sheets: search formulas as typed instead of displayed values.
    pub fn look_in_formulas(&self) -> bool {
        self.formulas.unwrap_or(false)
    }

    /// Show `current` (0-based) of `total` matches.
    pub fn set_result(&mut self, current: Option<usize>, total: usize, cx: &mut Context<Self>) {
        if (self.current, self.total) != (current, total) {
            self.current = current;
            self.total = total;
            cx.notify();
        }
    }

    fn toggle_option(&mut self, f: impl FnOnce(&mut Self), cx: &mut Context<Self>) {
        f(self);
        cx.emit(FindBarEvent::Search);
        cx.notify();
    }

    fn switch_field(&mut self, _: &SwitchField, window: &mut Window, cx: &mut Context<Self>) {
        if !self.replace {
            return;
        }
        let to = if self.query.read(cx).focus_handle(cx).is_focused(window) {
            &self.replacement
        } else {
            &self.query
        };
        to.update(cx, |i, cx| i.focus(window, cx));
    }

    /// An icon button; `tip` is the tooltip label and its shortcut.
    fn button(
        id: &'static str,
        icon: Lucide,
        active: bool,
        (tip, keys): (&str, Option<&str>),
        run: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        icon_button(id, icon, active, cx)
            .tip(tip, keys)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| run(this, window, cx));
            })
    }
}

/// "3 of 12", "12 matches", "No results" in the interface language.
fn count_label(current: Option<usize>, total: usize) -> String {
    match (current, total) {
        (_, 0) => i18n::t("No results").to_owned(),
        (Some(i), n) if i < n => tf("{} of {}", &[&(i + 1), &n]),
        (_, 1) => i18n::t("1 match").to_owned(),
        (_, n) => tf("{} matches", &[&n]),
    }
}

impl Render for FindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let o = self.options;
        let label = if self.query.read(cx).value().is_empty() {
            String::new()
        } else {
            count_label(self.current, self.total)
        };
        let none = self.total == 0 && !label.is_empty();

        let mut find_row = h_flex()
            .gap(SPACE_1)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.query).small()),
            )
            .child(Self::button(
                "find-case",
                Lucide::CaseSensitive,
                o.match_case,
                (i18n::t("Match case"), None),
                |this, _, cx| this.toggle_option(|t| t.options.match_case ^= true, cx),
                cx,
            ))
            .child(Self::button(
                "find-word",
                Lucide::WholeWord,
                o.whole_word,
                (i18n::t("Whole word"), None),
                |this, _, cx| this.toggle_option(|t| t.options.whole_word ^= true, cx),
                cx,
            ));
        if let Some(on) = self.formulas {
            find_row = find_row.child(Self::button(
                "find-formulas",
                Lucide::SquareFunction,
                on,
                (i18n::t("Look in formulas"), None),
                |this, _, cx| {
                    this.toggle_option(
                        |t| {
                            if let Some(f) = &mut t.formulas {
                                *f ^= true;
                            }
                        },
                        cx,
                    )
                },
                cx,
            ));
        }
        let find_row = find_row
            .child(
                div()
                    .flex_none()
                    .w(FIND_COUNT_WIDTH)
                    .px(SPACE_2)
                    .text_size(FONT_SMALL)
                    .text_color(if none { t.danger } else { t.fg_muted })
                    .truncate()
                    .child(label),
            )
            .child(Self::button(
                "find-prev",
                Lucide::ChevronUp,
                false,
                (i18n::t("Previous match"), Some("Shift+Enter")),
                |_, _, cx| cx.emit(FindBarEvent::Prev),
                cx,
            ))
            .child(Self::button(
                "find-next",
                Lucide::ChevronDown,
                false,
                (i18n::t("Next match"), Some("Enter")),
                |_, _, cx| cx.emit(FindBarEvent::Next),
                cx,
            ))
            .child(Self::button(
                "find-replace-toggle",
                Lucide::Replace,
                self.replace,
                (i18n::t("Toggle replace"), Some("Ctrl+H")),
                |this, _, cx| {
                    this.replace ^= true;
                    cx.notify();
                },
                cx,
            ))
            .child(Self::button(
                "find-close",
                Lucide::X,
                false,
                (i18n::t("Close"), Some("Esc")),
                |this, _, cx| this.hide(cx),
                cx,
            ));

        let replace_row = self.replace.then(|| {
            h_flex()
                .gap(SPACE_1)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.replacement).small()),
                )
                .child(Self::button(
                    "find-replace-one",
                    Lucide::Replace,
                    false,
                    (i18n::t("Replace"), Some("Enter")),
                    |_, _, cx| cx.emit(FindBarEvent::Replace),
                    cx,
                ))
                .child(Self::button(
                    "find-replace-all",
                    Lucide::ReplaceAll,
                    false,
                    (i18n::t("Replace all"), Some("Ctrl+Enter")),
                    |_, _, cx| cx.emit(FindBarEvent::ReplaceAll),
                    cx,
                ))
        });

        v_flex()
            .id("find-bar")
            .key_context(KEY_CONTEXT)
            .absolute()
            .top(SPACE_4)
            .right(SPACE_7)
            .w(FIND_BAR_WIDTH)
            .occlude()
            .gap(SPACE_2)
            .p(SPACE_2)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_BODY)
            .text_color(t.fg)
            .cursor_default()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|_, _: &FindNext, _, cx| cx.emit(FindBarEvent::Next)))
            .on_action(cx.listener(|_, _: &FindPrev, _, cx| cx.emit(FindBarEvent::Prev)))
            .on_action(cx.listener(|this, _: &CloseFind, _, cx| this.hide(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.hide(cx)))
            .on_action(cx.listener(Self::switch_field))
            .child(find_row)
            .children(replace_row)
    }
}
