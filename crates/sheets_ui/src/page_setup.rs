//! Page Setup… for the active sheet: a card with Excel's four tabs (Page,
//! Margins, Header/Footer, Sheet). OK sets the sheet's print settings
//! (`Workbook::set_print_settings`, one undo step); a save writes them as
//! `pageSetup`, `pageMargins`, `printOptions`, `headerFooter` and the
//! `_xlnm.Print_Area` / `_xlnm.Print_Titles` names. Set / Clear Print Area
//! are also commands of their own.

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use sheets_engine::{
    HEADER_FOOTER_PRESETS, Margins, PAPERS, Paper, SheetPrint, join_sections, split_sections,
};

use crate::SheetsView;
use crate::dialog::{self, choices, note, row};

actions!(sheets, [PageSetup, SetPrintArea, ClearPrintArea]);

const CM_PER_INCH: f64 = 2.54;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Page,
    Margins,
    HeaderFooter,
    Sheet,
}

/// The Page Setup card.
pub(crate) struct PageSetupDialog {
    sheet: u32,
    tab: Tab,
    /// The settings when the card opened (what it doesn't show is kept).
    base: SheetPrint,
    paper: Paper,
    landscape: bool,
    fit: bool,
    scale: Entity<InputState>,
    fit_w: Entity<InputState>,
    fit_h: Entity<InputState>,
    first_page: Entity<InputState>,
    over_then_down: bool,
    /// Top, bottom, left, right, header, footer, in cm.
    margins: [Entity<InputState>; 6],
    /// The margins as first shown (unchanged text keeps the exact value).
    margins_shown: [String; 6],
    center_h: bool,
    center_v: bool,
    header: [Entity<InputState>; 3],
    footer: [Entity<InputState>; 3],
    area: Entity<InputState>,
    rows: Entity<InputState>,
    cols: Entity<InputState>,
    grid_lines: bool,
    headings: bool,
    black_and_white: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

const MARGIN_LABELS: [&str; 6] = ["Top", "Bottom", "Left", "Right", "Header", "Footer"];

fn margins_cm(m: &Margins) -> [f64; 6] {
    [m.top, m.bottom, m.left, m.right, m.header, m.footer].map(|v| v * CM_PER_INCH)
}

fn cm_text(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl PageSetupDialog {
    /// The settings the card shows, or what is wrong with them.
    fn collect(&self, cx: &App) -> Result<SheetPrint, String> {
        let text = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let mut p = self.base.clone();
        p.setup.paper = self.paper;
        p.setup.landscape = self.landscape;
        p.setup.over_then_down = self.over_then_down;
        p.setup.black_and_white = self.black_and_white;
        if self.fit {
            let pages = |e: &Entity<InputState>, what: &str| -> Result<u32, String> {
                let t = text(e);
                if t.trim().is_empty() {
                    return Ok(0);
                }
                t.trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 32767)
                    .ok_or_else(|| format!("Pages {what}: type a whole number (blank: automatic)"))
            };
            let (w, h) = (pages(&self.fit_w, "wide")?, pages(&self.fit_h, "tall")?);
            if w == 0 && h == 0 {
                return Err("Fit to: give pages wide, tall or both".into());
            }
            p.setup.fit = Some((w, h));
        } else {
            let s = dialog::parse_number(&text(&self.scale))
                .filter(|v| (10.0..=400.0).contains(v))
                .ok_or("Scale: type a percentage from 10 to 400")?;
            p.setup.scale = s.round() as u32;
            p.setup.fit = None;
        }
        let first = text(&self.first_page);
        p.setup.first_page_number = if first.trim().is_empty() || first.trim() == "Auto" {
            None
        } else {
            Some(
                first
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "First page number: type a number, or leave it blank")?,
            )
        };
        let b = &self.base.margins;
        let mut m = [b.top, b.bottom, b.left, b.right, b.header, b.footer];
        for (i, e) in self.margins.iter().enumerate() {
            if text(e) == self.margins_shown[i] {
                continue;
            }
            m[i] = dialog::parse_number(&text(e))
                .filter(|v| (0.0..=50.0).contains(v))
                .ok_or_else(|| format!("{} margin: type centimetres", MARGIN_LABELS[i]))?
                / CM_PER_INCH;
        }
        p.margins = Margins {
            top: m[0],
            bottom: m[1],
            left: m[2],
            right: m[3],
            header: m[4],
            footer: m[5],
        };
        p.options.horizontal_centered = self.center_h;
        p.options.vertical_centered = self.center_v;
        p.options.grid_lines = self.grid_lines;
        p.options.headings = self.headings;
        let h = self.header.each_ref().map(text);
        let f = self.footer.each_ref().map(text);
        p.header_footer.odd_header = join_sections(&h[0], &h[1], &h[2]);
        p.header_footer.odd_footer = join_sections(&f[0], &f[1], &f[2]);
        if p.header_footer.odd_header.len() > 255 || p.header_footer.odd_footer.len() > 255 {
            return Err("Headers and footers hold at most 255 characters".into());
        }
        p.print_area = dialog::parse_ranges(&text(&self.area))
            .ok_or("Print area: type ranges such as $A$1:$F$40")?;
        let rows = text(&self.rows);
        p.title_rows = if rows.trim().is_empty() {
            None
        } else {
            Some(dialog::parse_rows(&rows).ok_or("Rows to repeat: type rows such as $1:$2")?)
        };
        let cols = text(&self.cols);
        p.title_cols = if cols.trim().is_empty() {
            None
        } else {
            Some(dialog::parse_cols(&cols).ok_or("Columns to repeat: type columns such as $A:$B")?)
        };
        Ok(p)
    }
}

