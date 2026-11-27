//! Workbooks with macros (a VBA project): a bar when one opens, and the
//! Macros dialog (Excel's View ▸ Macros, Alt+F8) listing the project's
//! modules and procedures, with a module's code shown read-only.
//!
//! Kantor doesn't run macros. The bar says so; its state ([`MacroTrust`])
//! is per pane and starts `Disabled` for every workbook with macros, so a
//! later "Enable Content" (per file, as Excel's trusted documents) only
//! needs a new state and a button on the bar.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, input, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::vba::{Module, ModuleKind, Procedure, VbaProject};

use crate::SheetsView;
use crate::dialog;

/// What the workbook's macros may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MacroTrust {
    /// No VBA project.
    #[default]
    NoMacros,
    /// Macros are shown, never run (running them isn't supported yet).
    Disabled,
}

/// A pane's macros: the trust, and whether the bar was closed.
#[derive(Default)]
pub(crate) struct MacroState {
    pub(crate) trust: MacroTrust,
    bar_closed: bool,
}

/// The Macros dialog.
pub(crate) struct MacrosDialog {
    /// The project, or why it couldn't be read.
    project: Result<Rc<VbaProject>, String>,
    /// Each module's procedures.
    procedures: Vec<Vec<Procedure>>,
    module: usize,
    procedure: Option<usize>,
    /// The picked module's lines (tabs as spaces).
    lines: Rc<Vec<SharedString>>,
    scroll: UniformListScrollHandle,
}

/// Rows of code shown at once.
const CODE_ROWS: f32 = 22.;
const LINE_H: Pixels = px(18.);

/// As the Visual Basic Editor's folders name them.
fn kind_label(kind: ModuleKind) -> &'static str {
    match kind {
        ModuleKind::Standard => t("Module"),
        ModuleKind::Class => t("Class Module"),
        ModuleKind::Document => t("Document"),
        ModuleKind::Form => t("UserForm"),
    }
}

fn code_lines(module: &Module) -> Rc<Vec<SharedString>> {
    Rc::new(
        module
            .code
            .lines()
            .map(|l| SharedString::from(l.replace('\t', "    ")))
            .collect(),
    )
}

impl SheetsView {
    /// After a workbook was opened: the bar when it has macros.
    pub(crate) fn macros_after_open(&mut self, cx: &mut Context<Self>) {
        let has = self.table.read(cx).delegate().book.has_macros();
        self.macros = MacroState {
            trust: if has {
                MacroTrust::Disabled
            } else {
                MacroTrust::NoMacros
            },
            bar_closed: false,
        };
    }

