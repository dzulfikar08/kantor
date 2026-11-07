//! The command palette (Ctrl+K): commands, the theme picker and the font picker
//! share one panel, like Voltrus Tools' palette modes.

use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::fuzzy;
use kantor_theme::catalog::{self, ThemeEntry};
use kantor_ui::components::kbd;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;

use super::Workspace;
use super::doc::CommonFormat;
use crate::actions::*;
use crate::theme;
use sheets_ui::{Format, NumberFormat};

/// A palette command.
pub(super) struct Command {
    title: &'static str,
    detail: &'static str,
    keys: &'static str,
    run: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
}

pub(super) const COMMANDS: &[Command] = &[
    Command {
        title: "New Spreadsheet",
        detail: "Sheets",
        keys: "Ctrl N",
        run: |ws, window, cx| ws.new_spreadsheet(&NewSpreadsheet, window, cx),
    },
    Command {
        title: "New Document",
        detail: "Docs",
        keys: "Ctrl Shift N",
        run: |ws, window, cx| ws.new_document(&NewDocument, window, cx),
    },
    Command {
        title: "Open File…",
        detail: ".xlsx, .docx, .pptx, .csv, .ods",
        keys: "Ctrl O",
        run: |ws, window, cx| ws.open_file(&OpenFile, window, cx),
    },
    Command {
        title: "Save",
        detail: "Current document",
        keys: "Ctrl S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save(w, cx)),
    },
    Command {
        title: "Save As…",
        detail: "Current document",
        keys: "Ctrl Shift S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save_as(w, cx)),
    },
    Command {
        title: "Undo",
        detail: "Last edit",
        keys: "Ctrl Z",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.undo(w, cx)),
    },
    Command {
        title: "Redo",
        detail: "Last undone edit",
        keys: "Ctrl Shift Z",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.redo(w, cx)),
    },
    Command {
        title: "Copy",
        detail: "Selection (cells as tab-separated text)",
        keys: "Ctrl C",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.copy(w, cx)),
    },
    Command {
        title: "Cut",
        detail: "Copy and delete the selection",
        keys: "Ctrl X",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.cut(w, cx)),
    },
    Command {
        title: "Paste",
        detail: "Plain text, at the caret or selected cell",
        keys: "Ctrl V",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.paste(w, cx)),
    },
    Command {
        title: "Bold",
        detail: "Selection",
        keys: "Ctrl B",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| v.format(CommonFormat::Bold, w, cx))
        },
    },
    Command {
        title: "Italic",
        detail: "Selection",
        keys: "Ctrl I",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| v.format(CommonFormat::Italic, w, cx))
        },
    },
    Command {
        title: "Underline",
        detail: "Selection",
        keys: "Ctrl U",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::Underline, w, cx)
            })
        },
    },
    Command {
        title: "Strikethrough",
        detail: "Selected cells",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Strikethrough, w, cx)
            })
        },
    },
    Command {
        title: "Align Left",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignLeft, w, cx)
            })
        },
    },
    Command {
        title: "Align Center",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignCenter, w, cx)
            })
        },
    },
    Command {
        title: "Align Right",
        detail: "Selection or paragraphs",
        keys: "",
        run: |ws, window, cx| {
            ws.with_active(window, cx, |v, w, cx| {
                v.format(CommonFormat::AlignRight, w, cx)
            })
        },
    },
    Command {
        title: "Format as General",
        detail: "Number format",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::General), w, cx)
            })
        },
    },
    Command {
        title: "Format as Number",
        detail: "Number format · 1,234.50",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Number), w, cx)
            })
        },
    },
    Command {
        title: "Format as Percent",
        detail: "Number format · 12.50%",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Percent), w, cx)
            })
        },
    },
    Command {
        title: "Format as Rupiah",
        detail: "Number format · Rp1,250,000",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::CurrencyIdr), w, cx)
            })
        },
    },
    Command {
        title: "Format as Dollars",
        detail: "Number format · $1,234.50",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::CurrencyUsd), w, cx)
            })
        },
    },
    Command {
        title: "Format as Date",
        detail: "Number format · 2026-11-07",
        keys: "",
        run: |ws, window, cx| {
            ws.with_sheets(window, cx, |v, w, cx| {
                v.format_doc(Format::Number(NumberFormat::Date), w, cx)
            })
        },
    },
    Command {
        title: "Toggle Sidebar",
        detail: "Open documents, recent files",
        keys: "Ctrl Shift E",
        run: |ws, window, cx| ws.toggle_sidebar(&ToggleSidebar, window, cx),
    },
    Command {
        title: "Toggle Inspector",
        detail: "Cell and workbook details",
        keys: "Ctrl Shift I",
        run: |ws, window, cx| ws.toggle_inspector(&ToggleInspector, window, cx),
    },
    Command {
        title: "Split Right",
        detail: "New pane beside",
        keys: "Ctrl \\",
        run: |ws, window, cx| ws.split_right(&SplitRight, window, cx),
    },
    Command {
        title: "Split Down",
        detail: "New pane below",
        keys: "Ctrl Alt \\",
        run: |ws, window, cx| ws.split_down(&SplitDown, window, cx),
    },
    Command {
        title: "Close Pane",
        detail: "Or the tab, if it has one pane",
        keys: "Ctrl W",
        run: |ws, window, cx| ws.close_pane(&ClosePane, window, cx),
    },
    Command {
        title: "Close Tab",
        detail: "All its panes",
        keys: "Ctrl Shift W",
        run: |ws, window, cx| ws.close_active_tab(&CloseTab, window, cx),
    },
    Command {
        title: "Next Tab",
        detail: "",
        keys: "Ctrl Tab",
        run: |ws, window, cx| ws.next_tab(&NextTab, window, cx),
    },
    Command {
        title: "Previous Tab",
        detail: "",
        keys: "Ctrl Shift Tab",
        run: |ws, window, cx| ws.prev_tab(&PrevTab, window, cx),
    },
    Command {
        title: "Go Home",
        detail: "Start screen",
        keys: "Ctrl Shift H",
        run: |ws, window, cx| ws.go_home(&GoHome, window, cx),
    },
    Command {
        title: "Heading 1",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 1",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading1"), w, cx)
            })
        },
    },
    Command {
        title: "Heading 2",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 2",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading2"), w, cx)
            })
        },
    },
    Command {
        title: "Heading 3",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 3",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_style_doc(Some("Heading3"), w, cx)
            })
        },
    },
    Command {
        title: "Normal Text",
        detail: "Docs · paragraph style",
        keys: "Ctrl Alt 0",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.set_style_doc(None, w, cx)),
    },
    Command {
        title: "Justify",
        detail: "Docs · paragraphs",
        keys: "Ctrl J",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.set_align_doc(docs_engine::Align::Justify, w, cx)
            })
        },
    },
    Command {
        title: "Bulleted List",
        detail: "Docs · lists",
        keys: "Ctrl Shift 8",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.toggle_list_doc(docs_engine::ListKind::Bullet, w, cx)
            })
        },
    },
    Command {
        title: "Numbered List",
        detail: "Docs · lists",
        keys: "Ctrl Shift 7",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.toggle_list_doc(docs_engine::ListKind::Numbered, w, cx)
            })
        },
    },
    Command {
        title: "Insert Table",
        detail: "Docs · 3 × 3 (toolbar grid for other sizes)",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.insert_table_doc(3, 3, w, cx)),
    },
    Command {
        title: "Insert Row Above",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::RowAbove, w, cx)
            })
        },
    },
    Command {
        title: "Insert Row Below",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::RowBelow, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Left",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::ColumnLeft, w, cx)
            })
        },
    },
    Command {
        title: "Insert Column Right",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::ColumnRight, w, cx)
            })
        },
    },
    Command {
        title: "Delete Row",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteRow, w, cx)
            })
        },
    },
    Command {
        title: "Delete Column",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteColumn, w, cx)
            })
        },
    },
    Command {
        title: "Delete Table",
        detail: "Docs · table",
        keys: "",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, w, cx| {
                v.table_edit_doc(docs_engine::TableEdit::DeleteTable, w, cx)
            })
        },
    },
    Command {
        title: "Insert Picture…",
        detail: "Docs · PNG or JPEG",
        keys: "",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, w, cx| v.insert_image_doc(w, cx)),
    },
    Command {
        title: "Zoom In",
        detail: "Docs · page zoom",
        keys: "Ctrl =",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, _, cx| {
                v.zoom_by(kantor_ui::metrics::ZOOM_STEP, cx)
            })
        },
    },
    Command {
        title: "Zoom Out",
        detail: "Docs · page zoom",
        keys: "Ctrl -",
        run: |ws, window, cx| {
            ws.with_docs(window, cx, |v, _, cx| {
                v.zoom_by(1.0 / kantor_ui::metrics::ZOOM_STEP, cx)
            })
        },
    },
    Command {
        title: "Actual Size",
        detail: "Docs · page zoom 100%",
        keys: "Ctrl 0",
        run: |ws, window, cx| ws.with_docs(window, cx, |v, _, cx| v.set_zoom(1.0, cx)),
    },
    Command {
        title: "New Presentation",
        detail: "Slides",
        keys: "Ctrl Alt N",
        run: |ws, window, cx| ws.new_presentation(&NewPresentation, window, cx),
    },
    Command {
        title: "New Slide",
        detail: "Slides",
        keys: "Ctrl M",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.new_slide_doc(w, cx)),
    },
    Command {
        title: "Duplicate Slide",
        detail: "Slides",
        keys: "Ctrl D",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.duplicate_slide_doc(w, cx)),
    },
    Command {
        title: "Delete Slide",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.delete_slide_doc(w, cx)),
    },
    Command {
        title: "Move Slide Up",
        detail: "Slides",
        keys: "Ctrl Up",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.move_slide_doc(true, w, cx)),
    },
    Command {
        title: "Move Slide Down",
        detail: "Slides",
        keys: "Ctrl Down",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.move_slide_doc(false, w, cx)),
    },
    Command {
        title: "Insert Text Box",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.insert_text_box_doc(w, cx)),
    },
    Command {
        title: "Insert Rectangle",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.insert_shape_doc(slides_engine::Geom::Rect, w, cx)
            })
        },
    },
    Command {
        title: "Insert Ellipse",
        detail: "Slides",
        keys: "",
        run: |ws, window, cx| {
            ws.with_slides(window, cx, |v, w, cx| {
                v.insert_shape_doc(slides_engine::Geom::Ellipse, w, cx)
            })
        },
    },
    Command {
        title: "Insert Picture…",
        detail: "Slides · PNG or JPEG",
        keys: "",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.insert_image_doc(w, cx)),
    },
    Command {
        title: "Present",
        detail: "Slides · from the first slide",
        keys: "F5",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.present_doc(true, w, cx)),
    },
    Command {
        title: "Present From This Slide",
        detail: "Slides",
        keys: "Shift F5",
        run: |ws, window, cx| ws.with_slides(window, cx, |v, w, cx| v.present_doc(false, w, cx)),
    },
    Command {
        title: "Choose Theme…",
        detail: "Omarchy, Ghostty",
        keys: "",
        run: |ws, window, cx| ws.open_palette(PaletteMode::themes(), window, cx),
    },
    Command {
        title: "Choose UI Font…",
        detail: "SF Pro, Inter, …",
        keys: "",
        run: |ws, window, cx| ws.open_palette(PaletteMode::fonts(cx), window, cx),
    },
    Command {
        title: "Quit Kantor",
        detail: "",
        keys: "Ctrl Q",
        run: |_, _, cx| cx.quit(),
    },
];

