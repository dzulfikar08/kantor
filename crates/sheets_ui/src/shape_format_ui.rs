//! The Format Shape panel, as Excel's pane for shapes, text boxes and
//! lines: Fill (no fill, solid colour with transparency, gradient,
//! picture), Line (no line, colour, width, dash type, begin and end
//! arrows), Effects (Excel's outer shadow presets) and the text's vertical
//! alignment. Right-click a shape (or the Shapes menu, or
//! the command palette) opens it; each change is one undo step.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tk};
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use sheets_engine::drawing::DrawingKey;
use sheets_engine::shape_format::{self as fmt, ARROWS, DASHES};
use sheets_engine::{OutlineEdit, ShadowPreset, ShapeFill, TextAnchor};
use slides_engine::fill::{ArrowKind, Dash, Fill};
use slides_engine::{Anchor, ShapeKind};

use crate::SheetsView;
use crate::dialog::row;

const PANEL_WIDTH: Pixels = px(290.);
use crate::toolbar::swatch_popover;

/// Outline widths offered, in points (Excel's Width list).
const WIDTHS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0];
const EMU_PER_PT: f32 = 12_700.0;
/// The colour a new solid fill or line takes (Office's accent 1).
const ACCENT1: [u8; 3] = [0x44, 0x72, 0xC4];

/// The panel's state.
pub(crate) struct ShapePanel {
    /// The shape the transparency box was filled for, with the revision.
    loaded: Option<(DrawingKey, (u64, u64))>,
    transparency: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// What the panel shows for a shape, read from its element.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShapeLook {
    /// `None`: no fill.
    pub fill: Option<[u8; 3]>,
    pub gradient: bool,
    /// Percent.
    pub transparency: u8,
    pub line: Option<[u8; 3]>,
    /// Points.
    pub line_pt: f32,
    pub dash: &'static str,
    pub head: &'static str,
    pub tail: &'static str,
    /// An open path (line, connector): arrows apply.
    pub is_line: bool,
    /// Holds text: vertical alignment applies.
    pub has_text: bool,
    pub anchor: Option<TextAnchor>,
    /// A picture fill.
    pub picture: bool,
    /// Its outer shadow preset (`None`: none, or not a preset).
    pub shadow: Option<ShadowPreset>,
    /// It has a shadow (preset or not).
    pub shadowed: bool,
}

fn dash_name(d: &Dash) -> &'static str {
    match d {
        Dash::Solid => "solid",
        Dash::SysDot | Dash::Dot => "sysDot",
        Dash::SysDash => "sysDash",
        Dash::Dash => "dash",
        Dash::DashDot | Dash::SysDashDot => "dashDot",
        Dash::LgDash => "lgDash",
        Dash::LgDashDot => "lgDashDot",
        Dash::LgDashDotDot | Dash::SysDashDotDot => "lgDashDotDot",
        Dash::Custom(_) => "dash",
    }
}

fn arrow_name(a: Option<ArrowKind>) -> &'static str {
    match a {
        None => "none",
        Some(ArrowKind::Triangle) => "triangle",
        Some(ArrowKind::Arrow) => "arrow",
        Some(ArrowKind::Stealth) => "stealth",
        Some(ArrowKind::Diamond) => "diamond",
        Some(ArrowKind::Oval) => "oval",
    }
}

