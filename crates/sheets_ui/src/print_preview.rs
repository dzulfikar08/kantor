//! The print preview beside Page Setup and Print: the sheet's pages exactly
//! as Export as PDF writes them. The pages are planned with the card's
//! settings as they are typed (not applied yet), the page on screen is read
//! from the workbook, and a background task writes it as a one-page PDF
//! with the export's own encoder and rasterizes that
//! (`sheets_pdf::preview`). Only the page shown is drawn, so a 50-page
//! sheet costs no more than a one-page one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::SheetPrint;
use sheets_pdf::preview::{Raster, Rasterizer};
use sheets_pdf::{Options, Plan, Shaper};

use crate::SheetsView;

/// The preview's box, logical px.
pub(crate) const BOX_W: f32 = 380.;
pub(crate) const BOX_H: f32 = 440.;
/// Room around the page in the box.
const PAD: f32 = 12.;
/// Zoom steps, percent of actual size (100 %: a point is 96/72 px).
const ZOOMS: [u32; 7] = [50, 75, 100, 125, 150, 200, 300];
/// Settings typed this long ago are previewed (typing stays smooth).
const SETTLE: Duration = Duration::from_millis(150);

/// What a rendered page shows.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Key {
    generation: u64,
    page: usize,
    zoom: Zoom,
    /// Device pixels per logical pixel.
    window_scale: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Zoom {
    /// The whole page in the box.
    Fit,
    Percent(u32),
}

/// Logical pixels per point for a `page_pt` page at `zoom` in the box.
pub(crate) fn px_per_pt(zoom: Zoom, page_pt: (f32, f32)) -> f32 {
    match zoom {
        Zoom::Fit => {
            let (w, h) = (page_pt.0.max(1.), page_pt.1.max(1.));
            ((BOX_W - 2. * PAD) / w).min((BOX_H - 2. * PAD) / h)
        }
        Zoom::Percent(p) => p as f32 / 100. * 96. / 72.,
    }
}

/// The next zoom step in (`up`) or out from `zoom`, given what Fit is in
/// percent.
pub(crate) fn step_zoom(zoom: Zoom, fit_percent: f32, up: bool) -> Zoom {
    let now = match zoom {
        Zoom::Fit => fit_percent,
        Zoom::Percent(p) => p as f32,
    };
    let next = if up {
        ZOOMS.iter().find(|&&z| z as f32 > now + 0.5)
    } else {
        ZOOMS.iter().rev().find(|&&z| (z as f32) < now - 0.5)
    };
    match next {
        Some(&z) => Zoom::Percent(z),
        None => zoom,
    }
}

pub(crate) struct Preview {
    /// The settings `plan` was made with, and those waiting to be planned.
    planned: Option<SheetPrint>,
    wanted: Option<SheetPrint>,
    plan: Option<Arc<Plan>>,
    /// Measures wrapped text while planning (made once: it scans fonts).
    shaper: Option<Shaper>,
    generation: u64,
    page: usize,
    zoom: Zoom,
    /// The page drawn, with its logical size.
    shown: Option<(Key, Arc<RenderImage>, Size<Pixels>)>,
    /// The page being drawn, and the newest request's number (older
    /// requests still queued are dropped).
    pending: Option<Key>,
    latest: Arc<AtomicU64>,
    rasterizer: Arc<Mutex<Rasterizer>>,
    error: Option<String>,
    /// Images replaced, to free on the next frame.
    retired: Vec<Arc<RenderImage>>,
    _settle: Option<Task<()>>,
    _render: Option<Task<()>>,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            planned: None,
            wanted: None,
            plan: None,
            shaper: None,
            generation: 0,
            page: 0,
            zoom: Zoom::Fit,
            shown: None,
            pending: None,
            latest: Arc::default(),
            rasterizer: Arc::default(),
            error: None,
            retired: Vec::new(),
            _settle: None,
            _render: None,
        }
    }
}

impl Preview {
    fn pages(&self) -> usize {
        self.plan.as_ref().map_or(0, |p| p.pages.len())
    }

    /// The current page's size, points.
    fn page_pt(&self) -> Option<(f32, f32)> {
        let plan = self.plan.as_ref()?;
        let page = plan.pages.get(self.page)?;
        let s = &plan.sheets[page.sheet];
        Some((s.page.0 as f32, s.page.1 as f32))
    }

    fn fit_percent(&self) -> f32 {
        let pt = self.page_pt().unwrap_or((612., 792.));
        px_per_pt(Zoom::Fit, pt) * 72. / 96. * 100.
    }

    /// Free the images (the card closes).
    pub(crate) fn release(&mut self, window: &mut Window) {
        if let Some((_, img, _)) = self.shown.take() {
            self.retired.push(img);
        }
        for img in self.retired.drain(..) {
            let _ = window.drop_image(img);
        }
    }
}

