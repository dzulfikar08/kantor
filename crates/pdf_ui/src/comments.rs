//! The Comments panel: every comment of the document in page order, with
//! its author, date, text, replies and review state (Acrobat's
//! "Completed"); reply, mark completed, delete, jump to it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pdf::annot::{Annot, Kind};
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::PdfView;
use crate::markup::Sel;

/// A kind's name in the list.
fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Highlight(_) => i18n::t("Highlight"),
        Kind::Underline(_) => i18n::t("Underline"),
        Kind::StrikeOut(_) => i18n::t("Strikethrough"),
        Kind::Note => i18n::t("Sticky Note"),
        Kind::FreeText { .. } => i18n::t("Text Box"),
        Kind::Ink(_) => i18n::t("Drawing"),
        Kind::Square => i18n::t("Rectangle"),
        Kind::Circle => i18n::t("Ellipse"),
        Kind::Line { arrow: true, .. } => i18n::t("Arrow"),
        Kind::Line { .. } => i18n::t("Line"),
        Kind::Polygon(_) => i18n::t("Polygon"),
        Kind::Stamp {
            signature: true, ..
        } => i18n::t("Signature"),
        Kind::Stamp { .. } => i18n::t("Stamp"),
        Kind::State(_) | Kind::Other(_) => "",
    }
}

fn kind_icon(k: &Kind) -> Lucide {
    match k {
        Kind::Highlight(_) => Lucide::Highlighter,
        Kind::Underline(_) => Lucide::Underline,
        Kind::StrikeOut(_) => Lucide::Strikethrough,
        Kind::Note => Lucide::StickyNote,
        Kind::FreeText { .. } => Lucide::Type,
        Kind::Ink(_) => Lucide::PenLine,
        Kind::Square => Lucide::Square,
        Kind::Circle => Lucide::Circle,
        Kind::Line { .. } => Lucide::MoveUpRight,
        Kind::Polygon(_) => Lucide::Pentagon,
        Kind::Stamp {
            signature: true, ..
        } => Lucide::Signature,
        Kind::Stamp { .. } => Lucide::Stamp,
        _ => Lucide::MessageSquare,
    }
}

