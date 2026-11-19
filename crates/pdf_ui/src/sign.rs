//! Fill & Sign: signatures drawn, typed or from a picture, image stamps
//! (a company stamp PNG), *My Stamps* (kept in `~/.config/kantor/stamps`,
//! never uploaded), the page's right-click menu, dropping a picture on a
//! page, Place on Pages, and the bar about forms and digital signatures.
//!
//! These signatures are pictures on the page (Ink and Stamp annotations),
//! not certificate-based digital signatures; the dialog says so.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pdf::annot::Seg;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};

use crate::PdfView;
use crate::markup::{Placed, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignTab {
    Draw,
    Type,
    Image,
}

pub(crate) enum SignDialog {
    /// Make a signature.
    Signature {
        tab: SignTab,
        /// Strokes drawn on the pad, px in the pad.
        strokes: Vec<Vec<[f32; 2]>>,
        drawing: bool,
        pad: Rc<Cell<Bounds<Pixels>>>,
        typed: Entity<InputState>,
        /// The typed name as outlines (made in the background).
        outlines: Option<(String, Arc<Vec<Seg>>, f32)>,
        remember: bool,
        _sub: Subscription,
    },
    /// Place the selected stamp on other pages.
    PlacePages {
        pages: Entity<InputState>,
        error: Option<String>,
    },
}

// ── My Stamps ──────────────────────────────────────────────────────────────

/// A remembered signature or stamp.
#[derive(Clone, Debug)]
pub(crate) struct Saved {
    pub path: PathBuf,
    pub what: Placed,
    pub signature: bool,
}

/// Where My Stamps live.
pub(crate) fn stamps_dir() -> Option<PathBuf> {
    kantor_core::settings::config_dir().map(|d| d.join("stamps"))
}

fn stamp_name(ext: &str) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{t}.{ext}")
}

/// Remember `what` (a copy of the picture, or the strokes / typed name).
pub(crate) fn remember(what: &Placed, typed: Option<&str>, signature: bool) -> std::io::Result<()> {
    let Some(dir) = stamps_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir)?;
    let kind = if signature { "sig" } else { "stamp" };
    match what {
        Placed::Image { bytes, .. } => {
            // The same picture once.
            for s in load_saved() {
                if let Placed::Image { bytes: b, .. } = &s.what
                    && b == bytes
                {
                    return Ok(());
                }
            }
            let ext = if bytes.starts_with(b"\x89PNG") {
                "png"
            } else {
                "jpg"
            };
            std::fs::write(
                dir.join(stamp_name(&format!("{kind}.{ext}"))),
                bytes.as_slice(),
            )
        }
        Placed::Ink { strokes, aspect } => {
            let mut s = format!("{aspect}\n");
            for st in strokes {
                let line: Vec<String> = st.iter().map(|p| format!("{},{}", p[0], p[1])).collect();
                s.push_str(&line.join(" "));
                s.push('\n');
            }
            std::fs::write(dir.join(stamp_name(&format!("{kind}.ink"))), s)
        }
        Placed::Typed { .. } => match typed {
            Some(t) => std::fs::write(dir.join(stamp_name(&format!("{kind}.typed"))), t),
            None => Ok(()),
        },
    }
}

/// My Stamps, oldest first.
pub(crate) fn load_saved() -> Vec<Saved> {
    let Some(dir) = stamps_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let signature = name.contains(".sig.");
            let what = if name.ends_with(".png") || name.ends_with(".jpg") {
                let bytes = std::fs::read(&path).ok()?;
                image_placed(bytes).ok()?
            } else if name.ends_with(".ink") {
                let text = std::fs::read_to_string(&path).ok()?;
                let mut lines = text.lines();
                let aspect: f32 = lines.next()?.trim().parse().ok()?;
                let strokes = lines
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| {
                        l.split_whitespace()
                            .filter_map(|p| {
                                let (x, y) = p.split_once(',')?;
                                Some([x.parse().ok()?, y.parse().ok()?])
                            })
                            .collect()
                    })
                    .collect();
                Placed::Ink { strokes, aspect }
            } else if name.ends_with(".typed") {
                let text = std::fs::read_to_string(&path).ok()?;
                let (segs, aspect) = typed_outlines(text.trim())?;
                Placed::Typed {
                    segs: Arc::new(segs),
                    aspect,
                }
            } else {
                return None;
            };
            Some(Saved {
                path,
                what,
                signature,
            })
        })
        .collect()
}