impl SheetsView {
    /// Open Page Setup for the active sheet.
    pub fn page_setup_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (sheet, p) = {
            let grid = self.table.read(cx).delegate();
            (grid.sheet, grid.book.print_settings(grid.sheet))
        };
        let new = |window: &mut Window, cx: &mut Context<Self>, value: String| {
            let e = cx.new(|cx| InputState::new(window, cx));
            e.update(cx, |i, cx| i.set_value(value, window, cx));
            e
        };
        let (w, h) = p.setup.fit.unwrap_or((1, 1));
        let num = |n: u32| if n == 0 { String::new() } else { n.to_string() };
        let m = margins_cm(&p.margins);
        let hs = split_sections(&p.header_footer.odd_header);
        let fs = split_sections(&p.header_footer.odd_footer);
        let scale = new(window, cx, p.setup.scale.to_string());
        let fit_w = new(window, cx, num(w));
        let fit_h = new(window, cx, num(h));
        let first_page = new(
            window,
            cx,
            p.setup
                .first_page_number
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        first_page.update(cx, |i, cx| i.set_placeholder("Auto", window, cx));
        let margins_shown = m.map(cm_text);
        let margins = margins_shown.clone().map(|v| new(window, cx, v));
        let header = hs.clone().map(|s| new(window, cx, s));
        let footer = fs.clone().map(|s| new(window, cx, s));
        let area_text = p
            .print_area
            .iter()
            .map(dialog::range_text)
            .collect::<Vec<_>>()
            .join(",");
        let area = new(window, cx, area_text);
        let rows = new(
            window,
            cx,
            p.title_rows.map(dialog::rows_text).unwrap_or_default(),
        );
        let cols = new(
            window,
            cx,
            p.title_cols.map(dialog::cols_text).unwrap_or_default(),
        );
        let mut subs = Vec::new();
        let all: Vec<Entity<InputState>> =
            [&scale, &fit_w, &fit_h, &first_page, &area, &rows, &cols]
                .into_iter()
                .cloned()
                .chain(margins.iter().cloned())
                .chain(header.iter().cloned())
                .chain(footer.iter().cloned())
                .collect();
        for e in &all {
            subs.push(cx.subscribe_in(e, window, Self::on_page_setup_input));
        }
        scale.update(cx, |i, cx| i.focus(window, cx));
        self.page_setup = Some(PageSetupDialog {
            sheet,
            tab: Tab::Page,
            paper: p.setup.paper,
            landscape: p.setup.landscape,
            fit: p.setup.fit.is_some(),
            scale,
            fit_w,
            fit_h,
            first_page,
            over_then_down: p.setup.over_then_down,
            margins,
            margins_shown,
            center_h: p.options.horizontal_centered,
            center_v: p.options.vertical_centered,
            header,
            footer,
            area,
            rows,
            cols,
            grid_lines: p.options.grid_lines,
            headings: p.options.headings,
            black_and_white: p.setup.black_and_white,
            error: None,
            base: p,
            _subs: subs,
        });
        cx.notify();
    }

