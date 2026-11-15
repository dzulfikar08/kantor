//! Move Chart (the Chart panel's button, the command palette), as Excel's
//! dialog: "Choose where you want the chart to be placed: New sheet:
//! [Chart1] / Object in: [sheet]". One undo step
//! (`sheets_engine::move_chart`); the chart's new place is shown.

use gpui_kit::component::{
    Sizable as _,
    button::Button,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    radio::Radio,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, t_dyn};
use kantor_ui::metrics::*;
use sheets_engine::drawing::DrawingKey;
use sheets_engine::{ChartPlace, MovedChart};

use crate::SheetsView;
use crate::dialog::{self, note};

/// The open Move Chart dialog.
pub(crate) struct MoveChartDialog {
    key: DrawingKey,
    /// "New sheet" (else "Object in").
    new_sheet: bool,
    name: Entity<InputState>,
    /// The worksheet "Object in" names (an index).
    sheet: u32,
    error: Option<String>,
    _sub: Subscription,
}

impl SheetsView {
    /// Open Move Chart for the selected chart.
    pub fn move_chart_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let key = self
            .drawings
            .borrow()
            .selected
            .or_else(|| self.chart_panel.as_ref().and_then(|p| p.loaded_key()));
        let book = &self.table.read(cx).delegate().book;
        let Some(key) = key.filter(|k| book.chart_info(*k).is_some()) else {
            self.status = t("Select a chart to move it.").into();
            cx.notify();
            return;
        };
        let on_chart_sheet = sheets_engine::drawing::chart_sheet_of_key(key.sheet_id);
        let suggested = match on_chart_sheet {
            Some(k) => book.chart_sheet_name(k).unwrap_or_default(),
            None => book.next_chart_sheet_name(),
        };
        // "Object in" starts on the chart's own sheet (else the first shown).
        let sheet = match on_chart_sheet {
            None => (0..book.sheet_names().len() as u32)
                .find(|&s| book.sheet_id(s) == key.sheet_id)
                .unwrap_or(0),
            Some(_) => book
                .tabs()
                .into_iter()
                .find_map(|t| match t {
                    sheets_engine::Tab::Sheet(i) => Some(i),
                    _ => None,
                })
                .unwrap_or(0),
        };
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(suggested, window, cx);
            s
        });
        let sub = cx.subscribe_in(&name, window, |this, _, e: &InputEvent, w, cx| match e {
            InputEvent::PressEnter { .. } => this.apply_move_chart(w, cx),
            InputEvent::Focus => {
                if let Some(d) = this.move_chart.as_mut() {
                    d.new_sheet = true;
                }
                cx.notify();
            }
            _ => {}
        });
        self.move_chart = Some(MoveChartDialog {
            key,
            new_sheet: true,
            name,
            sheet,
            error: None,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_move_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.move_chart.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn apply_move_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.move_chart.as_ref() else {
            return;
        };
        let key = d.key;
        let to = if d.new_sheet {
            ChartPlace::NewSheet(d.name.read(cx).value().trim().to_owned())
        } else {
            ChartPlace::ObjectIn(d.sheet)
        };
        let mut moved = None;
        let res = self.tab_edit(t("Chart moved").into(), window, cx, |b| {
            moved = Some(b.move_chart(key, to)?);
            Ok(())
        });
        match res {
            Err(e) => {
                if let Some(d) = self.move_chart.as_mut() {
                    d.error = Some(e);
                }
                cx.notify();
            }
            Ok(()) => {
                self.move_chart = None;
                match moved {
                    Some(MovedChart::ChartSheet(k)) => self.select_chart_sheet(k, window, cx),
                    Some(MovedChart::Object { sheet, key }) => {
                        if self.chart_tab.is_some() {
                            self.leave_chart_sheet(window, cx);
                        }
                        self.select_sheet(sheet, window, cx);
                        self.drawings.borrow_mut().selected = Some(key);
                        self.focus_grid(window, cx);
                    }
                    None => {}
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn render_move_chart(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.move_chart.as_ref()?;
        let view = cx.entity().downgrade();
        let book = &self.table.read(cx).delegate().book;
        let names = book.sheet_names();
        let sheets: Vec<(u32, String)> = book
            .tabs()
            .into_iter()
            .filter_map(|t| match t {
                sheets_engine::Tab::Sheet(i) => Some((i, names[i as usize].clone())),
                _ => None,
            })
            .collect();
        let current = names.get(d.sheet as usize).cloned().unwrap_or_default();
        let choose = |id: &'static str, label: &'static str, on: bool, new_sheet: bool| {
            let v = view.clone();
            Radio::new(id)
                .label(label)
                .checked(on)
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(d) = this.move_chart.as_mut() {
                            d.new_sheet = new_sheet;
                        }
                        cx.notify();
                    });
                })
        };
        let sv = view.clone();
        let picked = d.sheet;
        let sheet_button = Button::new("move-chart-sheet")
            .outline()
            .small()
            .label(current)
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                for (i, name) in &sheets {
                    let v = sv.clone();
                    let i = *i;
                    m = m.item(
                        PopupMenuItem::new(name.clone())
                            .checked(i == picked)
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(d) = this.move_chart.as_mut() {
                                        d.sheet = i;
                                        d.new_sheet = false;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                m
            });
        let row = |choice: Radio, control: AnyElement| {
            h_flex()
                .gap(SPACE_4)
                .child(div().w(px(110.)).flex_none().child(choice))
                .child(div().flex_1().min_w_0().child(control))
        };
        let card = dialog::card("move-chart", t("Move Chart"), px(420.), cx)
            .key_context("KantorMoveChart")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_move_chart(w, cx)))
            .child(div().child(t("Choose where you want the chart to be placed:")))
            .child(
                v_flex()
                    .gap(SPACE_3)
                    .child(row(
                        choose("move-chart-new", t("New sheet:"), d.new_sheet, true),
                        Input::new(&d.name).small().into_any_element(),
                    ))
                    .child(row(
                        choose("move-chart-object", t("Object in:"), !d.new_sheet, false),
                        sheet_button.into_any_element(),
                    )),
            )
            .children(
                d.error
                    .clone()
                    .map(|e| note(t_dyn(&e).to_owned(), true, cx)),
            )
            .child(dialog::buttons(
                "move-chart-buttons",
                t("OK"),
                true,
                &view,
                Self::apply_move_chart,
                Self::close_move_chart,
            ));
        Some(dialog::place(card).into_any_element())
    }
}