/// A PNG or JPEG file as something to place.
pub(crate) fn image_placed(bytes: Vec<u8>) -> Result<Placed, String> {
    let size = imagesize::blob_size(&bytes).map_err(|e| e.to_string())?;
    match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        _ => return Err(i18n::t("Only PNG and JPEG pictures can be placed").into()),
    }
    Ok(Placed::Image {
        bytes: Arc::new(bytes),
        size: (size.width as u32, size.height as u32),
    })
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

// ── typed signatures ───────────────────────────────────────────────────────

/// The system's fonts (read once, in the background).
fn fonts() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Script-like families tried for a typed signature, then any italic.
const SCRIPT_FONTS: &[&str] = &[
    "Dancing Script",
    "Great Vibes",
    "Allura",
    "Satisfy",
    "Pacifico",
    "Z003",
    "URW Chancery L",
    "Brush Script MT",
    "Segoe Script",
    "Snell Roundhand",
    "Apple Chancery",
    "Lucida Handwriting",
];

struct Outline {
    segs: Vec<Seg>,
    dx: f32,
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Move(x + self.dx, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Line(x + self.dx, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // As a cubic from the current point.
        let (px0, py0) = match self.segs.last() {
            Some(Seg::Move(a, b) | Seg::Line(a, b) | Seg::Curve(_, _, _, _, a, b)) => (*a, *b),
            _ => (x1 + self.dx, y1),
        };
        let (qx, qy) = (x1 + self.dx, y1);
        let (ex, ey) = (x + self.dx, y);
        self.segs.push(Seg::Curve(
            px0 + 2.0 / 3.0 * (qx - px0),
            py0 + 2.0 / 3.0 * (qy - py0),
            ex + 2.0 / 3.0 * (qx - ex),
            ey + 2.0 / 3.0 * (qy - ey),
            ex,
            ey,
        ));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let d = self.dx;
        self.segs.push(Seg::Curve(x1 + d, y1, x2 + d, y2, x + d, y));
    }
    fn close(&mut self) {
        self.segs.push(Seg::Close);
    }
}

/// `text` in a script font as outlines in a 0..1 box (y up), and the box's
/// width / height.
pub(crate) fn typed_outlines(text: &str) -> Option<(Vec<Seg>, f32)> {
    if text.trim().is_empty() {
        return None;
    }
    let db = fonts();
    let mut families: Vec<fontdb::Family> = SCRIPT_FONTS
        .iter()
        .map(|f| fontdb::Family::Name(f))
        .collect();
    families.push(fontdb::Family::Cursive);
    families.push(fontdb::Family::Serif);
    let id = db.query(&fontdb::Query {
        families: &families,
        style: fontdb::Style::Italic,
        ..Default::default()
    })?;
    let segs = db.with_face_data(id, |data, index| {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let mut o = Outline {
            segs: Vec::new(),
            dx: 0.0,
        };
        for ch in text.chars() {
            let Some(g) = face.glyph_index(ch) else {
                o.dx += f32::from(face.units_per_em()) * 0.3;
                continue;
            };
            face.outline_glyph(g, &mut o);
            o.dx += f32::from(face.glyph_hor_advance(g).unwrap_or(0));
        }
        Some(o.segs)
    })??;
    // Normalize to the outlines' bounds.
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    let mut grow = |x: f32, y: f32| b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    for s in &segs {
        match *s {
            Seg::Move(x, y) | Seg::Line(x, y) => grow(x, y),
            Seg::Curve(a, c, d, e, x, y) => {
                grow(a, c);
                grow(d, e);
                grow(x, y);
            }
            Seg::Close => {}
        }
    }
    if b[0] >= b[2] || b[1] >= b[3] {
        return None;
    }
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let n = |x: f32, y: f32| ((x - b[0]) / w, (y - b[1]) / h);
    let segs = segs
        .into_iter()
        .map(|s| match s {
            Seg::Move(x, y) => {
                let (x, y) = n(x, y);
                Seg::Move(x, y)
            }
            Seg::Line(x, y) => {
                let (x, y) = n(x, y);
                Seg::Line(x, y)
            }
            Seg::Curve(a, c, d, e, x, y) => {
                let (a, c) = n(a, c);
                let (d, e) = n(d, e);
                let (x, y) = n(x, y);
                Seg::Curve(a, c, d, e, x, y)
            }
            Seg::Close => Seg::Close,
        })
        .collect();
    Some((segs, w / h))
}

/// Paint a placeable item into `b` (window px): strokes, outlines, or a
/// placeholder frame for pictures (shown by an img element instead).
fn paint_placed(window: &mut Window, b: Bounds<Pixels>, what: &Placed, color: Hsla) {
    let fit = |aspect: f32| {
        let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
        let (w, h) = if bw / bh > aspect {
            (bh * aspect, bh)
        } else {
            (bw, bw / aspect)
        };
        let x = f32::from(b.origin.x) + (bw - w) / 2.0;
        let y = f32::from(b.origin.y) + (bh - h) / 2.0;
        move |p: [f32; 2]| point(px(x + p[0] * w), px(y + (1.0 - p[1]) * h))
    };
    match what {
        Placed::Ink { strokes, aspect } => {
            let f = fit(*aspect);
            for s in strokes {
                let Some(first) = s.first() else { continue };
                let mut pb = PathBuilder::stroke(px(2.));
                pb.move_to(f(*first));
                for p in &s[1..] {
                    pb.line_to(f(*p));
                }
                if let Ok(p) = pb.build() {
                    window.paint_path(p, color);
                }
            }
        }
        Placed::Typed { segs, aspect } => {
            let f = fit(*aspect);
            let mut pb = PathBuilder::fill();
            for s in segs.iter() {
                match *s {
                    Seg::Move(x, y) => pb.move_to(f([x, y])),
                    Seg::Line(x, y) => pb.line_to(f([x, y])),
                    Seg::Curve(a, c, d, e, x, y) => {
                        pb.cubic_bezier_to(f([x, y]), f([a, c]), f([d, e]))
                    }
                    Seg::Close => pb.close(),
                }
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
        }
        Placed::Image { .. } => {}
    }
}

impl PdfView {
    // ── adding pictures ────────────────────────────────────────────────────

    /// Add Image / Stamp…: pick a PNG or JPEG; it goes where the page menu
    /// was opened, else where the user clicks next. `path`: a file already
    /// chosen.
    pub fn add_image_stamp(
        &mut self,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page_count() == 0 {
            return;
        }
        let at = self.context_at.take();
        if let Some(p) = path {
            self.stamp_from_file(&p, at, cx);
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose a PNG or JPEG picture").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.stamp_from_file(&p, at, cx));
        })
        .detach();
    }

    fn stamp_from_file(&mut self, p: &Path, at: Option<(usize, [f32; 2])>, cx: &mut Context<Self>) {
        let placed = std::fs::read(p)
            .map_err(|e| e.to_string())
            .and_then(image_placed);
        match placed {
            Ok(what) => {
                if let Err(e) = remember(&what, None, false) {
                    log::warn!("keeping the stamp in My Stamps: {e}");
                }
                self.use_placed(what, false, at, cx);
            }
            Err(e) => {
                self.status = tf("Couldn't use {}: {}", &[&p.display(), &e]).into();
                cx.notify();
            }
        }
    }

    /// Put `what` at `at`, or arm the Place tool for the next click.
    pub(crate) fn use_placed(
        &mut self,
        what: Placed,
        signature: bool,
        at: Option<(usize, [f32; 2])>,
        cx: &mut Context<Self>,
    ) {
        if self.markup.mode == crate::Mode::View {
            self.markup.mode = crate::Mode::FillSign;
        }
        self.load_all_marks(cx);
        match at {
            Some((place, pt)) => self.place_item(place, pt, &what, signature, cx),
            None => {
                self.markup.tool = Tool::Place { what, signature };
                self.status = i18n::t("Click the page where it goes").into();
            }
        }
        cx.notify();
    }

    /// A picture dragged from the file manager over the pages.
    pub(crate) fn stamp_drag_over(
        &mut self,
        ev: &DragMoveEvent<ExternalPaths>,
        cx: &mut Context<Self>,
    ) {
        let images = ev.drag(cx).paths().iter().any(|p| is_image(p));
        let at = if images {
            self.user_at(ev.event.position, false)
                .map(|(p, _, pt)| (p, pt))
        } else {
            None
        };
        if at != self.markup.stamp_drop {
            self.markup.stamp_drop = at;
            cx.notify();
        }
    }

    /// Files dropped on the pages: pictures become stamps there.
    pub(crate) fn drop_stamps(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        let Some((place, pt)) = self.markup.stamp_drop.take() else {
            return false;
        };
        let images: Vec<&PathBuf> = paths.iter().filter(|p| is_image(p)).collect();
        if images.is_empty() {
            return false;
        }
        for (i, p) in images.into_iter().enumerate() {
            let pt = [pt[0] + 12.0 * i as f32, pt[1] - 12.0 * i as f32];
            self.stamp_from_file(p, Some((place, pt)), cx);
        }
        true
    }

    // ── the signature dialog ───────────────────────────────────────────────

    pub fn add_signature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Type your name")));
        typed.update(cx, |i, cx| {
            i.set_value(self.markup.author.clone(), window, cx)
        });
        let sub = cx.subscribe_in(&typed, window, |this, input, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                let text = input.read(cx).value().to_string();
                this.make_outlines(text, cx);
            }
        });
        // Fonts are read in the background the first time.
        let text = self.markup.author.clone();
        self.sign = Some(SignDialog::Signature {
            tab: SignTab::Draw,
            strokes: Vec::new(),
            drawing: false,
            pad: Rc::default(),
            typed,
            outlines: None,
            remember: true,
            _sub: sub,
        });
        self.make_outlines(text, cx);
        cx.notify();
    }

    fn make_outlines(&mut self, text: String, cx: &mut Context<Self>) {
        let t2 = text.clone();
        let work = cx.background_spawn(async move { typed_outlines(&t2) });
        cx.spawn(async move |this, cx| {
            let r = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(SignDialog::Signature { outlines, .. }) = &mut this.sign {
                    *outlines = r.map(|(s, a)| (text, Arc::new(s), a));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_ok(&mut self, cx: &mut Context<Self>) {
        let Some(SignDialog::Signature {
            tab,
            strokes,
            pad,
            outlines,
            remember: keep,
            ..
        }) = &self.sign
        else {
            return;
        };
        let (what, typed) = match tab {
            SignTab::Draw => {
                let b = pad.get();
                let all: Vec<[f32; 2]> = strokes.iter().flatten().copied().collect();
                if all.len() < 2 {
                    return;
                }
                let x0 = all.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
                let x1 = all.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
                let y0 = all.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                let y1 = all.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
                let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
                let _ = b;
                let norm = strokes
                    .iter()
                    .map(|s| {
                        s.iter()
                            .map(|p| [(p[0] - x0) / w, 1.0 - (p[1] - y0) / h])
                            .collect()
                    })
                    .collect();
                (
                    Placed::Ink {
                        strokes: norm,
                        aspect: w / h,
                    },
                    None,
                )
            }
            SignTab::Type => {
                let Some((text, segs, aspect)) = outlines.clone() else {
                    return;
                };
                (Placed::Typed { segs, aspect }, Some(text))
            }
            SignTab::Image => return,
        };
        if *keep && let Err(e) = remember(&what, typed.as_deref(), true) {
            log::warn!("keeping the signature: {e}");
        }
        self.sign = None;
        self.use_placed(what, true, None, cx);
    }

    fn sign_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose a picture of your signature").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                match std::fs::read(&p)
                    .map_err(|e| e.to_string())
                    .and_then(image_placed)
                {
                    Ok(what) => {
                        let keep = matches!(
                            &this.sign,
                            Some(SignDialog::Signature { remember: true, .. })
                        );
                        if keep && let Err(e) = remember(&what, None, true) {
                            log::warn!("keeping the signature: {e}");
                        }
                        this.sign = None;
                        this.use_placed(what, true, None, cx);
                    }
                    Err(e) => this.status = tf("Couldn't use {}: {}", &[&p.display(), &e]).into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Place on Pages ─────────────────────────────────────────────────────

    pub fn place_on_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_stamp = self
            .markup
            .sel
            .clone()
            .and_then(|s| self.find_mark(&s))
            .is_some_and(|a| a.kind.editable());
        if !is_stamp {
            self.status = i18n::t("Select a stamp or comment to place on other pages").into();
            cx.notify();
            return;
        }
        let pages = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("All pages")));
        pages.update(cx, |i, cx| i.focus(window, cx));
        self.dialog_subs.clear();
        let sub = cx.subscribe_in(&pages, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.place_pages_ok(window, cx);
            }
        });
        self.dialog_subs.push(sub);
        self.sign = Some(SignDialog::PlacePages { pages, error: None });
        self.load_all_marks(cx);
        cx.notify();
    }

    fn place_pages_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(SignDialog::PlacePages { pages, .. }) = &self.sign else {
            return;
        };
        let text = pages.read(cx).value().to_string();
        let n = self.page_count() as u32;
        let set = crate::dialog::parse_pages(&text)
            .and_then(|r| r.resolve_set(n).map_err(|e| e.to_string()));
        match set {
            Ok(set) => {
                let places: Vec<usize> = set.into_iter().map(|p| p as usize - 1).collect();
                self.sign = None;
                self.stamp_on_pages(&places, cx);
                self.focus(window, cx);
            }
            Err(e) => {
                if let Some(SignDialog::PlacePages { error, .. }) = &mut self.sign {
                    *error = Some(e);
                }
            }
        }
        cx.notify();
    }

    // ── rendering ──────────────────────────────────────────────────────────

    pub(crate) fn render_sign(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.sign.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let close = {
            let v = view.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    this.sign = None;
                    this.focus(window, cx);
                    cx.notify();
                });
            }
        };
        let card = match d {
            SignDialog::PlacePages { pages, error } => {
                let ok = view.clone();
                crate::dialog::card(i18n::t("Place on Pages"), 380.0, cx)
                    .child(div().text_color(t.fg_muted).text_size(FONT_SMALL).child(i18n::t(
                        "Copies the selected stamp to the same place on these pages. Leave empty for all pages, or type pages like 1-3, 5.",
                    )))
                    .child(Input::new(pages))
                    .children(error.as_ref().map(|e| div().text_size(FONT_SMALL).text_color(t.danger).child(e.clone())))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(SPACE_3)
                            .child(Button::new("pdf-place-cancel").ghost().small().label(i18n::t("Cancel")).on_click(close))
                            .child(Button::new("pdf-place-ok").primary().small().label(i18n::t("Place")).on_click(
                                move |_, window, cx| {
                                    let _ = ok.update(cx, |this, cx| this.place_pages_ok(window, cx));
                                },
                            )),
                    )
            }
            SignDialog::Signature {
                tab,
                strokes,
                pad,
                typed,
                outlines,
                remember,
                ..
            } => {
                let tab_btn = |id: &'static str, label: &'static str, which: SignTab| {
                    let v = view.clone();
                    Button::new(id)
                        .small()
                        .label(i18n::t(label))
                        .when(*tab == which, |b| b.primary())
                        .when(*tab != which, |b| b.ghost())
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(SignDialog::Signature { tab, .. }) = &mut this.sign {
                                    *tab = which;
                                }
                                cx.notify();
                            });
                        })
                };
                let ink = t.fg;
                let body: AnyElement = match tab {
                    SignTab::Draw => {
                        let pad_b = pad.clone();
                        let lines = strokes.clone();
                        let (vd, vm, vu) = (view.clone(), view.clone(), view.clone());
                        div()
                            .id("pdf-sign-pad")
                            .h(px(150.))
                            .w_full()
                            .rounded(RADIUS_MD)
                            .border_1()
                            .border_color(t.border_strong)
                            .bg(t.paper)
                            .cursor(CursorStyle::Crosshair)
                            .child(
                                canvas(
                                    move |b, _, _| pad_b.set(b),
                                    move |b, _, window, _| {
                                        // The signing line.
                                        let y = b.origin.y + b.size.height * 0.75;
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(b.origin.x + px(16.), y),
                                                size(b.size.width - px(32.), px(1.)),
                                            ),
                                            ink.opacity(0.25),
                                        ));
                                        for s in &lines {
                                            let Some(first) = s.first() else { continue };
                                            let mut pb = PathBuilder::stroke(px(2.));
                                            let at = |p: [f32; 2]| {
                                                point(b.origin.x + px(p[0]), b.origin.y + px(p[1]))
                                            };
                                            pb.move_to(at(*first));
                                            if s.len() == 1 {
                                                pb.line_to(at([first[0] + 0.5, first[1]]));
                                            }
                                            for p in &s[1..] {
                                                pb.line_to(at(*p));
                                            }
                                            if let Ok(p) = pb.build() {
                                                window.paint_path(p, ink);
                                            }
                                        }
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
                                let _ = vd.update(cx, |this, cx| {
                                    if let Some(SignDialog::Signature {
                                        strokes,
                                        drawing,
                                        pad,
                                        ..
                                    }) = &mut this.sign
                                    {
                                        let o = pad.get().origin;
                                        strokes.push(vec![[
                                            f32::from(ev.position.x - o.x),
                                            f32::from(ev.position.y - o.y),
                                        ]]);
                                        *drawing = true;
                                    }
                                    cx.notify();
                                });
                            })
                            .on_mouse_move(move |ev, _, cx| {
                                let _ = vm.update(cx, |this, cx| {
                                    if let Some(SignDialog::Signature {
                                        strokes,
                                        drawing: true,
                                        pad,
                                        ..
                                    }) = &mut this.sign
                                        && let Some(s) = strokes.last_mut()
                                    {
                                        let o = pad.get().origin;
                                        s.push([
                                            f32::from(ev.position.x - o.x),
                                            f32::from(ev.position.y - o.y),
                                        ]);
                                        cx.notify();
                                    }
                                });
                            })
                            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                                let _ = vu.update(cx, |this, _| {
                                    if let Some(SignDialog::Signature { drawing, .. }) =
                                        &mut this.sign
                                    {
                                        *drawing = false;
                                    }
                                });
                            })
                            .into_any_element()
                    }
                    SignTab::Type => {
                        let preview = outlines.as_ref().map(|(_, segs, aspect)| Placed::Typed {
                            segs: segs.clone(),
                            aspect: *aspect,
                        });
                        v_flex()
                            .gap(SPACE_3)
                            .child(Input::new(typed))
                            .child(
                                div()
                                    .h(px(110.))
                                    .w_full()
                                    .rounded(RADIUS_MD)
                                    .border_1()
                                    .border_color(t.border_strong)
                                    .bg(t.paper)
                                    .p(SPACE_4)
                                    .child(
                                        canvas(
                                            |_, _, _| {},
                                            move |b, _, window, _| {
                                                if let Some(p) = &preview {
                                                    paint_placed(window, b, p, ink);
                                                }
                                            },
                                        )
                                        .size_full(),
                                    ),
                            )
                            .into_any_element()
                    }
                    SignTab::Image => {
                        let v = view.clone();
                        v_flex()
                            .h(px(150.))
                            .items_center()
                            .justify_center()
                            .gap(SPACE_3)
                            .child(div().text_color(t.fg_muted).text_size(FONT_SMALL).child(i18n::t(
                                "A scan or photo of your signature (PNG keeps a transparent background).",
                            )))
                            .child(Button::new("pdf-sign-image").small().icon(Lucide::ImagePlus).label(i18n::t("Choose Image…")).on_click(
                                move |_, window, cx| {
                                    let _ = v.update(cx, |this, cx| this.sign_image(window, cx));
                                },
                            ))
                            .into_any_element()
                    }
                };
                let saved = saved_list(&view, cx);
                let (vc, vr, vo) = (view.clone(), view.clone(), view.clone());
                let remember = *remember;
                crate::dialog::card(i18n::t("Add Signature"), 520.0, cx)
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .child(tab_btn("pdf-sign-draw", "Draw", SignTab::Draw))
                            .child(tab_btn("pdf-sign-type", "Type", SignTab::Type))
                            .child(tab_btn("pdf-sign-img", "Image", SignTab::Image)),
                    )
                    .child(body)
                    .child(
                        div().text_size(FONT_SMALL).text_color(t.fg_muted).child(i18n::t(
                            "This places a picture of your signature on the page. It is not a certificate-based digital signature.",
                        )),
                    )
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                gpui_kit::component::checkbox::Checkbox::new("pdf-sign-remember")
                                    .checked(remember)
                                    .label(i18n::t("Remember on this computer"))
                                    .on_click(move |c, _, cx| {
                                        let c = *c;
                                        let _ = vr.update(cx, |this, cx| {
                                            if let Some(SignDialog::Signature { remember, .. }) = &mut this.sign {
                                                *remember = c;
                                            }
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(div().flex_1())
                            .child(Button::new("pdf-sign-clear").ghost().small().label(i18n::t("Clear")).on_click(move |_, _, cx| {
                                let _ = vc.update(cx, |this, cx| {
                                    if let Some(SignDialog::Signature { strokes, .. }) = &mut this.sign {
                                        strokes.clear();
                                    }
                                    cx.notify();
                                });
                            }))
                            .child(Button::new("pdf-sign-cancel").ghost().small().label(i18n::t("Cancel")).on_click(close))
                            .child(Button::new("pdf-sign-ok").primary().small().label(i18n::t("Place")).on_click(move |_, _, cx| {
                                let _ = vo.update(cx, |this, cx| this.sign_ok(cx));
                            })),
                    )
                    .children(saved)
            }
        };
        let esc = view.clone();
        let card = card.on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = esc.update(cx, |this, cx| {
                    this.sign = None;
                    this.focus(window, cx);
                    cx.notify();
                });
            }
        });
        Some(
            kantor_ui::overlay::backdrop()
                .child(
                    h_flex()
                        .absolute()
                        .top(CONTROL_LG * 2. + SPACE_5)
                        .left_0()
                        .right_0()
                        .justify_center()
                        .child(card),
                )
                .into_any_element(),
        )
    }

    /// The bar under the toolbar for forms and signed documents.
    pub(crate) fn render_form_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = tokens(cx);
        let sigs = self.signatures();
        let form = self.form_state();
        if sigs.is_none() && form.is_none() {
            return None;
        }
        let mut bar = h_flex()
            .flex_none()
            .px(SPACE_5)
            .py(SPACE_2)
            .gap(SPACE_4)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted);
        if let Some(s) = sigs {
            let msg = match s.allowed() {
                kantor_pdf::sig::Allowed::Nothing => i18n::t(
                    "This document is certified with a digital signature that allows no changes.",
                ),
                kantor_pdf::sig::Allowed::FormFill => i18n::t(
                    "This document is digitally signed. Only filling in its form keeps the signature valid; Kantor saves entries after the signed part.",
                ),
                kantor_pdf::sig::Allowed::Annotations => i18n::t(
                    "This document is digitally signed. Comments, stamps and form entries are saved after the signed part, so the signature stays valid; page changes would invalidate it.",
                ),
            };
            bar = bar
                .child(gpui_kit::component::Icon::new(Lucide::Lock).with_size(ICON_SM))
                .child(div().flex_1().child(msg));
        }
        match form {
            Some(true) => {
                bar = bar.child(div().flex_1().text_color(t.danger).child(i18n::t(
                    "This form uses XFA, which Kantor can't fill. Open it in Adobe Acrobat or Reader to fill it in.",
                )));
            }
            Some(false) => {
                let v = cx.entity().downgrade();
                bar = bar
                    .when(sigs.is_none(), |b| {
                        b.child(
                            div()
                                .flex_1()
                                .child(i18n::t("This document has form fields you can fill in.")),
                        )
                    })
                    .child(
                        Button::new("pdf-reset-form")
                            .ghost()
                            .xsmall()
                            .label(i18n::t("Reset Form"))
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| this.reset_form(cx));
                            }),
                    );
            }
            None => {}
        }
        Some(bar.into_any_element())
    }

    /// Page changes on a signed document: say once that saving would
    /// invalidate the signature.
    pub(crate) fn warn_signed_pages(&mut self) {
        if self.signed_warned || self.signatures().is_none() {
            return;
        }
        if self.doc().is_some_and(|d| !d.model.pages_as_base()) {
            self.signed_warned = true;
            self.status = i18n::t(
                "Changing pages invalidates the document's digital signature: save these changes as a copy",
            )
            .into();
        }
    }
}