impl SheetsView {
    /// Keep the preview up to date with the card's settings: called each
    /// frame while Page Setup or Print is open.
    pub(crate) fn sync_print_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        for img in d.preview.retired.drain(..) {
            let _ = window.drop_image(img);
        }
        let Ok(settings) = d.collect(cx) else {
            return; // a field being typed: keep the last preview
        };
        let p = &mut d.preview;
        let window_scale = window.scale_factor().to_bits();
        if p.shown
            .as_ref()
            .is_some_and(|(k, _, _)| k.window_scale != window_scale)
            && p.pending.is_none()
        {
            // Moved to a screen with another scale: draw it sharp there.
            cx.defer_in(window, |this, window, cx| {
                this.draw_preview_page(window, cx)
            });
        }
        if p.planned.as_ref() == Some(&settings) || p.wanted.as_ref() == Some(&settings) {
            return;
        }
        let first = p.planned.is_none() && p.wanted.is_none();
        p.wanted = Some(settings);
        let delay = if first { Duration::ZERO } else { SETTLE };
        p._settle = Some(cx.spawn_in(window, async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = this.update_in(cx, |this, window, cx| this.plan_preview(window, cx));
        }));
    }

    /// Plan the pages with the settings waiting, then draw the page shown.
    fn plan_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        let Some(settings) = d.preview.wanted.take() else {
            return;
        };
        let sheet = d.sheet;
        let mut shaper = d.preview.shaper.take().unwrap_or_default();
        let plan = {
            let book = &self.table.read(cx).delegate().book;
            let opts = Options {
                sheets: Some(vec![sheet]),
                print: Some((sheet, settings.clone())),
                ..Options::default()
            }
            .now()
            .with_path(book.source_path());
            Plan::new(book, &opts, &mut shaper)
        };
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        let p = &mut d.preview;
        p.shaper = Some(shaper);
        p.planned = Some(settings);
        p.generation += 1;
        p.page = p.page.min(plan.pages.len().saturating_sub(1));
        p.plan = Some(Arc::new(plan));
        p.error = None;
        self.draw_preview_page(window, cx);
    }

    /// Show page `page` (0-based) of the preview.
    pub(crate) fn preview_go(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        let last = d.preview.pages().saturating_sub(1);
        d.preview.page = page.min(last);
        self.draw_preview_page(window, cx);
    }

    pub(crate) fn preview_zoom(&mut self, zoom: Zoom, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.page_setup.as_mut() {
            d.preview.zoom = zoom;
        }
        self.draw_preview_page(window, cx);
    }

    /// Read the page shown from the workbook and draw it in the background
    /// (unless it is drawn already or being drawn).
    fn draw_preview_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let window_scale = window.scale_factor();
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        let p = &mut d.preview;
        let (Some(plan), Some(pt)) = (p.plan.clone(), p.page_pt()) else {
            cx.notify();
            return;
        };
        let key = Key {
            generation: p.generation,
            page: p.page,
            zoom: p.zoom,
            window_scale: window_scale.to_bits(),
        };
        if p.shown.as_ref().is_some_and(|(k, _, _)| *k == key) || p.pending == Some(key) {
            cx.notify();
            return;
        }
        let logical = px_per_pt(p.zoom, pt);
        let scale = logical * window_scale.max(0.5);
        p.pending = Some(key);
        let id = p.latest.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = p.latest.clone();
        let rasterizer = p.rasterizer.clone();
        let index = p.page;
        let content = self.table.update(cx, |t, _| {
            sheets_pdf::extract(&mut t.delegate_mut().book, &plan, index)
        });
        let size = size(px(pt.0 * logical), px(pt.1 * logical));
        let task = cx.background_spawn(async move {
            let mut r = rasterizer.lock().unwrap_or_else(|e| e.into_inner());
            // A newer page was asked for while this one waited.
            if latest.load(Ordering::SeqCst) != id {
                return None;
            }
            Some(r.render(&content, scale))
        });
        let Some(d) = self.page_setup.as_mut() else {
            return;
        };
        d.preview._render = Some(cx.spawn_in(window, async move |this, cx| {
            let Some(result) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let Some(d) = this.page_setup.as_mut() else {
                    return;
                };
                let p = &mut d.preview;
                if p.pending != Some(key) {
                    return;
                }
                p.pending = None;
                match result.map(to_image) {
                    Ok(Some(img)) => {
                        if let Some((_, old, _)) = p.shown.replace((key, img, size)) {
                            p.retired.push(old);
                        }
                        p.error = None;
                    }
                    Ok(None) => p.error = Some("The page could not be drawn".into()),
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The preview pane: the page, then page and zoom controls.
    pub(crate) fn render_print_preview(&self, cx: &mut Context<Self>) -> Option<Div> {
        let d = self.page_setup.as_ref()?;
        let p = &d.preview;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let pages = p.pages();
        let page = p.page;
        let image = p.shown.as_ref().map(|(_, image, size)| {
            img(ImageSource::Render(image.clone()))
                .w(size.width)
                .h(size.height)
        });
        let status: Option<SharedString> = if let Some(e) = &p.error {
            Some(e.clone().into())
        } else if p.plan.is_some() && pages == 0 {
            Some("Nothing to print on this sheet".into())
        } else if p.shown.is_none() {
            Some("Drawing the page…".into())
        } else {
            None
        };
        let fit_percent = p.fit_percent();
        let zoom_label: SharedString = match p.zoom {
            Zoom::Fit => format!("Fit ({fit_percent:.0}%)").into(),
            Zoom::Percent(z) => format!("{z}%").into(),
        };
        let nav = |id: &'static str, icon: Lucide, tip: &'static str, to: Option<usize>| {
            let view = view.clone();
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .disabled(to.is_none())
                .on_click(move |_, window, cx| {
                    if let Some(to) = to {
                        let _ = view.update(cx, |v, cx| v.preview_go(to, window, cx));
                    }
                })
        };
        let zoom_button = |id: &'static str, icon: Lucide, tip: &'static str, up: bool| {
            let view = view.clone();
            let next = step_zoom(p.zoom, fit_percent, up);
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tip)
                .disabled(next == p.zoom)
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |v, cx| v.preview_zoom(next, window, cx));
                })
        };
        let fit_view = view.clone();
        let page_box = div()
            .id("print-preview-box")
            .w(px(BOX_W))
            .h(px(BOX_H))
            .flex_none()
            .rounded(RADIUS_SM)
            .bg(t.chrome)
            .border_1()
            .border_color(t.border)
            .overflow_scroll()
            .child(
                // Auto margins centre the page while it fits and let a
                // zoomed-in page scroll from its top-left edge.
                v_flex()
                    .min_w_full()
                    .min_h_full()
                    .p(px(PAD))
                    .children(image.map(|i| div().flex_none().m_auto().shadow_md().child(i)))
                    .children(status.map(|s| {
                        div()
                            .m_auto()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(s)
                    })),
            );
        let page_label: SharedString = if pages == 0 {
            "No pages".into()
        } else {
            format!("Page {} of {pages}", page + 1).into()
        };
        Some(
            v_flex().flex_none().gap(SPACE_3).child(page_box).child(
                h_flex()
                    .justify_between()
                    .text_size(FONT_SMALL)
                    .child(
                        h_flex()
                            .gap(SPACE_1)
                            .items_center()
                            .child(nav(
                                "pp-prev",
                                Lucide::ChevronLeft,
                                "Previous page",
                                page.checked_sub(1),
                            ))
                            .child(div().min_w(px(92.)).text_center().child(page_label))
                            .child(nav(
                                "pp-next",
                                Lucide::ChevronRight,
                                "Next page",
                                (page + 1 < pages).then_some(page + 1),
                            )),
                    )
                    .child(
                        h_flex()
                            .gap(SPACE_1)
                            .items_center()
                            .child(zoom_button(
                                "pp-zoom-out",
                                Lucide::ZoomOut,
                                "Zoom out",
                                false,
                            ))
                            .child(
                                Button::new("pp-zoom-fit")
                                    .ghost()
                                    .xsmall()
                                    .label(zoom_label)
                                    .tooltip("Fit the page")
                                    .on_click(move |_, window, cx| {
                                        let _ = fit_view.update(cx, |v, cx| {
                                            v.preview_zoom(Zoom::Fit, window, cx)
                                        });
                                    }),
                            )
                            .child(zoom_button("pp-zoom-in", Lucide::ZoomIn, "Zoom in", true)),
                    ),
            ),
        )
    }
}