/// What the palette is listing.
pub(super) enum PaletteMode {
    Commands,
    Themes(Vec<ThemeEntry>),
    /// `None` = automatic font choice.
    Fonts(Vec<Option<String>>),
}

impl PaletteMode {
    pub(super) fn themes() -> Self {
        Self::Themes(catalog::all())
    }

    pub(super) fn fonts(cx: &App) -> Self {
        let mut fonts = vec![None];
        fonts.extend(kantor_ui::fonts::installed(cx).into_iter().map(Some));
        Self::Fonts(fonts)
    }

    pub(super) fn names(&self) -> Vec<String> {
        match self {
            Self::Commands => COMMANDS.iter().map(|c| c.title.to_owned()).collect(),
            Self::Themes(t) => t.iter().map(|e| e.name.clone()).collect(),
            Self::Fonts(f) => f
                .iter()
                .map(|f| f.clone().unwrap_or_else(|| "Automatic".to_owned()))
                .collect(),
        }
    }

    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            Self::Commands => "Type a command…",
            Self::Themes(_) => "Search themes…",
            Self::Fonts(_) => "Search fonts…",
        }
    }

    pub(super) fn row_height(&self) -> Pixels {
        match self {
            Self::Themes(_) => PALETTE_THEME_ROW_HEIGHT,
            _ => PALETTE_ROW_HEIGHT,
        }
    }

    pub(super) fn noun(&self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Themes(_) => "themes",
            Self::Fonts(_) => "fonts",
        }
    }
}