/// My Stamps in the signature dialog: click to place, × to forget.
fn saved_list(view: &WeakEntity<PdfView>, cx: &App) -> Option<Div> {
    let saved = load_saved();
    if saved.is_empty() {
        return None;
    }
    let t = tokens(cx);
    let mut row = h_flex().gap(SPACE_3).flex_wrap();
    for (i, s) in saved.into_iter().enumerate() {
        let (vp, vx) = (view.clone(), view.clone());
        let what = s.what.clone();
        let sig = s.signature;
        let path = s.path.clone();
        let ink = t.fg;
        let thumb: AnyElement = match &s.what {
            Placed::Image { .. } => img(s.path.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            p => {
                let p = p.clone();
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| paint_placed(window, b, &p, ink),
                )
                .size_full()
                .into_any_element()
            }
        };
        row = row.child(
            div()
                .id(("pdf-saved", i))
                .relative()
                .w(px(110.))
                .h(px(56.))
                .p(SPACE_2)
                .rounded(RADIUS_SM)
                .border_1()
                .border_color(t.border)
                .bg(t.paper)
                .cursor_pointer()
                .hover(|d| d.border_color(t.accent))
                .child(thumb)
                .on_click(move |_, _, cx| {
                    let what = what.clone();
                    let _ = vp.update(cx, |this, cx| {
                        this.sign = None;
                        this.use_placed(what, sig, None, cx);
                    });
                })
                .child(
                    div().absolute().top(px(-6.)).right(px(-6.)).child(
                        Button::new(("pdf-saved-x", i))
                            .xsmall()
                            .ghost()
                            .icon(Lucide::X)
                            .tooltip(i18n::t("Remove from My Stamps"))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = std::fs::remove_file(&path);
                                let _ = vx.update(cx, |_, cx| cx.notify());
                            }),
                    ),
                ),
        );
    }
    Some(
        v_flex()
            .gap(SPACE_2)
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i18n::t("My Signatures & Stamps")),
            )
            .child(row),
    )
}