impl PdfView {
    /// Reply to comment `id` on page `place`.
    pub(crate) fn start_reply(
        &mut self,
        place: usize,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Write a reply")));
        input.update(cx, |i, cx| i.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.send_reply(cx);
                this.focus(window, cx);
            }
        });
        self.markup.reply = Some((id, place, input, sub));
        cx.notify();
    }

    fn send_reply(&mut self, cx: &mut Context<Self>) {
        let Some((id, place, input, _)) = self.markup.reply.take() else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            cx.notify();
            return;
        }
        let Some(parent) = self.find_mark(&Sel {
            place,
            id: id.clone(),
        }) else {
            return;
        };
        let mut r = Annot::new(Kind::Note, parent.rect, parent.color);
        r.contents = text;
        r.author = self.markup.author.clone();
        r.reply_to = Some(id);
        self.edit_marks(place, tk("Reply"), |l| l.push(r), cx);
        cx.notify();
    }

    /// Mark comment `id` completed, or not (a review state reply).
    pub(crate) fn toggle_completed(&mut self, place: usize, id: String, cx: &mut Context<Self>) {
        let done = self.is_completed(place, &id);
        let author = self.markup.author.clone();
        self.edit_marks(
            place,
            if done {
                tk("Mark Not Completed")
            } else {
                tk("Mark Completed")
            },
            |l| {
                let parent = l.iter().find(|a| a.id == id).cloned();
                if let Some(p) = parent {
                    let mut s = Annot::new(
                        Kind::State(if done { "None" } else { "Completed" }.into()),
                        p.rect,
                        p.color,
                    );
                    s.reply_to = Some(id.clone());
                    s.author = author;
                    s.hidden = true;
                    l.push(s);
                }
            },
            cx,
        );
    }

    /// The latest review state of comment `id` is Completed (or Accepted).
    fn is_completed(&self, place: usize, id: &str) -> bool {
        self.place_marks(place).is_some_and(|l| {
            l.iter()
                .filter(|a| a.reply_to.as_deref() == Some(id))
                .filter_map(|a| match &a.kind {
                    Kind::State(s) => Some((a.modified.clone(), s.clone())),
                    _ => None,
                })
                .max_by(|a, b| a.0.cmp(&b.0))
                .is_some_and(|(_, s)| s == "Completed" || s == "Accepted")
        })
    }

    pub(crate) fn render_comments(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        self.load_all_marks(cx);
        let view = cx.entity().downgrade();
        let mut list = v_flex().gap(SPACE_2).p(SPACE_3);
        let mut count = 0;
        let n = self.page_count();
        let mut seen = std::collections::HashSet::new();
        for place in 0..n {
            let Some(p) = self.page_ref(place) else {
                continue;
            };
            // A page shown twice lists its comments once.
            if !seen.insert(p.key()) {
                continue;
            }
            let Some(marks) = self.place_marks(place) else {
                continue;
            };
            for a in marks
                .iter()
                .filter(|a| a.kind.editable() && a.reply_to.is_none())
            {
                count += 1;
                let selected = self
                    .markup
                    .sel
                    .as_ref()
                    .is_some_and(|s| s.place == place && s.id == a.id);
                let done = self.is_completed(place, &a.id);
                let replies: Vec<&Annot> = marks
                    .iter()
                    .filter(|r| {
                        r.reply_to.as_deref() == Some(&a.id) && matches!(r.kind, Kind::Note)
                    })
                    .collect();
                let (vj, vr, vc, vd) = (view.clone(), view.clone(), view.clone(), view.clone());
                let (id1, id2, id3) = (a.id.clone(), a.id.clone(), a.id.clone());
                let bounds = a.bounds();
                let mut item = v_flex()
                    .id(SharedString::from(format!("pdf-comment-{place}-{}", a.id)))
                    .gap(SPACE_1)
                    .p(SPACE_3)
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(if selected { t.accent } else { t.border })
                    .bg(t.surface)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let id = id1.clone();
                        let _ = vj.update(cx, |this, cx| {
                            this.markup.sel = Some(Sel { place, id });
                            if let Some(p) = this.page_ref(place)
                                && let Some(s) =
                                    this.doc().and_then(|d| d.model.source(p.src)).cloned()
                            {
                                let r = s.rect_to_page(p.page, bounds);
                                this.reveal(place, r, cx);
                            }
                            cx.notify();
                        });
                    })
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .child(
                                gpui_kit::component::Icon::new(kind_icon(&a.kind))
                                    .with_size(ICON_SM)
                                    .text_color(crate::markup::hsla(a.color, 1.0)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(FONT_SMALL)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(if a.author.is_empty() {
                                        kind_name(&a.kind).to_owned()
                                    } else {
                                        a.author.clone()
                                    }),
                            )
                            .when(done, |d| {
                                d.child(
                                    gpui_kit::component::Icon::new(Lucide::CircleCheck)
                                        .with_size(ICON_SM)
                                        .text_color(t.success),
                                )
                            })
                            .child(
                                div()
                                    .text_size(FONT_SMALL)
                                    .text_color(t.fg_dim)
                                    .child(tf("p. {}", &[&(place + 1)])),
                            ),
                    )
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child(format!(
                                "{} · {}",
                                kind_name(&a.kind),
                                kantor_pdf::show_date(&a.modified)
                            )),
                    )
                    .when(!a.contents.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(FONT_SMALL)
                                .line_clamp(4)
                                .child(a.contents.clone()),
                        )
                    });
                for r in replies {
                    item = item.child(
                        v_flex()
                            .ml(SPACE_4)
                            .pl(SPACE_3)
                            .border_l_2()
                            .border_color(t.border)
                            .child(
                                div()
                                    .text_size(FONT_SMALL)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!(
                                        "{} · {}",
                                        r.author,
                                        kantor_pdf::show_date(&r.modified)
                                    )),
                            )
                            .child(div().text_size(FONT_SMALL).child(r.contents.clone())),
                    );
                }
                if let Some((rid, rplace, input, _)) = &self.markup.reply
                    && *rid == a.id
                    && *rplace == place
                {
                    item = item.child(Input::new(input).xsmall());
                }
                item = item.child(
                    h_flex()
                        .gap(SPACE_1)
                        .child(
                            Button::new(SharedString::from(format!("pdf-reply-{place}-{}", a.id)))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::Reply)
                                .label(i18n::t("Reply"))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    let id = id2.clone();
                                    let _ = vr.update(cx, |this, cx| {
                                        this.start_reply(place, id, window, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("pdf-done-{place}-{}", a.id)))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::CircleCheck)
                                .label(if done {
                                    i18n::t("Reopen")
                                } else {
                                    i18n::t("Mark Completed")
                                })
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let id = id3.clone();
                                    let _ = vc.update(cx, |this, cx| {
                                        this.toggle_completed(place, id, cx)
                                    });
                                }),
                        )
                        .child(div().flex_1())
                        .child({
                            let id = a.id.clone();
                            Button::new(SharedString::from(format!("pdf-del-{place}-{}", a.id)))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::Trash)
                                .tooltip(i18n::t("Delete Comment"))
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let id = id.clone();
                                    let _ = vd.update(cx, |this, cx| {
                                        this.markup.sel = Some(Sel { place, id });
                                        this.delete_mark(cx);
                                    });
                                })
                        }),
                );
                list = list.child(item);
            }
        }
        if count == 0 {
            list = list.child(
                div()
                    .p(SPACE_4)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(i18n::t("No comments yet. Use Comment on the toolbar to highlight text, add notes, draw or stamp.")),
            );
        }
        div()
            .id("pdf-comments")
            .size_full()
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    }
}