pub(super) struct Palette {
    mode: PaletteMode,
    input: Entity<InputState>,
    /// Indices into the mode's items, best match first.
    matches: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl Workspace {
    pub(super) fn toggle_palette(
        &mut self,
        _: &TogglePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.close_palette(true, window, cx);
        } else {
            self.open_palette(PaletteMode::Commands, window, cx);
        }
    }

    pub(super) fn open_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = mode.placeholder();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&input, window, Self::on_palette_input);
        input.update(cx, |i, cx| i.focus(window, cx));
        let matches: Vec<usize> = (0..mode.names().len()).collect();
        // Start on the active theme/font so the picker opens where you are.
        let selected = match &mode {
            PaletteMode::Commands => 0,
            PaletteMode::Themes(t) => {
                let active = &theme::current(cx).theme;
                t.iter().position(|e| &e.id == active).unwrap_or(0)
            }
            PaletteMode::Fonts(f) => {
                let active = &theme::current(cx).ui_font;
                f.iter().position(|x| x == active).unwrap_or(0)
            }
        };
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item(selected, ScrollStrategy::Center);
        self.palette = Some(Palette {
            mode,
            input,
            matches,
            selected,
            scroll,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn on_palette_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                if let Some(p) = self.palette.as_mut() {
                    let names = p.mode.names();
                    p.matches = fuzzy::rank(&query, names.iter().map(String::as_str));
                    p.selected = 0;
                    p.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => self.run_selected(window, cx),
            _ => {}
        }
    }

    /// Close the palette; `restore` undoes theme/font previews (Esc, click outside).
    pub(super) fn close_palette(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.palette.take() else {
            return;
        };
        if restore && !matches!(p.mode, PaletteMode::Commands) {
            theme::restore(cx);
        }
        // Return focus to whatever the palette covered.
        self.activate(self.active, window, cx);
    }

    pub(super) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(true, window, cx);
    }

    pub(super) fn palette_escape(
        &mut self,
        _: &input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette(true, window, cx);
    }

    pub(super) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        let n = p.matches.len() as isize;
        if n == 0 {
            return;
        }
        p.selected = (p.selected as isize + delta).rem_euclid(n) as usize;
        p.scroll.scroll_to_item(p.selected, ScrollStrategy::Center);
        self.preview_selected(cx);
        cx.notify();
    }

    /// Live preview of the highlighted theme or font.
    pub(super) fn preview_selected(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_ref() else {
            return;
        };
        let Some(&ix) = p.matches.get(p.selected) else {
            return;
        };
        match &p.mode {
            PaletteMode::Commands => {}
            PaletteMode::Themes(t) => {
                let palette = t[ix].palette.clone();
                theme::preview_palette(&palette, cx);
            }
            PaletteMode::Fonts(f) => {
                let font = f[ix].clone();
                theme::preview_font(font.as_deref(), cx);
            }
        }
    }

    pub(super) fn palette_up(&mut self, _: &PaletteUp, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(-1, cx);
    }

    pub(super) fn palette_down(&mut self, _: &PaletteDown, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(1, cx);
    }

    /// Run/apply item `ix` of the current mode.
    pub(super) fn palette_pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.take() else {
            return;
        };
        match p.mode {
            PaletteMode::Commands => {
                self.activate(self.active, window, cx);
                if let Some(cmd) = COMMANDS.get(ix) {
                    (cmd.run)(self, window, cx);
                }
            }
            PaletteMode::Themes(t) => {
                if let Some(e) = t.into_iter().nth(ix) {
                    self.message = format!("Theme: {}", e.name).into();
                    theme::set_theme(e.id, cx);
                }
                self.activate(self.active, window, cx);
            }
            PaletteMode::Fonts(f) => {
                if let Some(font) = f.into_iter().nth(ix) {
                    self.message =
                        format!("Font: {}", font.as_deref().unwrap_or("Automatic")).into();
                    theme::set_font(font, cx);
                }
                self.activate(self.active, window, cx);
            }
        }
    }

    pub(super) fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .palette
            .as_ref()
            .and_then(|p| p.matches.get(p.selected).copied());
        match ix {
            Some(ix) => self.palette_pick(ix, window, cx),
            None => self.close_palette(true, window, cx),
        }
    }

    pub(super) fn render_palette_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let (active_theme, active_font) = {
            let a = theme::current(cx);
            (a.theme.clone(), a.ui_font.clone())
        };
        let automatic = kantor_ui::fonts::automatic_ui_font(cx);
        let Some(p) = self.palette.as_ref() else {
            return Vec::new();
        };
        let row_h = p.mode.row_height();
        let mut rows = Vec::new();
        for row in range {
            let Some(&ix) = p.matches.get(row) else {
                continue;
            };
            let selected = row == p.selected;
            let entity = entity.clone();
            let base = div()
                .id(("palette-row", ix))
                .w_full()
                .h(row_h)
                .px(SPACE_6)
                .flex()
                .items_center()
                .gap(SPACE_4)
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when_else(
                    selected,
                    |d| d.bg(t.selected),
                    |d| d.hover(|s| s.bg(t.hover)),
                )
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.palette_pick(ix, window, cx));
                });
            let check = |on: bool| {
                div()
                    .w(px(14.))
                    .text_size(FONT_SMALL)
                    .text_color(t.accent)
                    .child(if on { "✓" } else { "" })
            };
            let row_el = match &p.mode {
                PaletteMode::Commands => {
                    let cmd = &COMMANDS[ix];
                    base.child(div().text_size(FONT_BODY).text_color(t.fg).child(cmd.title))
                        .child(
                            div()
                                .flex_1()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_dim)
                                .child(cmd.detail),
                        )
                        .when(!cmd.keys.is_empty(), |d| d.child(kbd(cmd.keys, cx)))
                }
                PaletteMode::Themes(themes) => {
                    let e = &themes[ix];
                    let pal = &e.palette;
                    let swatch = h_flex()
                        .h(px(14.))
                        .w_full()
                        .rounded(px(3.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .w(CONTROL_MD)
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(pal.background))
                                .text_color(rgb(pal.foreground))
                                .text_size(FONT_CAPTION)
                                .font_weight(FontWeight::MEDIUM)
                                .child("Ab"),
                        )
                        .children(
                            e.swatch()
                                .into_iter()
                                .map(|c| div().flex_1().h_full().bg(rgb(c))),
                        );
                    base.child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(SPACE_2)
                            .child(
                                h_flex()
                                    .gap(SPACE_3)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(FONT_BODY)
                                            .text_color(t.fg)
                                            .child(e.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(FONT_CAPTION)
                                            .text_color(t.fg_dim)
                                            .child(e.group.label()),
                                    )
                                    .child(check(e.id == active_theme)),
                            )
                            .child(swatch),
                    )
                }
                PaletteMode::Fonts(fonts) => {
                    let font = &fonts[ix];
                    let (label, family) = match font {
                        Some(f) => (f.clone(), f.clone()),
                        None => (
                            format!("Automatic ({})", automatic.as_deref().unwrap_or("system")),
                            automatic.clone().unwrap_or_default(),
                        ),
                    };
                    base.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_EMPHASIS)
                            .text_color(t.fg)
                            .when(!family.is_empty(), |d| d.font_family(family))
                            .child(label),
                    )
                    .child(check(font == &active_font))
                }
            };
            rows.push(row_el);
        }
        rows
    }

    pub(super) fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let hints: Vec<Div> = [("↑↓", "navigate"), ("↵", "choose"), ("Esc", "close")]
            .into_iter()
            .map(|(keys, label)| h_flex().gap(SPACE_2).child(kbd(keys, cx)).child(label))
            .collect();
        let on_escape = cx.listener(Self::palette_escape);
        let rows_fn = cx.processor(|ws: &mut Self, range: std::ops::Range<usize>, _, cx| {
            ws.render_palette_rows(range, cx)
        });
        let palette = self.palette.as_ref()?;

        let count = palette.matches.len();
        let row_h = palette.mode.row_height();
        let visible = count.clamp(1, PALETTE_MAX_ROWS) as f32;
        let list = uniform_list("palette-list", count, rows_fn)
            .track_scroll(&palette.scroll)
            .w_full()
            .h(row_h * visible);

        let panel = v_flex()
            .id("palette")
            .key_context(PALETTE)
            .on_action(on_escape)
            .w(PALETTE_WIDTH)
            .mt(PALETTE_TOP)
            .rounded(RADIUS_XL)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border_strong)
            .shadow(t.shadow(Elevation::Modal))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(SPACE_4)
                    .border_b_1()
                    .border_color(t.border)
                    .child(Input::new(&palette.input).appearance(false)),
            )
            .child(div().p(SPACE_3).when_else(
                count == 0,
                |d| {
                    d.child(
                        div()
                            .h(PALETTE_ROW_HEIGHT)
                            .px(SPACE_6)
                            .flex()
                            .items_center()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child("No matches"),
                    )
                },
                |d| d.child(list),
            ))
            .child(
                h_flex()
                    .h(PALETTE_FOOTER_HEIGHT)
                    .px(SPACE_6)
                    .gap(SPACE_6)
                    .bg(t.chrome)
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(FONT_CAPTION)
                    .text_color(t.fg_muted)
                    .children(hints)
                    .child(div().flex_1())
                    .child(format!("{count} {}", palette.mode.noun())),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .bg(t.scrim)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.close_palette(true, window, cx));
                })
                .child(panel),
        )
    }
}
