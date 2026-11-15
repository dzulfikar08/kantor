//! Formatted text edited in a plain text box, for shapes' text: the box
//! holds the text (paragraphs as lines), the formatted paragraphs follow
//! each change through Slides' `text_ops::replace_changed` (the unchanged
//! start and end keep their runs, new text takes the formatting where it
//! is typed), and the toolbar formats the box's selection. The shape draws
//! the formatted paragraphs as you type.
//!
//! The box is a `Textarea` rendered from the Sheets root, never inside the
//! grid (whose key handler would take the typing).

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
};
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n::{self, tk};
use kantor_ui::metrics::*;
use slides_engine::{Align, LINE_BREAK, Position, Rgb, RunProps, Selection, TextPara, text_ops};

/// Fonts offered (Excel's usual ones).
pub(crate) const FONTS: [&str; 10] = [
    "Aptos",
    "Arial",
    "Calibri",
    "Cambria",
    "Courier New",
    "Georgia",
    "Segoe UI",
    "Tahoma",
    "Times New Roman",
    "Verdana",
];

/// Sizes offered, in points.
pub(crate) const SIZES: [u32; 14] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];

/// The text as the box shows it: paragraphs as lines, line breaks too.
pub(crate) fn plain(paras: &[TextPara]) -> String {
    paras
        .iter()
        .map(|p| p.text.replace(LINE_BREAK, "\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The paragraph position of byte `b` of [`plain`]'s text.
pub(crate) fn position(paras: &[TextPara], mut b: usize) -> Position {
    for (i, p) in paras.iter().enumerate() {
        let mut shown = 0;
        for (off, ch) in p.text.char_indices() {
            if b <= shown {
                return Position::new(i, off);
            }
            shown += if ch == LINE_BREAK { 1 } else { ch.len_utf8() };
        }
        if b <= shown || i + 1 == paras.len() {
            return Position::new(i, p.text.len());
        }
        b -= shown + 1;
    }
    Position::default()
}

/// The paragraphs a format applies to for the box's selection `range`:
/// the selection, or all the text when nothing is selected (as Excel does
/// with the shape itself selected).
pub(crate) fn selection(paras: &[TextPara], range: Range<usize>) -> Selection {
    if range.is_empty() {
        return Selection {
            anchor: Position::new(0, 0),
            head: text_ops::end(paras),
        };
    }
    Selection {
        anchor: position(paras, range.start),
        head: position(paras, range.end),
    }
}

/// A formatting command of the toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RichOp {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Font(&'static str),
    /// Points.
    Size(u32),
    /// `None`: Automatic (the theme's text colour).
    Color(Option<u32>),
    Align(Align),
}

/// Apply `op` to the text `sel` covers.
pub(crate) fn apply(paras: &mut [TextPara], sel: Selection, op: RichOp) {
    if let RichOp::Align(a) = op {
        let (s, e) = sel.ordered();
        for p in paras.iter_mut().take(e.para + 1).skip(s.para) {
            p.props.align = Some(a);
        }
        return;
    }
    text_ops::format(paras, sel, &|p: &mut RunProps| match op {
        RichOp::Bold(on) => slides_engine::set_toggle(p, slides_engine::Toggle::Bold, on),
        RichOp::Italic(on) => slides_engine::set_toggle(p, slides_engine::Toggle::Italic, on),
        RichOp::Underline(on) => slides_engine::set_toggle(p, slides_engine::Toggle::Underline, on),
        RichOp::Font(f) => slides_engine::sheet_shape::set_typeface(p, Some(f)),
        RichOp::Size(pt) => p.size = Some(pt * 100),
        RichOp::Color(Some(c)) => slides_engine::sheet_shape::set_color(
            p,
            Rgb([(c >> 16) as u8, (c >> 8) as u8, c as u8]),
        ),
        RichOp::Color(None) => slides_engine::sheet_shape::clear_color(p),
        RichOp::Align(_) => {}
    });
}

/// What the toolbar shows: the formatting at the selection's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RichState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub font: Option<String>,
    /// Points.
    pub size: Option<u32>,
    pub align: Align,
}

impl RichState {
    /// The state at the start of `sel`.
    pub(crate) fn at(paras: &[TextPara], sel: Selection) -> Self {
        let (s, _) = sel.ordered();
        let s = text_ops::clamp(paras, s);
        let Some(p) = paras.get(s.para) else {
            return Self::default();
        };
        // The character after the caret when at the start.
        let props = p.props_at(s.offset.max(1).min(p.text.len().max(1)));
        Self {
            bold: props.bold == Some(true),
            italic: props.italic == Some(true),
            underline: props.underline == Some(true) || props.shown_underline,
            font: props.shown_font.clone(),
            size: props.size.map(|s| (s + 50) / 100),
            align: p.props.align.unwrap_or_default(),
        }
    }
}

/// What the toolbar does with a command.
pub(crate) type OnRichOp = Rc<dyn Fn(RichOp, &mut Window, &mut App)>;

/// The toolbar over the text box: B, I, U, font, size, colour, alignment.
pub(crate) fn toolbar(id: &'static str, state: &RichState, on_op: OnRichOp, cx: &App) -> Div {
    let toggle = |n: usize, icon: Lucide, label: &'static str, on: bool, op: RichOp| {
        let f = on_op.clone();
        icon_button((id, n), icon, on, cx)
            .tip(i18n::t(label), None)
            .on_click(move |_, w, cx| f(op, w, cx))
    };
    let f = on_op.clone();
    let current_font = state.font.clone();
    let font = Button::new((id, 10usize))
        .ghost()
        .xsmall()
        .label(
            state
                .font
                .clone()
                .unwrap_or_else(|| i18n::t("Body font").to_owned()),
        )
        .dropdown_caret(true)
        .dropdown_menu(move |mut m, _, _| {
            for face in FONTS {
                let f = f.clone();
                m = m.item(
                    PopupMenuItem::new(face)
                        .checked(current_font.as_deref() == Some(face))
                        .on_click(move |_, w, cx| f(RichOp::Font(face), w, cx)),
                );
            }
            m
        });
    let f = on_op.clone();
    let current_size = state.size;
    let size = Button::new((id, 11usize))
        .ghost()
        .xsmall()
        .label(current_size.map_or_else(|| "11".to_owned(), |s| s.to_string()))
        .dropdown_caret(true)
        .dropdown_menu(move |mut m, _, _| {
            for pt in SIZES {
                let f = f.clone();
                m = m.item(
                    PopupMenuItem::new(pt.to_string())
                        .checked(current_size == Some(pt))
                        .on_click(move |_, w, cx| f(RichOp::Size(pt), w, cx)),
                );
            }
            m
        });
    let f = on_op.clone();
    let colour = crate::toolbar::swatch_popover(
        id_colour(id),
        Lucide::Baseline,
        tk("Text Colour"),
        Some(tk("Automatic")),
        None,
        Rc::new(move |c, w, cx| f(RichOp::Color(c), w, cx)),
        cx,
    );
    h_flex()
        .gap(SPACE_1)
        .flex_wrap()
        .child(toggle(
            0,
            Lucide::Bold,
            tk("Bold"),
            state.bold,
            RichOp::Bold(!state.bold),
        ))
        .child(toggle(
            1,
            Lucide::Italic,
            tk("Italic"),
            state.italic,
            RichOp::Italic(!state.italic),
        ))
        .child(toggle(
            2,
            Lucide::Underline,
            tk("Underline"),
            state.underline,
            RichOp::Underline(!state.underline),
        ))
        .child(font)
        .child(size)
        .child(colour)
        .child(toggle(
            3,
            Lucide::TextAlignStart,
            tk("Align Left"),
            state.align == Align::Left,
            RichOp::Align(Align::Left),
        ))
        .child(toggle(
            4,
            Lucide::TextAlignCenter,
            tk("Center"),
            state.align == Align::Center,
            RichOp::Align(Align::Center),
        ))
        .child(toggle(
            5,
            Lucide::TextAlignEnd,
            tk("Align Right"),
            state.align == Align::Right,
            RichOp::Align(Align::Right),
        ))
}

