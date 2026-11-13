//! Home: the start screen shown when no tab is active.

use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{kbd, section_label};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::{AppKind, Workspace};

impl Workspace {
    pub(super) fn render_card(
        &self,
        id: &'static str,
        app: AppKind,
        subtitle: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = tokens(cx);
        let (accent, ready) = match app {
            AppKind::Docs => (t.docs, true),
            AppKind::Sheets => (t.sheets, true),
            AppKind::Slides => (t.slides, true),
        };
        let entity = cx.entity().downgrade();
        v_flex()
            .id(id)
            .w(px(208.))
            .p(SPACE_7)
            .gap(SPACE_4)
            .rounded(RADIUS_XL)
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .when(ready, |d| {
                d.cursor_pointer()
                    .hover(|s| s.border_color(t.border_strong).bg(t.elevated))
            })
            .child(div().w(px(28.)).h(px(4.)).rounded(px(2.)).bg(accent))
            .child(
                div()
                    .text_size(FONT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(app.name()),
            )
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(subtitle),
            )
            .child(
                div()
                    .text_size(FONT_CAPTION)
                    .text_color(if ready { accent } else { t.fg_dim })
                    .child(if ready { "New  →" } else { "Coming soon" }),
            )
            .on_click(move |_, window, cx| {
                let _ = entity.update(cx, |ws, cx| ws.launch(app, None, window, cx));
            })
    }

    pub(super) fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let docs = self.render_card("home-docs", AppKind::Docs, "Write .docx documents", cx);
        let sheets = self.render_card(
            "home-sheets",
            AppKind::Sheets,
            "Edit .xlsx spreadsheets",
            cx,
        );
        let slides = self.render_card("home-slides", AppKind::Slides, "Present .pptx decks", cx);
        let hints: Vec<Div> = [
            ("Ctrl N", "New spreadsheet"),
            ("Ctrl Shift N", "New document"),
            ("Ctrl Alt N", "New presentation"),
            ("Ctrl O", "Open file"),
            ("Ctrl K", "All commands"),
        ]
        .into_iter()
        .map(|(keys, label)| h_flex().gap(SPACE_3).child(kbd(keys, cx)).child(label))
        .collect();
        let create = section_label("Create", cx);

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(SPACE_9)
            .bg(t.bg)
            .child(img(self.logo.clone()).size(px(88.)))
            .child(
                v_flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .text_size(FONT_DISPLAY)
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.fg)
                            .child("Kantor"),
                    )
                    .child(
                        div()
                            .text_size(FONT_BODY)
                            .text_color(t.fg_muted)
                            .child("Docs, Sheets and Slides — native, fast, yours."),
                    ),
            )
            .child(
                v_flex().gap(SPACE_4).child(create).child(
                    h_flex()
                        .gap(SPACE_6)
                        .child(docs)
                        .child(sheets)
                        .child(slides),
                ),
            )
            .child(
                h_flex()
                    .gap(SPACE_8)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .children(hints),
            )
            .child({
                let entity = cx.entity().downgrade();
                h_flex()
                    .id("home-cloud")
                    .gap(SPACE_3)
                    .px(SPACE_5)
                    .py(SPACE_3)
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover).border_color(t.border_strong))
                    .text_size(FONT_SMALL)
                    .text_color(t.fg)
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Cloud)
                            .with_size(ICON_MD)
                            .text_color(t.accent),
                    )
                    .child(kantor_ui::i18n::tr("Open from Cloud…", "Buka dari Cloud…"))
                    .on_click(move |_, window, cx| {
                        let _ = entity.update(cx, |ws, cx| {
                            ws.open_from_cloud(&crate::actions::OpenFromCloud, window, cx)
                        });
                    })
            })
            .child(
                div()
                    .h(px(18.))
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(self.message.clone()),
            )
    }
}