/// A raster as an image GPUI can paint (BGRA).
fn to_image(r: Raster) -> Option<Arc<RenderImage>> {
    let mut bytes = r.rgba;
    for c in bytes.as_chunks_mut::<4>().0 {
        c.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(r.width, r.height, bytes)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn zoom_sizes() {
        // A4 portrait fits the box by its height.
        let a4 = (595.0, 842.0);
        let fit = px_per_pt(Zoom::Fit, a4);
        assert!((842.0 * fit - (BOX_H - 2. * PAD)).abs() < 0.01);
        assert!(595.0 * fit <= BOX_W - 2. * PAD);
        // Landscape fits by its width.
        let fit_l = px_per_pt(Zoom::Fit, (842.0, 595.0));
        assert!((842.0 * fit_l - (BOX_W - 2. * PAD)).abs() < 0.01);
        // 100 % is actual size.
        assert!((px_per_pt(Zoom::Percent(100), a4) - 96. / 72.).abs() < 1e-6);
    }

    #[std::prelude::v1::test]
    fn zoom_steps() {
        // From Fit (~51 %) in: 75 %; out: 50 %.
        assert_eq!(step_zoom(Zoom::Fit, 51.0, true), Zoom::Percent(75));
        assert_eq!(step_zoom(Zoom::Fit, 51.0, false), Zoom::Percent(50));
        assert_eq!(
            step_zoom(Zoom::Percent(100), 51.0, true),
            Zoom::Percent(125)
        );
        assert_eq!(
            step_zoom(Zoom::Percent(300), 51.0, true),
            Zoom::Percent(300)
        );
        assert_eq!(step_zoom(Zoom::Percent(50), 51.0, false), Zoom::Percent(50));
    }
}
