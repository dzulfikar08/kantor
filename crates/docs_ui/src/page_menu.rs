//! Page setup: the toolbar's "Page" control and its popover (paper size,
//! orientation, margin presets, custom margins in cm), plus the `DocsView`
//! methods the palette calls. Every change is one undo step.

use std::time::{Duration, Instant};

use docs_engine::{MarginPreset, Orientation, PageGeometry, PaperSize, cm_to_twips, twips_to_cm};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{Tokens, components::section_label, metrics::*, tokens};

use crate::DocsView;

/// The open popover.
pub(crate) struct PageMenu {
    at: Point<Pixels>,
    /// Top, bottom, left, right, in cm.
    inputs: [Entity<InputState>; 4],
    _subs: Vec<Subscription>,
}

const SIDES: [&str; 4] = [tk("Top"), tk("Bottom"), tk("Left"), tk("Right")];

/// Margin preset names and details from the engine, marked for the catalog.
const MARGIN_TEXT: [&str; 8] = [
    tk("Normal"),
    tk("Narrow"),
    tk("Moderate"),
    tk("Wide"),
    tk("2.54 cm all sides"),
    tk("1.27 cm all sides"),
    tk("2.54 cm top/bottom, 1.91 cm sides"),
    tk("2.54 cm top/bottom, 5.08 cm sides"),
];

fn margin_text(s: &str) -> &str {
    if MARGIN_TEXT.contains(&s) {
        t_dyn(s)
    } else {
        s
    }
}

