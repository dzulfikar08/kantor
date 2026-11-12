//! The per-pane formatting toolbar above the formula bar: bold / italic /
//! underline / strikethrough toggles, horizontal alignment, fill and text
//! colour (swatch popovers), font size up / down, borders, wrap, merge and
//! the number format menu, all showing the active cell's state.
//!
//! In a narrow pane (see `COMPACT_WIDTH`) only B / I / U stay as buttons; the
//! rest moves into a "…" overflow menu (colours are in the command palette).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    popover::Popover,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{BorderPreset, CellStyle, HAlign, NumberFormat, Rgb};

use crate::{Format, SheetsView, look::hsla};

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

const SIZE_TOOLS: [Tool; 2] = [
    Tool {
        id: "fmt-size-up",
        label: "Larger Font",
        keys: "",
        icon: Lucide::AArrowUp,
        format: Format::FontSize(1),
    },
    Tool {
        id: "fmt-size-down",
        label: "Smaller Font",
        keys: "",
        icon: Lucide::AArrowDown,
        format: Format::FontSize(-1),
    },
];

const CELL_TOOLS: [Tool; 2] = [
    Tool {
        id: "fmt-wrap",
        label: "Wrap Text",
        keys: "",
        icon: Lucide::TextWrap,
        format: Format::Wrap,
    },
    Tool {
        id: "fmt-merge",
        label: "Merge & Center",
        keys: "",
        icon: Lucide::TableCellsMerge,
        format: Format::Merge,
    },
];

/// Border presets: label and icon.
const BORDERS: [(BorderPreset, &str, Lucide); 3] = [
    (BorderPreset::All, "All Borders", Lucide::Grid3x3),
    (BorderPreset::Outline, "Outside Borders", Lucide::Square),
    (BorderPreset::None, "No Borders", Lucide::SquareDashed),
];

/// The swatches of the colour popovers, in rows: light tints for fills,
/// Excel's standard colours, darks.
const SWATCHES: [[Rgb; 8]; 3] = [
    [
        0xFFFFFF, 0xF2F2F2, 0xD9D9D9, 0xFFF2CC, 0xE2EFDA, 0xDDEBF7, 0xFCE4D6, 0xE4DFEC,
    ],
    [
        0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0,
    ],
    [
        0x002060, 0x7030A0, 0x000000, 0x404040, 0x808080, 0xBFBFBF, 0x1F4E78, 0x375623,
    ],
];

/// Number of font tools kept as buttons in a narrow pane (B, I, U).
const COMPACT_FONT_TOOLS: usize = 3;

/// The active cell's state, for the buttons.
pub(crate) struct Active<'a> {
    pub style: &'a CellStyle,
    /// The active cell is (the anchor of) a merged cell.
    pub merged: bool,
}

fn is_on(format: Format, active: &Active) -> bool {
    let style = active.style;
    match format {
        Format::Bold => style.bold,
        Format::Italic => style.italic,
        Format::Underline => style.underline,
        Format::Strikethrough => style.strikethrough,
        Format::Align(a) => style.align == a,
        Format::VAlign(a) => style.valign == a,
        Format::Number(f) => style.number_format() == f,
        Format::Wrap => style.wrap,
        Format::Merge => active.merged,
        Format::Fill(_) | Format::FontColor(_) | Format::FontSize(_) | Format::Borders(_) => false,
    }
}

fn apply(view: &WeakEntity<SheetsView>, format: Format, window: &mut Window, cx: &mut App) {
    let _ = view.update(cx, |view, cx| view.format_doc(format, window, cx));
}

fn tool_button(tool: &Tool, active: &Active, cx: &mut Context<SheetsView>) -> Button {
    let view = cx.entity().downgrade();
    let format = tool.format;
    icon_button(tool.id, tool.icon, is_on(format, active), cx)
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

/// Menu entries for the border presets.
fn border_items(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    for (preset, label, icon) in BORDERS {
        let view = view.clone();
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(icon)
                .on_click(move |_, window, cx| apply(&view, Format::Borders(preset), window, cx)),
        );
    }
    menu
}

/// Which colour a swatch popover sets.
#[derive(Clone, Copy)]
enum Paint {
    Fill,
    Text,
}

impl Paint {
    fn format(self, color: Option<Rgb>) -> Format {
        match self {
            Self::Fill => Format::Fill(color),
            Self::Text => Format::FontColor(color),
        }
    }
}

