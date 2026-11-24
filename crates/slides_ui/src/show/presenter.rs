//! The presenter view: the slide on screen (large, drawable), the next
//! one, speaker notes (scrollable, A+ / A−), the elapsed time (pause,
//! reset) and the clock, "Slide X of Y", a strip of every slide to jump
//! to, and the show's buttons (black / white screen, pointer and pens,
//! swap displays, end).

use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};

use super::ink::Tool;
use super::state::{Pos, format_elapsed};
use super::window::{ShowWindow, color_row, number_badge, theme_button, tip, tool_row};
use super::{Cover, SlideFrame, paint_show_slide};

/// Thumbnails in the slide strip, px.
const THUMB_W: f32 = 128.;
const THUMB_H: f32 = 72.;

impl ShowWindow {
    pub(super) fn render_presenter(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let s = self.show.read(cx);
        let pos = s.nav.pos();
        let (x, y) = s.nav.ordinal();
        let cover = s.cover;
        let tool = s.tool;
        let pen = s.pen_color;
        let hl = s.highlighter_color;
        let two = s.two_displays();
        let paused = s.watch.is_paused();
        let elapsed = format_elapsed(s.watch.elapsed(Instant::now()));
        let number = s.number.text().to_owned();
        let notes_px = s.notes_size_px();
        let current = s.current_frame(cx);
        let upcoming = s.nav.upcoming();
        let next = upcoming.and_then(|i| s.frame(i, cx)).map(SlideFrame::plain);
        let notes = match pos {
            Pos::Slide { index, .. } => s
                .view()
                .map(|v| v.read(cx).pres.notes_text(index))
                .unwrap_or_default(),
            Pos::End => String::new(),
        };
        let current_index = s.nav.slide();
        let n = s.nav.len();
        let strip: Vec<_> = (0..n)
            .map(|i| (i, s.frame(i, cx).map(SlideFrame::plain), s.nav.is_hidden(i)))
            .collect();
        let clock = chrono::Local::now().format("%H:%M").to_string();

        // Keep the slide on screen in view in the strip.
        if let Some(i) = current_index
            && self.strip_at != Some(i)
        {
            self.strip_at = Some(i);
            self.strip_scroll.scroll_to_item(i);
        }

        // Pointer tools (and the pen's colours) over the slide on screen.
        let tools = h_flex()
            .flex_none()
            .gap(SPACE_4)
            .child(tool_row(tool, cx, false))
            .when(tool == Tool::Pen || tool == Tool::Highlighter, |d| {
                d.child(separator(t.border))
                    .child(color_row(pen, hl, tool, cx, false))
            });

        let top = h_flex()
            .flex_none()
            .h(px(56.))
            .px(SPACE_7)
            .gap(SPACE_4)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if paused { t.fg_muted } else { t.fg })
                    .min_w(px(96.))
                    .child(elapsed),
            )
            .child(theme_button(
                "presenter-pause",
                if paused { Lucide::Play } else { Lucide::Pause },
                paused,
                tip(
                    if paused {
                        i18n::t("Resume timer")
                    } else {
                        i18n::t("Pause timer")
                    },
                    None,
                ),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.toggle_pause(cx))),
                cx,
            ))
            .child(theme_button(
                "presenter-reset",
                Lucide::RotateCcw,
                false,
                tip(i18n::t("Reset timer"), None),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.reset_timer(cx))),
                cx,
            ))
            .child(
                div()
                    .ml(SPACE_7)
                    .text_size(FONT_TITLE)
                    .text_color(t.fg_muted)
                    .child(clock),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(FONT_TITLE)
                    .font_weight(FontWeight::MEDIUM)
                    .child(match pos {
                        Pos::Slide { .. } => tf("Slide {} of {}", &[&x, &y]),
                        Pos::End => i18n::t("End of slide show").to_owned(),
                    }),
            )
            .child(div().flex_1())
            .child(theme_button(
                "presenter-black",
                Lucide::Square,
                cover == Cover::Black,
                tip(i18n::t("Black screen"), Some("B")),
                cx.listener(|this, _, _, cx| {
                    this.update_show(cx, |s, cx| s.toggle_cover(Cover::Black, cx))
                }),
                cx,
            ))
            .child(theme_button(
                "presenter-white",
                Lucide::Sun,
                cover == Cover::White,
                tip(i18n::t("White screen"), Some("W")),
                cx.listener(|this, _, _, cx| {
                    this.update_show(cx, |s, cx| s.toggle_cover(Cover::White, cx))
                }),
                cx,
            ))
            .when(two, |d| {
                d.child(theme_button(
                    "presenter-swap",
                    Lucide::ArrowLeftRight,
                    false,
                    tip(i18n::t("Swap displays"), None),
                    cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.swap_displays(cx))),
                    cx,
                ))
            })
            .when(!two, |d| {
                d.child(theme_button(
                    "presenter-slides",
                    Lucide::Presentation,
                    false,
                    tip(i18n::t("Show slides"), Some("Alt+F5")),
                    cx.listener(|this, _, _, cx| {
                        this.update_show(cx, |s, cx| s.toggle_presenter_view(cx))
                    }),
                    cx,
                ))
            })
            .child(
                div()
                    .id("presenter-end")
                    .flex_none()
                    .h(CONTROL_LG)
                    .px(SPACE_6)
                    .ml(SPACE_2)
                    .flex()
                    .items_center()
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .bg(t.danger)
                    .text_color(white())
                    .font_weight(FontWeight::MEDIUM)
                    .child(i18n::t("End show"))
                    .on_click(
                        cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.end(cx))),
                    ),
            );

        // The slide on screen: drawable; covered, it still shows (dimmed).
        let cover_label = match cover {
            Cover::None => None,
            Cover::Black => Some(i18n::t("Black screen")),
            Cover::White => Some(i18n::t("White screen")),
        };
        let current_area = match pos {
            Pos::End => self
                .slide_area("presenter-current", None, Cover::Black, cx)
                .child(centered_note(i18n::t("End of slide show, click to exit."))),
            Pos::Slide { .. } => self.slide_area("presenter-current", current, Cover::None, cx),
        }
        .when_some(cover_label, |d, label| {
            d.child(
                div()
                    .absolute()
                    .top(SPACE_4)
                    .left(SPACE_4)
                    .px(SPACE_4)
                    .py(SPACE_2)
                    .rounded(RADIUS_MD)
                    .bg(hsla(0., 0., 0.05, 0.8))
                    .text_color(white())
                    .text_size(FONT_BODY)
                    .child(label),
            )
        });
        let other = t.fg_muted;
        let next_area = div()
            .relative()
            .w_full()
            .h(relative(0.42))
            .flex_none()
            .bg(black())
            .rounded(RADIUS_LG)
            .overflow_hidden()
            .child(match next {
                Some(f) => canvas(
                    |_, _, _| {},
                    move |b, _, window, _| paint_show_slide(&f, b, None, other, window),
                )
                .size_full()
                .into_any_element(),
                None => centered_note(i18n::t("End of slide show")).into_any_element(),
            });
        let notes_lines: Vec<Div> = if notes.trim().is_empty() {
            vec![div().text_color(t.fg_muted).child(i18n::t("No notes"))]
        } else {
            notes.lines().map(|l| div().child(l.to_owned())).collect()
        };
        let side = v_flex()
            .flex_none()
            .w(relative(0.36))
            .h_full()
            .gap(SPACE_4)
            .child(caption(i18n::t("Next"), t.fg_muted))
            .child(next_area)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(caption(i18n::t("Notes"), t.fg_muted))
                    .child(div().flex_1())
                    .child(theme_button(
                        "presenter-notes-smaller",
                        Lucide::AArrowDown,
                        false,
                        tip(i18n::t("Smaller notes text"), Some("Ctrl+-")),
                        cx.listener(|this, _, _, cx| {
                            this.update_show(cx, |s, cx| s.notes_bigger(false, cx))
                        }),
                        cx,
                    ))
                    .child(theme_button(
                        "presenter-notes-bigger",
                        Lucide::AArrowUp,
                        false,
                        tip(i18n::t("Larger notes text"), Some("Ctrl+=")),
                        cx.listener(|this, _, _, cx| {
                            this.update_show(cx, |s, cx| s.notes_bigger(true, cx))
                        }),
                        cx,
                    )),
            )
            .child(
                div()
                    .id("presenter-notes")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.notes_scroll)
                    .p(SPACE_6)
                    .rounded(RADIUS_LG)
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.border)
                    .text_size(px(notes_px))
                    .line_height(relative(1.4))
                    .text_color(t.fg)
                    .children(notes_lines),
            );
        let main = h_flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .gap(SPACE_7)
            .p(SPACE_7)
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .gap(SPACE_4)
                    .child(tools)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .rounded(RADIUS_LG)
                            .overflow_hidden()
                            .bg(black())
                            .child(current_area),
                    ),
            )
            .child(side);

        let accent = t.accent;
        let thumbs = strip.into_iter().map(|(i, frame, hidden)| {
            let on = current_index == Some(i);
            v_flex()
                .id(("presenter-strip", i))
                .flex_none()
                .gap(SPACE_1)
                .items_center()
                .cursor_pointer()
                .when(hidden, |d| d.opacity(0.45))
                .child(
                    div()
                        .w(px(THUMB_W))
                        .h(px(THUMB_H))
                        .rounded(RADIUS_SM)
                        .overflow_hidden()
                        .bg(black())
                        .border_2()
                        .border_color(if on { accent } else { transparent_black() })
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |b, _, window, _| {
                                    if let Some(f) = &frame {
                                        paint_show_slide(f, b, None, other, window);
                                    }
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(if on { t.fg } else { t.fg_muted })
                        .child((i + 1).to_string()),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.update_show(cx, |s, cx| s.go_to(i, cx))),
                )
        });
        let bottom = h_flex()
            .flex_none()
            .h(px(THUMB_H + 44.))
            .px(SPACE_4)
            .gap(SPACE_4)
            .bg(t.chrome)
            .border_t_1()
            .border_color(t.border)
            .child(theme_button(
                "presenter-prev",
                Lucide::ChevronLeft,
                false,
                tip(i18n::t("Previous"), Some("Left")),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.retreat(cx))),
                cx,
            ))
            .child(
                h_flex()
                    .id("presenter-strip-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(SPACE_4)
                    .overflow_x_scroll()
                    .track_scroll(&self.strip_scroll)
                    .children(thumbs),
            )
            .child(theme_button(
                "presenter-next",
                Lucide::ChevronRight,
                false,
                tip(i18n::t("Next"), Some("Right")),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.advance(cx))),
                cx,
            ));

        v_flex()
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .child(top)
            .child(main)
            .child(bottom)
            .when(!number.is_empty(), |d| d.child(number_badge(&number)))
            .when(self.grid, |d| d.child(self.render_grid(cx)))
            .into_any_element()
    }
}

fn separator(color: Hsla) -> Div {
    div().flex_none().w(px(1.)).h(ICON_LG).mx(SPACE_2).bg(color)
}

fn caption(text: &'static str, color: Hsla) -> Div {
    div()
        .flex_none()
        .text_size(FONT_SMALL)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text)
}

/// Grey text in the middle of a dark box.
fn centered_note(text: &'static str) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(FONT_TITLE)
        .text_color(white().opacity(0.7))
        .child(text)
}