/// Read what the panel shows from a shape element, with the workbook's
/// theme (Office's when it has none) for theme colours.
pub(crate) fn shape_look(xml: &str, theme_xml: Option<&str>) -> Option<ShapeLook> {
    let theme = slides_engine::theme::Theme::parse(
        &theme_xml
            .map(str::to_owned)
            .unwrap_or_else(slides_engine::sheet_shape::office_theme),
    );
    let map = slides_engine::theme::ClrMap::default();
    let cx = slides_engine::theme::ColorCtx {
        theme: &theme,
        map: &map,
        ph: None,
    };
    let s = slides_engine::sheet_shape::parse(&format!("<r>{xml}</r>"), cx)?;
    let look = &s.look;
    let (fill, gradient, alpha) = match &look.fill_as_read {
        Some(Fill::None) => (None, false, 255),
        Some(Fill::Solid(c, a)) => (Some(c.0), false, *a),
        Some(Fill::Gradient(g)) => (g.stops.last().map(|s| s.color.0), true, 255),
        Some(Fill::Picture(_)) => (None, false, 255),
        None => (look.fill.map(|c| c.0), false, 255),
    };
    let picture = matches!(look.fill_as_read, Some(Fill::Picture(_)));
    // Lines, arcs and connectors are open paths: arrows apply.
    let is_line = matches!(s.kind, ShapeKind::Shape { .. })
        && (fmt::is_connector(xml)
            || xml.contains("prst=\"line\"")
            || xml.contains("prst=\"lineInv\"")
            || xml.contains("prst=\"arc\"")
            || xml.contains("Connector"));
    let has_text = fmt::takes_text(xml);
    let anchor = s
        .text
        .as_ref()
        .map(|t| match t.body.anchor.unwrap_or_default() {
            Anchor::Top => TextAnchor::Top,
            Anchor::Middle => TextAnchor::Middle,
            Anchor::Bottom => TextAnchor::Bottom,
        });
    Some(ShapeLook {
        fill,
        gradient,
        transparency: (100 - (alpha as u32 * 100 + 127) / 255).min(100) as u8,
        line: look.line.map(|(c, _)| c.0),
        line_pt: look.line.map_or(0.75, |(_, w)| w as f32 / EMU_PER_PT),
        dash: dash_name(&look.line_style.dash),
        head: arrow_name(look.line_style.head.map(|a| a.kind)),
        tail: arrow_name(look.line_style.tail.map(|a| a.kind)),
        is_line,
        has_text,
        anchor: has_text.then(|| anchor.unwrap_or(TextAnchor::Top)),
        picture,
        shadow: fmt::shadow_of(xml),
        shadowed: look.effects.shadow.is_some(),
    })
}

fn dash_label(d: &str) -> &'static str {
    match d {
        "solid" => tk("Solid"),
        "sysDot" => tk("Round Dot"),
        "sysDash" => tk("Square Dot"),
        "dash" => tk("Dash"),
        "dashDot" => tk("Dash Dot"),
        "lgDash" => tk("Long Dash"),
        "lgDashDot" => tk("Long Dash Dot"),
        _ => tk("Long Dash Dot Dot"),
    }
}

fn arrow_label(a: &str) -> &'static str {
    match a {
        "none" => tk("No Arrow"),
        "triangle" => tk("Arrow"),
        "arrow" => tk("Open Arrow"),
        "stealth" => tk("Stealth Arrow"),
        "diamond" => tk("Diamond Arrow"),
        _ => tk("Oval Arrow"),
    }
}

fn rgb_u32([r, g, b]: [u8; 3]) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