    fn on_page_setup_input(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if let Some(d) = self.page_setup.as_mut() {
                    d.error = None;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => self.page_setup_ok(window, cx),
            _ => {}
        }
    }

    fn close_page_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_setup.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    fn page_setup_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.page_setup.as_ref() else {
            return;
        };
        let sheet = d.sheet;
        match d.collect(cx) {
            Ok(p) => {
                let res = self.table.update(cx, |table, _| {
                    table.delegate_mut().book.set_print_settings(sheet, p)
                });
                match res {
                    Ok(()) => {
                        self.page_setup = None;
                        self.status = "Page setup changed".into();
                        self.focus_grid(window, cx);
                        self.update_title(window, cx);
                        cx.notify();
                    }
                    Err(e) => {
                        if let Some(d) = self.page_setup.as_mut() {
                            d.error = Some(format!("{e:#}"));
                        }
                        cx.notify();
                    }
                }
            }
            Err(e) => {
                if let Some(d) = self.page_setup.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    /// Set the print area to the selection (one undo step).
    pub fn set_print_area_doc(&mut self, clear: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let res = self.table.update(cx, |table, _| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let sel = grid.range();
            let mut p = grid.book.print_settings(sheet);
            p.print_area = if clear { Vec::new() } else { vec![sel] };
            grid.book.set_print_settings(sheet, p).map(|()| sel)
        });
        self.status = match res {
            Ok(_) if clear => "Print area cleared".into(),
            Ok(sel) => format!("Print area set to {}", sel.name()).into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    fn page_setup_selection(&self, cx: &App) -> sheets_engine::CellRange {
        self.table.read(cx).delegate().range()
    }

    pub(crate) fn render_page_setup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.page_setup.as_ref()?;
        let view = cx.entity().downgrade();
        let tabs = choices(
            "ps-tab",
            &[
                (Tab::Page, "Page"),
                (Tab::Margins, "Margins"),
                (Tab::HeaderFooter, "Header/Footer"),
                (Tab::Sheet, "Sheet"),
            ],
            d.tab,
            &view,
            |this, tab| {
                if let Some(d) = this.page_setup.as_mut() {
                    d.tab = tab;
                }
            },
        );
        let body = match d.tab {
            Tab::Page => self.render_ps_page(d, &view, cx),
            Tab::Margins => self.render_ps_margins(d, &view, cx),
            Tab::HeaderFooter => self.render_ps_header_footer(d, &view, cx),
            Tab::Sheet => self.render_ps_sheet(d, &view, cx),
        };
        let sheet_name = {
            let grid = self.table.read(cx).delegate();
            grid.book
                .sheet_names()
                .get(d.sheet as usize)
                .cloned()
                .unwrap_or_default()
        };
        let card = dialog::card(
            "page-setup",
            format!("Page Setup · {sheet_name}"),
            px(500.),
            cx,
        )
        .key_context("KantorPageSetup")
        .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_page_setup(w, cx)))
        .child(tabs)
        .child(
            v_flex()
                .id("ps-body")
                .gap(SPACE_4)
                .min_h(px(250.))
                .overflow_y_scroll()
                .child(body),
        )
        .children(d.error.clone().map(|e| note(e, true, cx)))
        .child(dialog::buttons(
            "ps-buttons",
            "OK",
            true,
            &view,
            Self::page_setup_ok,
            Self::close_page_setup,
        ));
        Some(dialog::place(card).into_any_element())
    }