fn format_cm(tw: u32) -> String {
    let s = format!("{:.2}", twips_to_cm(tw));
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A cm value typed by the user ("2,5" works too).
fn parse_cm(s: &str) -> Option<f64> {
    let v: f64 = s
        .trim()
        .trim_end_matches("cm")
        .trim()
        .replace(',', ".")
        .parse()
        .ok()?;
    (v.is_finite() && v >= 0.0).then_some(v)
}

fn margins(p: &PageGeometry) -> [u32; 4] {
    [p.margin_top, p.margin_bottom, p.margin_left, p.margin_right]
}

/// Short label for the toolbar, e.g. `A4` or `F4 ⟂` (landscape).
pub(crate) fn page_label(p: &PageGeometry) -> String {
    let paper = PaperSize::of(p).map_or(i18n::t("Custom"), |s| match s {
        PaperSize::F4 => "F4",
        s => s.label(),
    });
    match p.orientation() {
        Orientation::Portrait => paper.to_owned(),
        Orientation::Landscape => tf("{} landscape", &[&paper]),
    }
}

impl DocsView {
    /// Page setup of the caret's section.
    pub fn page_geometry(&self) -> PageGeometry {
        self.doc.section_page(self.sel.head.para)
    }

    /// Replace the page setup (one undo step) and repaginate.
    pub fn set_page_doc(
        &mut self,
        page: PageGeometry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.doc
            .set_section_page(self.sel.head.para, page, self.sel);
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.fill_page_inputs(window, cx);
    }

    pub fn set_paper_doc(&mut self, paper: PaperSize, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.page_geometry().with_paper(paper);
        self.set_page_doc(p, window, cx);
    }

    pub fn set_orientation_doc(
        &mut self,
        o: Orientation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self.page_geometry().with_orientation(o);
        self.set_page_doc(p, window, cx);
    }

    pub fn set_margins_doc(
        &mut self,
        m: MarginPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self.page_geometry().with_margins(m);
        self.set_page_doc(p, window, cx);
    }

    /// Open the Page popover at `at` (window coordinates, its top-right
    /// corner), or near the top-right of the page area. Clicking the toolbar
    /// control while it's open closes it.
    pub fn toggle_page_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page_menu.take().is_some()
            || self
                .page_menu_closed
                .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
        {
            self.page_menu_closed = None;
            cx.notify();
            return;
        }
        let at = at.unwrap_or_else(|| {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width - SPACE_4, b.origin.y + SPACE_2)
        });
        let inputs = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));
        let subs = inputs
            .iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.apply_custom_margins(window, cx);
                    }
                })
            })
            .collect();
        self.page_menu = Some(PageMenu {
            at,
            inputs,
            _subs: subs,
        });
        self.fill_page_inputs(window, cx);
        cx.notify();
    }

    /// Show the current margins in the custom-margin fields.
    fn fill_page_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.page_menu else {
            return;
        };
        for (input, tw) in menu.inputs.iter().zip(margins(&self.page_geometry())) {
            input.update(cx, |i, cx| i.set_value(format_cm(tw), window, cx));
        }
    }

    /// Apply the custom-margin fields (fields that don't parse keep their
    /// current value).
    fn apply_custom_margins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.page_menu else {
            return;
        };
        let page = self.page_geometry();
        let mut m = margins(&page);
        for (slot, input) in m.iter_mut().zip(&menu.inputs) {
            if let Some(cm) = parse_cm(&input.read(cx).value()) {
                *slot = cm_to_twips(cm.min(100.0));
            }
        }
        let p = PageGeometry {
            margin_top: m[0],
            margin_bottom: m[1],
            margin_left: m[2],
            margin_right: m[3],
            ..page
        };
        self.set_page_doc(p, window, cx);
    }

    pub(crate) fn render_page_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.page_menu.as_ref()?;
        let t = tokens(cx);
        let page = self.page_geometry();
        let paper = PaperSize::of(&page);
        let sections = self.doc.sections().len();
        let section_note = (sections > 1).then(|| {
            let msg = tk("Section {} of {}: changes apply to this section");
            tf(
                msg,
                &[&(self.doc.section_of(self.sel.head.para) + 1), &sections],
            )
        });
        let margin = MarginPreset::of(&page);
        let orient = page.orientation();
        let view = cx.entity().downgrade();

        type Apply = Box<dyn Fn(&mut DocsView, &mut Window, &mut Context<DocsView>)>;
        let row = |id: SharedString, label: &str, detail: &str, on: bool, apply: Apply| {
            let v = view.clone();
            h_flex()
                .id(id)
                .h(CONTROL_MD)
                .px(SPACE_4)
                .gap(SPACE_4)
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(
                    div()
                        .flex_none()
                        .size(ICON_MD)
                        .when(on, |d| d.child(Icon::new(Lucide::Check).with_size(ICON_MD))),
                )
                .child(div().text_color(t.fg).child(label.to_owned()))
                .child(div().flex_1())
                .child(div().text_color(t.fg_dim).child(detail.to_owned()))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| apply(this, window, cx));
                })
        };

        let mut papers = v_flex();
        for p in PaperSize::ALL {
            papers = papers.child(row(
                format!("docs-paper-{}", p.label()).into(),
                p.label(),
                p.detail(),
                paper == Some(p),
                Box::new(move |v, w, cx| v.set_paper_doc(p, w, cx)),
            ));
        }
        let mut presets = v_flex();
        for m in MarginPreset::ALL {
            presets = presets.child(row(
                format!("docs-margins-{}", m.label()).into(),
                margin_text(m.label()),
                margin_text(m.detail()),
                margin == Some(m),
                Box::new(move |v, w, cx| v.set_margins_doc(m, w, cx)),
            ));
        }
        let segment = |o: Orientation, label: &'static str| {
            let v = view.clone();
            let on = orient == o;
            div()
                .id(label)
                .flex_1()
                .h(CONTROL_SM)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .when(on, |d| d.bg(t.selected).text_color(t.fg))
                .when(!on, |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)))
                .child(i18n::t(label))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.set_orientation_doc(o, window, cx));
                })
        };
        let orientation = h_flex()
            .p(SPACE_1)
            .gap(SPACE_1)
            .rounded(RADIUS_MD)
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .child(segment(Orientation::Portrait, tk("Portrait")))
            .child(segment(Orientation::Landscape, tk("Landscape")));

        let field = |i: usize| {
            v_flex()
                .flex_1()
                .gap(SPACE_1)
                .child(div().text_color(t.fg_muted).child(t_dyn(SIDES[i])))
                .child(Input::new(&menu.inputs[i]).small())
        };
        let apply = {
            let v = view.clone();
            div()
                .id("docs-margins-apply")
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_5)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .bg(t.accent)
                .text_color(t.accent_fg)
                .cursor_pointer()
                .child(i18n::t("Apply"))
                .tooltip(|window, cx| {
                    Tooltip::new(i18n::t("Apply custom margins  Enter")).build(window, cx)
                })
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.apply_custom_margins(window, cx));
                })
        };
        let custom = v_flex()
            .gap(SPACE_3)
            .px(SPACE_4)
            .child(h_flex().gap(SPACE_4).child(field(0)).child(field(1)))
            .child(h_flex().gap(SPACE_4).child(field(2)).child(field(3)))
            .child(h_flex().child(div().flex_1()).child(apply));

        let panel = v_flex()
            .id("docs-page-menu")
            .occlude()
            .w(REM * 20.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.page_menu = None;
                this.page_menu_closed = Some(Instant::now());
                cx.notify();
            }))
            .children(section_note.map(|n| div().px(SPACE_4).text_color(t.fg_muted).child(n)))
            .child(heading(i18n::t("Paper size"), &t, cx))
            .child(papers)
            .child(heading(i18n::t("Orientation"), &t, cx))
            .child(div().px(SPACE_4).child(orientation))
            .child(heading(i18n::t("Margins"), &t, cx))
            .child(presets)
            .child(heading(i18n::t("Custom margins (cm)"), &t, cx))
            .child(custom);
        Some(deferred(
            anchored()
                .position(menu.at)
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

fn heading(text: &'static str, _: &Tokens, cx: &App) -> Div {
    section_label(text, cx).px(SPACE_4).pt(SPACE_2)
}

/// The toolbar's Page control: paper name, opens the popover.
pub(crate) fn page_button(view: &DocsView, cx: &mut Context<DocsView>) -> Stateful<Div> {
    let t = tokens(cx);
    let v = cx.entity().downgrade();
    let open = view.page_menu.is_some();
    h_flex()
        .id("docs-page")
        .flex_none()
        .h(CONTROL_SM)
        .px(SPACE_3)
        .gap(SPACE_2)
        .rounded(RADIUS_MD)
        .text_size(FONT_SMALL)
        .cursor_pointer()
        .when(open, |d| d.bg(t.selected).text_color(t.fg))
        .when(!open, |d| {
            d.text_color(t.fg_muted)
                .hover(|s| s.bg(t.hover).text_color(t.fg))
        })
        .child(Icon::new(Lucide::FileText).with_size(ICON_MD))
        .child(page_label(&view.page_geometry()))
        .tooltip(|window, cx| {
            Tooltip::new(i18n::t("Page setup: size, orientation, margins")).build(window, cx)
        })
        .on_click(move |ev, window, cx| {
            let p = ev.position();
            let at = point(p.x + SPACE_7, p.y + SPACE_7);
            let _ = v.update(cx, |this, cx| this.toggle_page_menu(Some(at), window, cx));
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn cm_fields() {
        assert_eq!(format_cm(1440), "2.54");
        assert_eq!(format_cm(720), "1.27");
        assert_eq!(format_cm(567), "1");
        assert_eq!(parse_cm("2,5"), Some(2.5));
        assert_eq!(parse_cm(" 3 cm"), Some(3.0));
        assert_eq!(parse_cm("-1"), None);
        assert_eq!(parse_cm("abc"), None);
        assert_eq!(page_label(&PageGeometry::default()), "A4");
        let f4 = PageGeometry::default()
            .with_paper(PaperSize::F4)
            .with_orientation(Orientation::Landscape);
        assert_eq!(page_label(&f4), "F4 landscape");
    }
}