/// A toolbar button that opens a swatch popover. The icon carries a bar of
/// the active cell's colour.
fn color_button(paint: Paint, current: Option<Hsla>, cx: &mut Context<SheetsView>) -> Popover {
    let t = tokens(cx);
    let (id, icon, label, none) = match paint {
        Paint::Fill => ("fmt-fill", Lucide::PaintBucket, "Fill Colour", "No Fill"),
        Paint::Text => (
            "fmt-text-color",
            Lucide::Baseline,
            "Text Colour",
            "Automatic",
        ),
    };
    let view = cx.entity().downgrade();
    let trigger = Button::new(id)
        .ghost()
        .xsmall()
        .child(
            v_flex()
                .items_center()
                .gap(px(1.))
                .child(gpui_kit::component::Icon::new(icon).size(ICON_MD))
                .child(
                    div()
                        .w(ICON_MD)
                        .h(px(3.))
                        .rounded(px(1.))
                        .bg(current.unwrap_or(t.border_strong)),
                ),
        )
        .tooltip(label);
    Popover::new(SharedString::from(format!("{id}-popover")))
        .trigger(trigger)
        .content(move |_, _, cx| {
            let t = tokens(cx);
            let popover = cx.entity().downgrade();
            let view = view.clone();
            let pick = move |color: Option<Rgb>| {
                let view = view.clone();
                let popover = popover.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = popover.update(cx, |p, cx| p.dismiss(window, cx));
                    apply(&view, paint.format(color), window, cx);
                }
            };
            let swatch = |color: Rgb| {
                div()
                    .id(("swatch", color as usize))
                    .size(px(18.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(t.border_strong)
                    .bg(hsla(color))
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.accent))
                    .tooltip(move |window, cx| {
                        Tooltip::new(format!("#{color:06X}")).build(window, cx)
                    })
                    .on_click(pick(Some(color)))
            };
            v_flex()
                .gap(SPACE_2)
                .p(SPACE_1)
                .child(
                    div()
                        .id("swatch-none")
                        .px(SPACE_3)
                        .py(SPACE_1)
                        .rounded(RADIUS_SM)
                        .text_size(FONT_SMALL)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .child(none)
                        .on_click(pick(None)),
                )
                .children(SWATCHES.iter().map(|row| {
                    h_flex()
                        .gap(SPACE_1)
                        .children(row.iter().map(|&c| swatch(c)))
                }))
        })
}