fn id_colour(id: &'static str) -> &'static str {
    match id {
        "note-rich" => "note-rich-colour",
        _ => "shape-rich-colour",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(lines: &[&str]) -> Vec<TextPara> {
        lines.iter().map(|l| TextPara::new(*l)).collect()
    }

    #[std::prelude::v1::test]
    fn box_offsets_map_to_paragraphs() {
        let mut ps = paras(&["ab", "", "cd"]);
        ps[2].text = format!("c{LINE_BREAK}d");
        ps[2].runs[0].len = ps[2].text.len();
        assert_eq!(plain(&ps), "ab\n\nc\nd");
        assert_eq!(position(&ps, 0), Position::new(0, 0));
        assert_eq!(position(&ps, 2), Position::new(0, 2));
        assert_eq!(position(&ps, 3), Position::new(1, 0));
        assert_eq!(position(&ps, 4), Position::new(2, 0));
        // Past the line break (one byte shown, three stored).
        assert_eq!(
            position(&ps, 6),
            Position::new(2, 1 + LINE_BREAK.len_utf8())
        );
        assert_eq!(position(&ps, 99), Position::new(2, ps[2].text.len()));
    }

    #[std::prelude::v1::test]
    fn formatting_applies_to_the_selection_and_keeps_through_typing() {
        let mut ps = paras(&["Hello world"]);
        let sel = selection(&ps, 0..5);
        apply(&mut ps, sel, RichOp::Bold(true));
        let sel = selection(&ps, 6..11);
        apply(&mut ps, sel, RichOp::Color(Some(0xFF0000)));
        assert_eq!(ps[0].runs.len(), 3);
        // Typing at the end keeps the bold start and the red end.
        text_ops::replace_changed(&mut ps, "Hello world!");
        assert_eq!(ps[0].runs[0].props.bold, Some(true));
        assert_eq!(
            ps[0].runs.last().unwrap().props.color,
            Some(Rgb([255, 0, 0]))
        );
        assert_eq!(ps[0].text, "Hello world!");
        assert!(RichState::at(&ps, selection(&ps, 0..2)).bold);
        assert!(!RichState::at(&ps, selection(&ps, 7..8)).bold);
        // No selection: everything.
        let sel = selection(&ps, 3..3);
        apply(&mut ps, sel, RichOp::Size(14));
        assert!(ps[0].runs.iter().all(|r| r.props.size == Some(1400)));
        let sel = selection(&ps, 0..0);
        apply(&mut ps, sel, RichOp::Align(Align::Center));
        assert_eq!(ps[0].props.align, Some(Align::Center));
    }
}
