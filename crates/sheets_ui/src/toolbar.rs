//! The per-pane formatting toolbar above the formula bar: bold / italic /
//! underline / strikethrough toggles, horizontal alignment and the number
//! format menu, all showing the active cell's state.
//!
//! In a narrow pane (see `COMPACT_WIDTH`) only B / I / U stay as buttons; the
//! rest moves into a "…" overflow menu.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{CellStyle, HAlign, NumberFormat};

use crate::{Format, SheetsView};

/// A toolbar button: label (tooltip / menu text), shortcut hint and icon.
struct Tool {
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    icon: Lucide,
    format: Format,
}

const FONT_TOOLS: [Tool; 4] = [
    Tool {
        id: "fmt-bold",
        label: "Bold",
        keys: "Ctrl+B",
        icon: Lucide::Bold,
        format: Format::Bold,
    },
    Tool {
        id: "fmt-italic",
        label: "Italic",
        keys: "Ctrl+I",
        icon: Lucide::Italic,
        format: Format::Italic,
    },
    Tool {
        id: "fmt-underline",
        label: "Underline",
        keys: "Ctrl+U",
        icon: Lucide::Underline,
        format: Format::Underline,
    },
    Tool {
        id: "fmt-strike",
        label: "Strikethrough",
        keys: "",
        icon: Lucide::Strikethrough,
        format: Format::Strikethrough,
    },
];

const ALIGN_TOOLS: [Tool; 3] = [
    Tool {
        id: "fmt-align-left",
        label: "Align Left",
        keys: "",
        icon: Lucide::TextAlignStart,
        format: Format::Align(HAlign::Left),
    },
    Tool {
        id: "fmt-align-center",
        label: "Align Center",
        keys: "",
        icon: Lucide::TextAlignCenter,
        format: Format::Align(HAlign::Center),
    },
    Tool {
        id: "fmt-align-right",
        label: "Align Right",
        keys: "",
        icon: Lucide::TextAlignEnd,
        format: Format::Align(HAlign::Right),
    },
];

/// Number of font tools kept as buttons in a narrow pane (B, I, U).
const COMPACT_FONT_TOOLS: usize = 3;

fn is_on(format: Format, style: &CellStyle) -> bool {
    match format {
        Format::Bold => style.bold,
        Format::Italic => style.italic,
        Format::Underline => style.underline,
        Format::Strikethrough => style.strikethrough,
        Format::Align(a) => style.align == a,
        Format::Number(f) => style.number_format() == f,
    }
}

fn apply(view: &WeakEntity<SheetsView>, format: Format, window: &mut Window, cx: &mut App) {
    let _ = view.update(cx, |view, cx| view.format_doc(format, window, cx));
}

fn tool_button(tool: &Tool, style: &CellStyle, cx: &mut Context<SheetsView>) -> Button {
    let view = cx.entity().downgrade();
    let format = tool.format;
    icon_button(tool.id, tool.icon, is_on(format, style), cx)
        .tip(tool.label, Some(tool.keys))
        .on_click(move |_, window, cx| apply(&view, format, window, cx))
}

fn separator(cx: &App) -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .h(ICON_LG)
        .mx(SPACE_2)
        .bg(tokens(cx).border)
}

/// Menu entries that set a number format, with the current one checked.
fn number_items(
    mut menu: PopupMenu,
    current: NumberFormat,
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    for f in NumberFormat::ALL {
        let view = view.clone();
        menu = menu.item(
            PopupMenuItem::new(f.label())
                .checked(current == f)
                .on_click(move |_, window, cx| apply(&view, Format::Number(f), window, cx)),
        );
    }
    menu
}

fn number_label(current: NumberFormat) -> SharedString {
    current.label().into()
}

/// The toolbar row for the active cell's `style`.
pub(crate) fn render(style: &CellStyle, compact: bool, cx: &mut Context<SheetsView>) -> Div {
    let t = tokens(cx);
    let current = style.number_format();
    let view = cx.entity().downgrade();
    let font_count = if compact {
        COMPACT_FONT_TOOLS
    } else {
        FONT_TOOLS.len()
    };

    let mut row = h_flex()
        .flex_none()
        .h(CONTROL_LG)
        .gap(SPACE_1)
        .px(SPACE_4)
        .bg(t.chrome)
        .border_b_1()
        .border_color(t.border)
        .children(
            FONT_TOOLS[..font_count]
                .iter()
                .map(|tool| tool_button(tool, style, cx)),
        );

    if compact {
        // Everything else goes into one overflow menu.
        let style = style.clone();
        row = row.child(separator(cx)).child(
            Button::new("fmt-more")
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .tooltip("More formatting")
                .dropdown_menu(move |mut menu, _, _| {
                    for tool in FONT_TOOLS[COMPACT_FONT_TOOLS..]
                        .iter()
                        .chain(ALIGN_TOOLS.iter())
                    {
                        let view = view.clone();
                        let format = tool.format;
                        menu = menu.item(
                            PopupMenuItem::new(tool.label)
                                .icon(tool.icon)
                                .checked(is_on(format, &style))
                                .on_click(move |_, window, cx| apply(&view, format, window, cx)),
                        );
                    }
                    number_items(menu.separator().label("Number format"), current, &view)
                }),
        );
    } else {
        row = row
            .child(separator(cx))
            .children(ALIGN_TOOLS.iter().map(|tool| tool_button(tool, style, cx)))
            .child(separator(cx))
            .child(
                Button::new("fmt-number")
                    .ghost()
                    .xsmall()
                    .label(number_label(current))
                    .dropdown_caret(true)
                    .tooltip("Number format")
                    .dropdown_menu(move |menu, _, _| number_items(menu, current, &view)),
            );
    }
    if kantor_ui::ai::ai_enabled(cx) {
        row = row.child(div().flex_1()).child(kantor_ui::ai::ai_button(
            "ai-formula",
            "Ask AI for a formula",
            kantor_ui::ai::AskForFormula,
            cx,
        ));
    }
    row
}