/// The Sign menu: My Stamps, Add Signature…, Add Image / Stamp….
pub(crate) fn sign_menu(menu: PopupMenu, view: &WeakEntity<PdfView>) -> PopupMenu {
    let mut menu = menu;
    for (i, s) in load_saved().into_iter().enumerate() {
        let v = view.clone();
        let label = match (&s.what, s.signature) {
            (_, true) => tf("Signature {}", &[&(i + 1)]),
            (Placed::Image { .. }, false) => s
                .path
                .file_name()
                .map(|_| tf("Stamp {}", &[&(i + 1)]))
                .unwrap_or_default(),
            _ => tf("Stamp {}", &[&(i + 1)]),
        };
        let what = s.what.clone();
        let sig = s.signature;
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let what = what.clone();
            let _ = v.update(cx, |this, cx| this.use_placed(what, sig, None, cx));
        }));
    }
    let (a, b) = (view.clone(), view.clone());
    menu.separator()
        .item(
            PopupMenuItem::new(i18n::t("Add Signature…")).on_click(move |_, window, cx| {
                let _ = a.update(cx, |this, cx| this.add_signature(window, cx));
            }),
        )
        .item(
            PopupMenuItem::new(i18n::t("Add Image / Stamp…")).on_click(move |_, window, cx| {
                let _ = b.update(cx, |this, cx| this.add_image_stamp(None, window, cx));
            }),
        )
}