    fn render_ps_page(
        &self,
        d: &PageSetupDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let paper_view = view.clone();
        let current = d.paper;
        let paper = Button::new("ps-paper")
            .outline()
            .small()
            .label(current.name())
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for (name, p) in PAPERS {
                    let v = paper_view.clone();
                    m = m.item(
                        PopupMenuItem::new(name)
                            .checked(p.same_as(&current))
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(d) = this.page_setup.as_mut() {
                                        d.paper = p;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        v_flex()
            .gap(SPACE_4)
            .child(row(
                "Orientation",
                choices(
                    "ps-orient",
                    &[(false, "Portrait"), (true, "Landscape")],
                    d.landscape,
                    view,
                    |this, v| {
                        if let Some(d) = this.page_setup.as_mut() {
                            d.landscape = v;
                        }
                    },
                ),
                cx,
            ))
            .child(row("Paper size", paper, cx))
            .child(row(
                "Scaling",
                choices(
                    "ps-fit",
                    &[(false, "Adjust to %"), (true, "Fit to pages")],
                    d.fit,
                    view,
                    |this, v| {
                        if let Some(d) = this.page_setup.as_mut() {
                            d.fit = v;
                        }
                    },
                ),
                cx,
            ))
            .child(if d.fit {
                row(
                    "Pages",
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().w(px(64.)).child(Input::new(&d.fit_w).small()))
                        .child(note("wide by", false, cx))
                        .child(div().w(px(64.)).child(Input::new(&d.fit_h).small()))
                        .child(note("tall (blank: automatic)", false, cx)),
                    cx,
                )
            } else {
                row(
                    "Scale",
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().w(px(80.)).child(Input::new(&d.scale).small()))
                        .child(note("% of normal size", false, cx)),
                    cx,
                )
            })
            .child(row(
                "First page number",
                div().w(px(80.)).child(Input::new(&d.first_page).small()),
                cx,
            ))
            .child(row(
                "Page order",
                choices(
                    "ps-order",
                    &[(false, "Down, then over"), (true, "Over, then down")],
                    d.over_then_down,
                    view,
                    |this, v| {
                        if let Some(d) = this.page_setup.as_mut() {
                            d.over_then_down = v;
                        }
                    },
                ),
                cx,
            ))
    }

    fn render_ps_margins(
        &self,
        d: &PageSetupDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let preset = |id: usize, label: &'static str, m: Margins| {
            let v = view.clone();
            Button::new(("ps-margin-preset", id))
                .small()
                .outline()
                .label(label)
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(d) = this.page_setup.as_ref() {
                            for (e, val) in d.margins.iter().zip(margins_cm(&m)) {
                                e.update(cx, |i, cx| i.set_value(cm_text(val), window, cx));
                            }
                        }
                        cx.notify();
                    });
                })
        };
        let mut grid = v_flex().gap(SPACE_3);
        for pair in [[0, 1], [2, 3], [4, 5]] {
            let mut r = h_flex().gap(SPACE_6);
            for i in pair {
                r = r.child(
                    h_flex()
                        .gap(SPACE_3)
                        .child(
                            div()
                                .w(px(52.))
                                .text_size(FONT_SMALL)
                                .child(MARGIN_LABELS[i]),
                        )
                        .child(div().w(px(80.)).child(Input::new(&d.margins[i]).small()))
                        .child(note("cm", false, cx)),
                );
            }
            grid = grid.child(r);
        }
        let ch = view.clone();
        let cv = view.clone();
        v_flex()
            .gap(SPACE_4)
            .child(row(
                "Presets",
                h_flex()
                    .gap(SPACE_2)
                    .child(preset(0, "Normal", Margins::default()))
                    .child(preset(1, "Wide", Margins::WIDE))
                    .child(preset(2, "Narrow", Margins::NARROW)),
                cx,
            ))
            .child(grid)
            .child(row(
                "Center on page",
                h_flex()
                    .gap(SPACE_6)
                    .child(
                        Checkbox::new("ps-center-h")
                            .label("Horizontally")
                            .checked(d.center_h)
                            .on_click(move |on, _, cx| {
                                let on = *on;
                                let _ = ch.update(cx, |this, cx| {
                                    if let Some(d) = this.page_setup.as_mut() {
                                        d.center_h = on;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Checkbox::new("ps-center-v")
                            .label("Vertically")
                            .checked(d.center_v)
                            .on_click(move |on, _, cx| {
                                let on = *on;
                                let _ = cv.update(cx, |this, cx| {
                                    if let Some(d) = this.page_setup.as_mut() {
                                        d.center_v = on;
                                    }
                                    cx.notify();
                                });
                            }),
                    ),
                cx,
            ))
    }

    fn render_ps_header_footer(
        &self,
        d: &PageSetupDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let part = |id: &'static str, label: &'static str, inputs: &[Entity<InputState>; 3]| {
            let v = view.clone();
            let targets = inputs.clone();
            let presets = Button::new(id)
                .outline()
                .small()
                .label("Presets")
                .dropdown_caret(true)
                .dropdown_menu(move |mut m, _, _| {
                    for (name, text) in HEADER_FOOTER_PRESETS {
                        let v = v.clone();
                        let targets = targets.clone();
                        m = m.item(PopupMenuItem::new(name).on_click(move |_, window, cx| {
                            let s = split_sections(text);
                            let _ = v.update(cx, |_, cx| {
                                for (e, val) in targets.iter().zip(s) {
                                    e.update(cx, |i, cx| i.set_value(val, window, cx));
                                }
                                cx.notify();
                            });
                        }));
                    }
                    m
                });
            v_flex()
                .gap(SPACE_3)
                .child(
                    h_flex()
                        .gap(SPACE_4)
                        .child(kantor_ui::components::section_label(label, cx))
                        .child(div().flex_1())
                        .child(presets),
                )
                .child(
                    h_flex().gap(SPACE_3).children(
                        ["Left", "Centre", "Right"]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| {
                                v_flex()
                                    .flex_1()
                                    .gap(SPACE_1)
                                    .child(note(*l, false, cx))
                                    .child(Input::new(&inputs[i]).small())
                            }),
                    ),
                )
        };
        v_flex()
            .gap(SPACE_6)
            .child(part("ps-header-presets", "Header", &d.header))
            .child(part("ps-footer-presets", "Footer", &d.footer))
            .child(note(
                "Codes: &P page · &N pages · &D date · &T time · &F file name · &A sheet name · && an ampersand",
                false,
                cx,
            ))
    }

    fn render_ps_sheet(
        &self,
        d: &PageSetupDialog,
        view: &WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let sel = self.page_setup_selection(cx);
        let fill =
            |id: &'static str, label: &'static str, target: Entity<InputState>, text: String| {
                let v = view.clone();
                Button::new(id)
                    .ghost()
                    .small()
                    .label(label)
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |_, cx| {
                            target.update(cx, |i, cx| i.set_value(text.clone(), window, cx));
                            cx.notify();
                        });
                    })
            };
        let check = |id: &'static str,
                     label: &'static str,
                     on: bool,
                     set: fn(&mut PageSetupDialog, bool)| {
            let v = view.clone();
            Checkbox::new(id)
                .label(label)
                .checked(on)
                .on_click(move |on, _, cx| {
                    let on = *on;
                    let _ = v.update(cx, |this, cx| {
                        if let Some(d) = this.page_setup.as_mut() {
                            set(d, on);
                        }
                        cx.notify();
                    });
                })
        };
        v_flex()
            .gap(SPACE_4)
            .child(row(
                "Print area",
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.area).small()))
                    .child(fill(
                        "ps-area-sel",
                        "Selection",
                        d.area.clone(),
                        dialog::range_text(&sel),
                    ))
                    .child(fill(
                        "ps-area-clear",
                        "Clear",
                        d.area.clone(),
                        String::new(),
                    )),
                cx,
            ))
            .child(row(
                "Rows to repeat",
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.rows).small()))
                    .child(fill(
                        "ps-rows-sel",
                        "Selection",
                        d.rows.clone(),
                        dialog::rows_text((sel.top, sel.bottom)),
                    )),
                cx,
            ))
            .child(row(
                "Columns to repeat",
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.cols).small()))
                    .child(fill(
                        "ps-cols-sel",
                        "Selection",
                        d.cols.clone(),
                        dialog::cols_text((sel.left, sel.right)),
                    )),
                cx,
            ))
            .child(kantor_ui::components::section_label("Print", cx))
            .child(
                h_flex()
                    .gap(SPACE_6)
                    .flex_wrap()
                    .child(check("ps-grid", "Gridlines", d.grid_lines, |d, v| {
                        d.grid_lines = v
                    }))
                    .child(check(
                        "ps-headings",
                        "Row and column headings",
                        d.headings,
                        |d, v| d.headings = v,
                    ))
                    .child(check(
                        "ps-bw",
                        "Black and white",
                        d.black_and_white,
                        |d, v| d.black_and_white = v,
                    )),
            )
    }

    pub(crate) fn page_setup_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &PageSetup, w, cx| this.page_setup_doc(w, cx)))
            .on_action(
                cx.listener(|this, _: &SetPrintArea, w, cx| this.set_print_area_doc(false, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ClearPrintArea, w, cx| this.set_print_area_doc(true, w, cx)),
            )
    }
}