fn rgb_bytes(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

impl SheetsView {
    /// Show Format Shape for shape `key` (right-click).
    pub(crate) fn open_shape_panel(
        &mut self,
        key: DrawingKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drawings.borrow_mut().selected = Some(key);
        if self.shape_panel.is_none() {
            let transparency = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
            let sub = cx.subscribe_in(&transparency, window, |this, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } | InputEvent::Blur = e {
                    this.apply_transparency(w, cx);
                }
            });
            self.shape_panel = Some(ShapePanel {
                loaded: None,
                transparency,
                _subs: vec![sub],
            });
        }
        // Charts have their own panel.
        self.chart_panel = None;
        cx.notify();
    }

    /// Format Shape (palette, Shapes menu): show or hide the panel for the
    /// selected shape.
    pub fn format_shape_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shape_panel.take().is_some() {
            cx.notify();
            return;
        }
        match self.selected_shape(cx) {
            Some((key, _)) => self.open_shape_panel(key, window, cx),
            None => {
                self.status = i18n::t("Select a shape or text box first").into();
                cx.notify();
            }
        }
    }

    /// The selected shape and its element (else the one the panel shows:
    /// clicking in the panel deselects it on the grid).
    pub(crate) fn selected_shape(&self, cx: &App) -> Option<(DrawingKey, std::sync::Arc<str>)> {
        let key = match self.drawings.borrow().selected {
            Some(k) => k,
            None => self.shape_panel.as_ref()?.loaded?.0,
        };
        let xml = self.table.read(cx).delegate().book.shape_xml(key)?;
        Some((key, xml))
    }

    /// Change the selected shape's element with `f` (one undo step).
    pub(crate) fn format_shape(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&str) -> anyhow::Result<String>,
    ) {
        let Some((key, _)) = self.selected_shape(cx) else {
            return;
        };
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let r = grid.book.edit_shape(key, f);
            table.refresh(cx);
            r
        });
        self.status = match res {
            Ok(()) => i18n::t("Shape formatted").into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    fn apply_transparency(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.shape_panel.as_ref() else {
            return;
        };
        let text = p
            .transparency
            .read(cx)
            .value()
            .trim()
            .trim_end_matches('%')
            .to_owned();
        let Ok(t) = text.parse::<f32>() else {
            return;
        };
        let Some(look) = self.selected_look(cx) else {
            return;
        };
        let t = t.clamp(0.0, 100.0).round() as u8;
        if t == look.transparency {
            return;
        }
        let rgb = look.fill.unwrap_or(ACCENT1);
        self.format_shape(window, cx, move |x| {
            fmt::set_fill(
                x,
                ShapeFill::Solid {
                    rgb,
                    transparency: t,
                },
            )
        });
    }

    fn selected_look(&self, cx: &App) -> Option<ShapeLook> {
        let (_, xml) = self.selected_shape(cx)?;
        let theme = self.table.read(cx).delegate().book.theme_xml();
        shape_look(&xml, theme.as_deref())
    }

    /// Fill the transparency box for the selected shape (from `render`).
    pub(crate) fn sync_shape_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.shape_panel.as_ref() else {
            return;
        };
        let rev = self.table.read(cx).delegate().book.revision();
        let key = self.selected_shape(cx).map(|s| s.0);
        let now = key.map(|k| (k, rev));
        if p.loaded == now || key.is_none() {
            return;
        }
        let look = self.selected_look(cx);
        let input = p.transparency.clone();
        let v = look
            .filter(|l| l.fill.is_some() && !l.gradient)
            .map(|l| format!("{}%", l.transparency))
            .unwrap_or_default();
        input.update(cx, |i, cx| i.set_value(v, window, cx));
        if let Some(p) = self.shape_panel.as_mut() {
            p.loaded = now;
        }
    }

    /// The panel beside the grid.
    pub(crate) fn render_shape_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.shape_panel.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let close = view.clone();
        let header = h_flex()
            .flex_none()
            .gap(SPACE_3)
            .child(
                div()
                    .flex_1()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i18n::t("Format Shape")),
            )
            .child(
                Button::new("shape-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::X)
                    .tooltip(i18n::t("Close"))
                    .on_click(move |_, _, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.shape_panel = None;
                            cx.notify();
                        });
                    }),
            );
        let body: AnyElement = match self.selected_look(cx) {
            Some(look) => self.render_shape_options(&look, &p.transparency.clone(), &view, cx),
            None => div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(i18n::t("Select a shape or text box to format it."))
                .into_any_element(),
        };
        Some(
            v_flex()
                .id("shape-panel")
                .key_context("KantorShapePanel")
                .flex_none()
                .w(PANEL_WIDTH)
                .h_full()
                .gap(SPACE_4)
                .p(SPACE_5)
                .bg(t.chrome)
                .border_l_1()
                .border_color(t.border)
                .overflow_y_scroll()
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_shape_options(
        &self,
        look: &ShapeLook,
        transparency: &Entity<InputState>,
        view: &WeakEntity<SheetsView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let section = |title: &'static str| {
            div()
                .pt(SPACE_2)
                .text_size(FONT_SMALL)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.fg_muted)
                .child(i18n::t(title))
        };
        // A row of choice buttons, `on` checked.
        let choice = |id: &'static str, label: &'static str, on: bool, act: Action| {
            let v = view.clone();
            Button::new(id)
                .small()
                .when(on, |b| b.primary())
                .when(!on, |b| b.outline())
                .label(i18n::t(label))
                .on_click(move |_, w, cx| {
                    let act = act.clone();
                    let _ = v.update(cx, |this, cx| this.format_shape(w, cx, move |x| act(x)));
                })
        };
        let fill_rgb = look.fill.unwrap_or(ACCENT1);
        let solid = look.fill.is_some() && !look.gradient;
        let tr = look.transparency;
        let fills = h_flex()
            .gap(SPACE_2)
            .flex_wrap()
            .child(choice(
                "shape-no-fill",
                tk("No fill"),
                look.fill.is_none(),
                act(|x| fmt::set_fill(x, ShapeFill::None)),
            ))
            .child(choice(
                "shape-solid-fill",
                tk("Solid fill"),
                solid,
                act(move |x| {
                    fmt::set_fill(
                        x,
                        ShapeFill::Solid {
                            rgb: fill_rgb,
                            transparency: tr,
                        },
                    )
                }),
            ))
            .child(choice(
                "shape-gradient-fill",
                tk("Gradient fill"),
                look.gradient,
                act(move |x| {
                    fmt::set_fill(
                        x,
                        ShapeFill::Gradient {
                            rgb: fill_rgb,
                            vertical: true,
                        },
                    )
                }),
            ));
        let pv = view.clone();
        let fills = fills.child(
            Button::new("shape-picture-fill")
                .small()
                .when(look.picture, |b| b.primary())
                .when(!look.picture, |b| b.outline())
                .label(i18n::t("Picture fill…"))
                .on_click(move |_, w, cx| {
                    let _ = pv.update(cx, |this, cx| this.pick_shape_picture(w, cx));
                }),
        );
        let sv = view.clone();
        let current_shadow = look.shadow;
        let shadow_label = match (look.shadow, look.shadowed) {
            (Some(p), _) => i18n::t_dyn(p.label()).to_owned(),
            (None, true) => i18n::t("Custom").to_owned(),
            (None, false) => i18n::t("No shadow").to_owned(),
        };
        let shadow = Button::new("shape-shadow")
            .outline()
            .small()
            .label(shadow_label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut m, _, _| {
                let opts = std::iter::once(None).chain(ShadowPreset::ALL.into_iter().map(Some));
                for p in opts {
                    let v = sv.clone();
                    let label = p.map_or(i18n::t("No shadow"), |p| i18n::t_dyn(p.label()));
                    m = m.item(
                        PopupMenuItem::new(label)
                            .checked(p == current_shadow)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.format_shape(w, cx, move |x| fmt::set_shadow(x, p))
                                });
                            }),
                    );
                }
                m
            });
        let gradient = look.gradient;
        let fv = view.clone();
        let fill_colour = swatch_popover(
            "shape-fill-colour",
            Lucide::PaintBucket,
            tk("Fill Colour"),
            None,
            look.fill.map(|c| crate::look::hsla(rgb_u32(c))),
            std::rc::Rc::new(move |c, w, cx| {
                let Some(c) = c else { return };
                let rgb = rgb_bytes(c);
                let _ = fv.update(cx, |this, cx| {
                    this.format_shape(w, cx, move |x| {
                        fmt::set_fill(
                            x,
                            if gradient {
                                ShapeFill::Gradient {
                                    rgb,
                                    vertical: true,
                                }
                            } else {
                                ShapeFill::Solid {
                                    rgb,
                                    transparency: tr,
                                }
                            },
                        )
                    })
                });
            }),
            cx,
        );
        let line_rgb = look.line.unwrap_or(ACCENT1);
        let lines = h_flex()
            .gap(SPACE_2)
            .child(choice(
                "shape-no-line",
                tk("No line"),
                look.line.is_none(),
                act(|x| {
                    fmt::set_outline(
                        x,
                        &OutlineEdit {
                            color: Some(None),
                            ..Default::default()
                        },
                    )
                }),
            ))
            .child(choice(
                "shape-solid-line",
                tk("Solid line"),
                look.line.is_some(),
                act(move |x| {
                    fmt::set_outline(
                        x,
                        &OutlineEdit {
                            color: Some(Some(line_rgb)),
                            ..Default::default()
                        },
                    )
                }),
            ));
        let lv = view.clone();
        let line_colour = swatch_popover(
            "shape-line-colour",
            Lucide::Square,
            tk("Line Colour"),
            None,
            look.line.map(|c| crate::look::hsla(rgb_u32(c))),
            std::rc::Rc::new(move |c, w, cx| {
                let Some(c) = c else { return };
                let _ = lv.update(cx, |this, cx| {
                    this.format_shape(w, cx, move |x| {
                        fmt::set_outline(
                            x,
                            &OutlineEdit {
                                color: Some(Some(rgb_bytes(c))),
                                ..Default::default()
                            },
                        )
                    })
                });
            }),
            cx,
        );
        let menu_button = |id: &'static str, label: String| {
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .dropdown_caret(true)
        };
        let wv = view.clone();
        let current_pt = look.line_pt;
        let width = menu_button("shape-line-width", format!("{current_pt} pt")).dropdown_menu(
            move |mut m, _, _| {
                for pt in WIDTHS {
                    let v = wv.clone();
                    m = m.item(
                        PopupMenuItem::new(format!("{pt} pt"))
                            .checked((pt - current_pt).abs() < 0.01)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.format_shape(w, cx, move |x| {
                                        fmt::set_outline(
                                            x,
                                            &OutlineEdit {
                                                width: Some((pt * EMU_PER_PT) as i64),
                                                ..Default::default()
                                            },
                                        )
                                    })
                                });
                            }),
                    );
                }
                m
            },
        );
        let dv = view.clone();
        let current_dash = look.dash;
        let dash = menu_button("shape-line-dash", i18n::t(dash_label(current_dash)).into())
            .dropdown_menu(move |mut m, _, _| {
                for d in DASHES {
                    let v = dv.clone();
                    m = m.item(
                        PopupMenuItem::new(i18n::t(dash_label(d)))
                            .checked(d == current_dash)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    this.format_shape(w, cx, move |x| {
                                        fmt::set_outline(
                                            x,
                                            &OutlineEdit {
                                                dash: Some(d),
                                                ..Default::default()
                                            },
                                        )
                                    })
                                });
                            }),
                    );
                }
                m
            });
        let arrow_menu = |id: &'static str, current: &'static str, tail: bool| {
            let av = view.clone();
            menu_button(id, i18n::t(arrow_label(current)).into()).dropdown_menu(
                move |mut m, _, _| {
                    for a in ARROWS {
                        let v = av.clone();
                        m = m.item(
                            PopupMenuItem::new(i18n::t(arrow_label(a)))
                                .checked(a == current)
                                .on_click(move |_, w, cx| {
                                    let edit = if tail {
                                        OutlineEdit {
                                            tail: Some(a),
                                            ..Default::default()
                                        }
                                    } else {
                                        OutlineEdit {
                                            head: Some(a),
                                            ..Default::default()
                                        }
                                    };
                                    let _ = v.update(cx, |this, cx| {
                                        this.format_shape(w, cx, move |x| {
                                            fmt::set_outline(x, &edit)
                                        })
                                    });
                                }),
                        );
                    }
                    m
                },
            )
        };
        let anchors = look.anchor.map(|current| {
            let mut r = h_flex().gap(SPACE_2);
            for (id, label, a) in [
                ("shape-anchor-top", tk("Top"), TextAnchor::Top),
                ("shape-anchor-middle", tk("Middle"), TextAnchor::Middle),
                ("shape-anchor-bottom", tk("Bottom"), TextAnchor::Bottom),
            ] {
                r = r.child(choice(
                    id,
                    label,
                    current == a,
                    act(move |x| fmt::set_text_anchor(x, a)),
                ));
            }
            r
        });
        v_flex()
            .gap(SPACE_4)
            .child(section(tk("Fill")))
            .child(fills)
            .when(look.fill.is_some(), |d| {
                d.child(row(i18n::t("Colour"), fill_colour, cx))
            })
            .when(solid, |d| {
                d.child(row(
                    i18n::t("Transparency"),
                    Input::new(transparency).small(),
                    cx,
                ))
            })
            .child(section(tk("Line")))
            .child(lines)
            .when(look.line.is_some(), |d| {
                d.child(row(i18n::t("Colour"), line_colour, cx))
                    .child(row(i18n::t("Width"), width, cx))
                    .child(row(i18n::t("Dash type"), dash, cx))
                    .when(look.is_line, |d| {
                        d.child(row(
                            i18n::t("Begin arrow"),
                            arrow_menu("shape-head", look.head, false),
                            cx,
                        ))
                        .child(row(
                            i18n::t("End arrow"),
                            arrow_menu("shape-tail", look.tail, true),
                            cx,
                        ))
                    })
            })
            .child(section(tk("Effects")))
            .child(row(i18n::t("Shadow"), shadow, cx))
            .when_some(anchors, |d, a| {
                d.child(section(tk("Text Box")))
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .child(i18n::t("Vertical alignment")),
                    )
                    .child(a)
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(i18n::t(
                                "Double-click the shape to edit its text and format it.",
                            )),
                    )
            })
            .into_any_element()
    }
}