    /// The bar saying the workbook has macros (until closed).
    pub(crate) fn render_macro_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.macros.trust != MacroTrust::Disabled || self.macros.bar_closed {
            return None;
        }
        let t_ = tokens(cx);
        Some(
            h_flex()
                .id("sheets-macro-bar")
                .flex_none()
                .h(CONTROL_LG + SPACE_2)
                .gap(SPACE_4)
                .px(SPACE_4)
                .bg(t_.warning.opacity(0.14))
                .border_b_1()
                .border_color(t_.border)
                .text_size(FONT_SMALL)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t_.fg)
                        .child(t("Macros")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t_.fg_muted)
                        .child(t(
                            "This workbook contains macros. Kantor can show them; running macros is not supported yet.",
                        )),
                )
                .child(
                    Button::new("sheets-macro-view")
                        .label(t("View Macros"))
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.macros_doc(window, cx))),
                )
                .child(
                    Button::new("sheets-macro-close")
                        .icon(gpui_kit::assets::IconName::X)
                        .ghost()
                        .small()
                        .tooltip(t("Close"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.macros.bar_closed = true;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Open the Macros dialog (View ▸ Macros, Alt+F8).
    pub fn macros_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let project = self.table.read(cx).delegate().book.vba_project();
        let Some(project) = project else {
            self.status = t("This workbook has no macros").into();
            cx.notify();
            return;
        };
        let project = project.map(Rc::new);
        let procedures = match &project {
            Ok(p) => p.modules().iter().map(Module::procedures).collect(),
            Err(e) => {
                log::warn!("reading the VBA project: {e}");
                Vec::new()
            }
        };
        // The first module with a procedure, as Alt+F8 lists macros first.
        let module = procedures.iter().position(|p| !p.is_empty()).unwrap_or(0);
        let lines = match &project {
            Ok(p) => p.modules().get(module).map(code_lines).unwrap_or_default(),
            Err(_) => Rc::default(),
        };
        let first = procedures
            .get(module)
            .and_then(|p| (!p.is_empty()).then_some(0));
        self.macros_dialog = Some(MacrosDialog {
            project,
            procedures,
            module,
            procedure: first,
            lines,
            scroll: UniformListScrollHandle::new(),
        });
        self.pick_macro(module, first, window, cx);
    }

    fn close_macros(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.macros_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// Show module `module`'s code, scrolled to procedure `procedure`.
    fn pick_macro(
        &mut self,
        module: usize,
        procedure: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.macros_dialog.as_mut() else {
            return;
        };
        if let Ok(p) = &d.project
            && let Some(m) = p.modules().get(module)
        {
            if d.module != module || d.lines.is_empty() {
                d.lines = code_lines(m);
            }
            d.module = module;
            d.procedure = procedure;
            let line = procedure
                .and_then(|i| d.procedures.get(module)?.get(i))
                .map_or(0, |p| p.line);
            d.scroll.scroll_to_item(line, ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub(crate) fn render_macros_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.macros_dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let t_ = tokens(cx);
        let mut card = dialog::card("macros", t("Macros"), px(860.), cx)
            .key_context("KantorMacros")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_macros(w, cx)));
        match &d.project {
            Err(e) => {
                card = card.child(dialog::note(
                    tf("Kantor can't read this workbook's macros: {}", &[e]),
                    true,
                    cx,
                ));
            }
            Ok(project) => {
                let mono = cx.theme().mono_font_family.clone();
                // Modules, each with its procedures.
                let mut list = v_flex()
                    .id("macros-modules")
                    .flex_none()
                    .w(px(260.))
                    .h(LINE_H * CODE_ROWS)
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(t_.border)
                    .rounded(RADIUS_SM)
                    .p(SPACE_1)
                    .text_size(FONT_SMALL);
                for (mi, m) in project.modules().iter().enumerate() {
                    let v = view.clone();
                    let on = d.module == mi && d.procedure.is_none();
                    list = list.child(
                        h_flex()
                            .id(("macro-module", mi))
                            .gap(SPACE_2)
                            .px(SPACE_2)
                            .py(px(2.))
                            .rounded(RADIUS_SM)
                            .when(on, |r| r.bg(t_.selected))
                            .hover(|r| r.bg(t_.hover))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(m.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t_.fg_muted)
                                    .child(kind_label(m.kind)),
                            )
                            .on_click(move |_, window, cx| {
                                let _ =
                                    v.update(cx, |this, cx| this.pick_macro(mi, None, window, cx));
                            }),
                    );
                    let procs = d.procedures.get(mi).map(Vec::as_slice).unwrap_or_default();
                    for (pi, p) in procs.iter().enumerate() {
                        let v = view.clone();
                        let on = d.module == mi && d.procedure == Some(pi);
                        let mut what = p.kind.keyword().to_owned();
                        if p.private {
                            what = format!("Private {what}");
                        }
                        list = list.child(
                            h_flex()
                                .id(("macro-procedure", mi * 10_000 + pi))
                                .gap(SPACE_2)
                                .pl(SPACE_6)
                                .pr(SPACE_2)
                                .py(px(2.))
                                .rounded(RADIUS_SM)
                                .when(on, |r| r.bg(t_.selected))
                                .hover(|r| r.bg(t_.hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(!p.is_macro(), |d| d.text_color(t_.fg_muted))
                                        .child(p.name.clone()),
                                )
                                .child(div().flex_none().text_color(t_.fg_dim).child(what))
                                .on_click(move |_, window, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        this.pick_macro(mi, Some(pi), window, cx)
                                    });
                                }),
                        );
                    }
                }
                // The code, read-only, with line numbers.
                let lines = d.lines.clone();
                let span = d
                    .procedure
                    .and_then(|i| d.procedures.get(d.module)?.get(i))
                    .map(|p| (p.line, p.end_line));
                let digits = lines.len().max(1).to_string().len();
                let gutter = px(9. * digits as f32 + 12.);
                let code = if lines.is_empty() {
                    div()
                        .flex_1()
                        .h(LINE_H * CODE_ROWS)
                        .p(SPACE_4)
                        .border_1()
                        .border_color(t_.border)
                        .rounded(RADIUS_SM)
                        .text_size(FONT_SMALL)
                        .text_color(t_.fg_muted)
                        .child(t("This module has no code."))
                        .into_any_element()
                } else {
                    uniform_list("macros-code", lines.len(), move |range, _, _| {
                        range
                            .map(|i| {
                                let inside = span.is_some_and(|(a, b)| i >= a && i <= b);
                                h_flex()
                                    .id(("macro-line", i))
                                    .h(LINE_H)
                                    .w_full()
                                    .when(inside, |r| r.bg(t_.selected.opacity(0.5)))
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .w(gutter)
                                            .pr(SPACE_3)
                                            .justify_end()
                                            .text_color(t_.fg_dim)
                                            .child(SharedString::from((i + 1).to_string())),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_color(t_.fg)
                                            .child(lines[i].clone()),
                                    )
                            })
                            .collect::<Vec<_>>()
                    })
                    .track_scroll(&d.scroll)
                    .flex_1()
                    .h(LINE_H * CODE_ROWS)
                    .py(SPACE_1)
                    .border_1()
                    .border_color(t_.border)
                    .rounded(RADIUS_SM)
                    .bg(t_.surface)
                    .font_family(mono)
                    .text_size(FONT_SMALL)
                    .into_any_element()
                };
                card = card.child(h_flex().gap(SPACE_4).items_start().child(list).child(code));
            }
        }
        let close = view.clone();
        let card = card
            .child(dialog::note(
                t("Kantor shows macros but doesn't run them yet. Paparan AI can explain a macro or turn it into formulas."),
                false,
                cx,
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new("macros-close")
                        .label(t("Close"))
                        .primary()
                        .small()
                        .on_click(move |_, window, cx| {
                            let _ = close.update(cx, |this, cx| this.close_macros(window, cx));
                        }),
                ),
            );
        Some(dialog::place(card, self, cx).into_any_element())
    }
}
