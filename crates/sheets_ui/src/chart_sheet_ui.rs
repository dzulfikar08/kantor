//! Chart sheets (a tab that is one chart): the bar over the chart.
//!
//! Showing a chart sheet selects its chart, as Excel does, so the Chart
//! panel (`chart_ui`, the same one embedded charts use) edits it; the bar
//! opens the panel. Their tabs share the worksheets' menu (`sheet_tab_ui`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};

use crate::SheetsView;

impl SheetsView {
    /// The first chart of chart sheet `k`, which showing it selects.
    pub(crate) fn chart_sheet_chart(
        &self,
        k: usize,
        cx: &App,
    ) -> Option<sheets_engine::drawing::DrawingKey> {
        self.table
            .read(cx)
            .delegate()
            .book
            .chart_sheet_drawings(k)
            .into_iter()
            .find(|d| d.editable && !d.hidden)
            .map(|d| d.key)
    }

    /// Show the grid's sheet again (the chart sheet shown went away).
    pub(crate) fn leave_chart_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chart_tab = None;
        self.drawings.borrow_mut().selected = None;
        self.focus_grid(window, cx);
        cx.notify();
    }

    /// Focus what shows: the chart sheet or the grid.
    pub(crate) fn refocus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() {
            window.focus(&self.chart_focus, cx);
        } else {
            self.focus_grid(window, cx);
        }
    }

    /// The bar over a chart sheet: its name and the Chart panel button.
    pub(crate) fn render_chart_sheet_bar(
        &self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let open = self.chart_panel.is_some();
        h_flex()
            .flex_none()
            .h(CONTROL_LG)
            .px(SPACE_4)
            .gap(SPACE_3)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .child(div().flex_1().min_w_0().truncate().child(format!(
                "Chart sheet \u{201c}{name}\u{201d} \u{b7} its data is on the worksheets"
            )))
            .child(
                Button::new("chart-sheet-panel")
                    .xsmall()
                    .when_else(open, |b| b.primary(), |b| b.ghost())
                    .icon(Lucide::ChartColumn)
                    .label("Chart")
                    .tooltip("Change the chart's type, titles, legend, colours and data")
                    .on_click(move |_, w, cx| {
                        let _ = view.update(cx, |this, cx| this.chart_options_doc(w, cx));
                    }),
            )
    }
}