impl SheetsView {
    /// Picture fill: pick a picture file for the selected shape.
    fn pick_shape_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _)) = self.selected_shape(cx) else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Fill with a picture (PNG, JPEG, GIF or BMP)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let res = std::fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| {
                        this.table.update(cx, |table, cx| {
                            let r = table
                                .delegate_mut()
                                .book
                                .set_shape_picture(key, bytes, &ext);
                            table.refresh(cx);
                            r
                        })
                    });
                if let Err(e) = res {
                    this.status = format!("{e:#}").into();
                }
                this.update_title(window, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// An edit of a shape's element.
type Action = std::rc::Rc<dyn Fn(&str) -> anyhow::Result<String>>;

fn act(f: impl Fn(&str) -> anyhow::Result<String> + 'static) -> Action {
    std::rc::Rc::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheets_engine::ShapePreset;

    #[std::prelude::v1::test]
    fn the_panel_reads_what_it_sets() {
        let rect = ShapePreset::Rectangle.xml("R");
        let l = shape_look(&rect, None).unwrap();
        // Style fill (accent 1) and outline from the theme.
        assert!(l.fill.is_some() && l.line.is_some());
        assert!(l.has_text && !l.is_line);
        let x = fmt::set_fill(
            &rect,
            ShapeFill::Solid {
                rgb: [255, 0, 0],
                transparency: 25,
            },
        )
        .unwrap();
        let x = fmt::set_outline(
            &x,
            &OutlineEdit {
                color: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        let x = fmt::set_text_anchor(&x, TextAnchor::Bottom).unwrap();
        let l = shape_look(&x, None).unwrap();
        assert_eq!(l.fill, Some([255, 0, 0]));
        assert_eq!(l.transparency, 25);
        assert_eq!(l.line, None);
        assert_eq!(l.anchor, Some(TextAnchor::Bottom));
        let arrow = ShapePreset::Arrow.xml("A");
        let l = shape_look(&arrow, None).unwrap();
        assert!(l.is_line && !l.has_text);
        assert_eq!(l.tail, "triangle");
        assert_eq!(l.head, "none");
    }

    #[std::prelude::v1::test]
    fn shadows_read_back() {
        let rect = ShapePreset::Rectangle.xml("R");
        assert_eq!(shape_look(&rect, None).unwrap().shadow, None);
        for p in ShadowPreset::ALL {
            let x = fmt::set_shadow(&rect, Some(p)).unwrap();
            let l = shape_look(&x, None).unwrap();
            assert_eq!(l.shadow, Some(p));
            assert!(l.shadowed);
            assert!(i18n::indonesian(p.label()).is_some(), "{}", p.label());
        }
        let x = fmt::set_shadow(
            &fmt::set_shadow(&rect, Some(ShadowPreset::Top)).unwrap(),
            None,
        )
        .unwrap();
        assert!(!shape_look(&x, None).unwrap().shadowed);
    }

    #[std::prelude::v1::test]
    fn labels_are_translated() {
        for d in DASHES {
            assert!(i18n::indonesian(dash_label(d)).is_some(), "{d}");
        }
        for a in ARROWS {
            assert!(i18n::indonesian(arrow_label(a)).is_some(), "{a}");
        }
    }
}