/// The toolbar row for the active cell.
pub(crate) fn render(active: &Active, compact: bool, cx: &mut Context<SheetsView>) -> Div {
    let t = tokens(cx);
    let style = active.style;
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
                .map(|tool| tool_button(tool, active, cx)),
        );

    if compact {
        // Everything else goes into one overflow menu.
        let style = style.clone();
        let merged = active.merged;
        row = row.child(separator(cx)).child(
            Button::new("fmt-more")
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .tooltip("More formatting")
                .dropdown_menu(move |mut menu, _, _| {
                    let active = Active {
                        style: &style,
                        merged,
                    };
                    for tool in FONT_TOOLS[COMPACT_FONT_TOOLS..]
                        .iter()
                        .chain(ALIGN_TOOLS.iter())
                        .chain(SIZE_TOOLS.iter())
                        .chain(CELL_TOOLS.iter())
                    {
                        let view = view.clone();
                        let format = tool.format;
                        menu = menu.item(
                            PopupMenuItem::new(tool.label)
                                .icon(tool.icon)
                                .checked(is_on(format, &active))
                                .on_click(move |_, window, cx| apply(&view, format, window, cx)),
                        );
                    }
                    let menu = border_items(menu.separator(), &view);
                    number_items(menu.separator().label("Number format"), current, &view)
                }),
        );
    } else {
        let border_view = view.clone();
        row = row
            .child(separator(cx))
            .children(ALIGN_TOOLS.iter().map(|tool| tool_button(tool, active, cx)))
            .child(separator(cx))
            .child(color_button(Paint::Fill, style.fill.map(hsla), cx))
            .child(color_button(Paint::Text, style.font_color.map(hsla), cx))
            .children(SIZE_TOOLS.iter().map(|tool| tool_button(tool, active, cx)))
            .child(
                Button::new("fmt-borders")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Grid3x3)
                    .dropdown_caret(true)
                    .tooltip("Borders")
                    .dropdown_menu(move |menu, _, _| border_items(menu, &border_view)),
            )
            .children(CELL_TOOLS.iter().map(|tool| tool_button(tool, active, cx)))
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
    // View: frozen panes, hidden rows and columns.
    let view_view = cx.entity().downgrade();
    row = row.child(separator(cx)).child(
        Button::new("view-menu")
            .ghost()
            .xsmall()
            .icon(Lucide::Snowflake)
            .dropdown_caret(true)
            .tooltip("Freeze panes, hide rows and columns")
            .dropdown_menu(move |menu, _, _| view_items(menu, &view_view)),
    );
    // PivotTables.
    let pivot_view = cx.entity().downgrade();
    row = row.child(separator(cx)).child(
        Button::new("pivot-menu")
            .ghost()
            .xsmall()
            .icon(Lucide::TableProperties)
            .dropdown_caret(true)
            .tooltip("PivotTable")
            .dropdown_menu(move |menu, _, _| pivot_items(menu, &pivot_view)),
    );
    // Export as PDF and Print.
    let print_view = cx.entity().downgrade();
    row = row.child(separator(cx)).child(
        Button::new("file-print")
            .ghost()
            .xsmall()
            .icon(Lucide::Printer)
            .dropdown_caret(true)
            .tooltip("Print and export")
            .dropdown_menu(move |menu, _, _| {
                let export = print_view.clone();
                let print = print_view.clone();
                menu.item(
                    PopupMenuItem::new("Export as PDF…")
                        .icon(Lucide::FileDown)
                        .on_click(move |_, window, cx| {
                            let _ = export.update(cx, |v, cx| v.export_pdf_doc(window, cx));
                        }),
                )
                .item(PopupMenuItem::new("Print…").icon(Lucide::Printer).on_click(
                    move |_, window, cx| {
                        let _ = print.update(cx, |v, cx| v.print_doc(window, cx));
                    },
                ))
            }),
    );
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

/// The View menu: Excel's Freeze Panes choices, then Hide / Unhide.
fn view_items(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    use crate::GridCommand as G;
    let items: [(&str, &str, G); 8] = [
        ("Freeze Panes", "", G::FreezePanes),
        ("Freeze Top Row", "", G::FreezeTopRow),
        ("Freeze First Column", "", G::FreezeFirstColumn),
        ("Unfreeze Panes", "", G::Unfreeze),
        ("Hide Rows", "Ctrl+9", G::HideRows),
        ("Unhide Rows", "Ctrl+Shift+9", G::UnhideRows),
        ("Hide Columns", "Ctrl+0", G::HideColumns),
        ("Unhide Columns", "Ctrl+Shift+0", G::UnhideColumns),
    ];
    for (i, (label, keys, command)) in items.into_iter().enumerate() {
        if i == 4 {
            menu = menu.separator();
        }
        let view = view.clone();
        let text = if keys.is_empty() {
            label.to_owned()
        } else {
            format!("{label}  ({keys})")
        };
        menu = menu.item(PopupMenuItem::new(text).on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| v.grid_command(command, window, cx));
        }));
    }
    menu
}

/// The PivotTable menu: insert, fields, refresh, delete.
fn pivot_items(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    type Run = fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>);
    let items: [(&str, &str, Run); 5] = [
        ("Insert PivotTable…", "", |v, w, cx| {
            v.insert_pivot_doc(w, cx)
        }),
        ("PivotTable Fields", "", |v, w, cx| {
            v.pivot_fields_doc(w, cx)
        }),
        ("Refresh PivotTable", "Alt+F5", |v, w, cx| {
            v.refresh_pivots_doc(false, w, cx)
        }),
        ("Refresh All PivotTables", "Ctrl+Alt+F5", |v, w, cx| {
            v.refresh_pivots_doc(true, w, cx)
        }),
        ("Delete PivotTable", "", |v, w, cx| {
            v.delete_pivot_doc(w, cx)
        }),
    ];
    for (i, (label, keys, run)) in items.into_iter().enumerate() {
        if i == 2 || i == 4 {
            menu = menu.separator();
        }
        let view = view.clone();
        let text = if keys.is_empty() {
            label.to_owned()
        } else {
            format!("{label}  ({keys})")
        };
        menu = menu.item(PopupMenuItem::new(text).on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| run(v, window, cx));
        }));
    }
    menu
}