/// The pages' right-click menu.
pub(crate) fn page_menu(
    menu: PopupMenu,
    view: &WeakEntity<PdfView>,
    _: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(v) = view.upgrade() else {
        return menu;
    };
    let (has_text, has_mark, has_form) = {
        let v = v.read(cx);
        (
            v.ordered_sel().is_some(),
            v.markup.sel.is_some(),
            v.form_state() == Some(false),
        )
    };
    let item = |label: &'static str, run: fn(&mut PdfView, &mut Window, &mut Context<PdfView>)| {
        let v = view.clone();
        PopupMenuItem::new(i18n::t(label)).on_click(move |_, window, cx| {
            let _ = v.update(cx, |this, cx| run(this, window, cx));
        })
    };
    let mut menu = menu;
    if has_text {
        menu = menu
            .item(item("Highlight Text", |v, _, cx| {
                v.markup_selection(&Tool::Highlight, cx)
            }))
            .item(item("Underline Text", |v, _, cx| {
                v.markup_selection(&Tool::Underline, cx)
            }))
            .item(item("Strikethrough Text", |v, _, cx| {
                v.markup_selection(&Tool::StrikeOut, cx)
            }))
            .item(item("Copy", |v, _, cx| v.copy_text(cx)))
            .separator();
    }
    menu = menu
        .item(item("Add Sticky Note", |v, w, cx| {
            if let Some((place, pt)) = v.context_at.take() {
                let mut a = kantor_pdf::annot::Annot::new(
                    kantor_pdf::annot::Kind::Note,
                    [pt[0], pt[1] - 20.0, pt[0] + 20.0, pt[1]],
                    crate::markup::YELLOW,
                );
                a.author = v.markup.author.clone();
                let id = a.id.clone();
                if v.edit_marks(
                    place,
                    kantor_ui::i18n::tk("Add Sticky Note"),
                    |l| l.push(a),
                    cx,
                ) {
                    v.markup.sel = Some(crate::markup::Sel {
                        place,
                        id: id.clone(),
                    });
                    v.open_text_edit(place, id, w, cx);
                }
            }
        }))
        .item(item("Add Image / Stamp…", |v, w, cx| {
            v.add_image_stamp(None, w, cx)
        }))
        .item(item("Add Signature…", |v, w, cx| v.add_signature(w, cx)));
    if has_mark {
        menu = menu
            .separator()
            .item(item("Place on Pages…", |v, w, cx| {
                v.place_on_pages(w, cx)
            }))
            .item(item("Delete Comment", |v, _, cx| {
                v.delete_mark(cx);
            }));
    }
    if has_form {
        menu = menu
            .separator()
            .item(item("Reset Form", |v, _, cx| v.reset_form(cx)));
    }
    menu
}
