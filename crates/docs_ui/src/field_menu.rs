//! Field codes and the Insert ▸ Field…, Bookmark… and Cross-reference…
//! popovers, as in Word:
//!
//! - Alt+F9 shows every field's code (`{ PAGEREF _Toc1 \h }`) instead of
//!   its result, Shift+F9 the field at the caret; a code is typed in its
//!   braces ([`crate::code_inline`]) and F9 updates the field. Ctrl+F9
//!   starts a new field there (the selection becomes its code).
//! - Insert Field… lists common fields (dates with a format, page numbers,
//!   document properties, references to bookmarks) and shows the code it
//!   will insert, which can be edited.
//! - Bookmark… adds, finds and deletes bookmarks; Cross-reference… points
//!   at a heading, bookmark, footnote, endnote or caption.
//!
//! Every change is one undo step.

use docs_engine::{ChapterSep, CodeView, NumFormat, PageNumbering, Selection, XrefKind, XrefWhat};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, t_dyn, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;
use crate::fields::now;

/// Fields Insert Field… offers: code word and what it shows.
const FIELDS: [(&str, &str); 22] = [
    ("DATE", tk("Today's date")),
    ("TIME", tk("The time now")),
    ("PAGE", tk("The page number")),
    ("NUMPAGES", tk("The number of pages")),
    ("AUTHOR", tk("The document's author")),
    ("TITLE", tk("The document's title")),
    ("FILENAME", tk("The file's name")),
    ("REF", tk("A bookmark's text")),
    ("PAGEREF", tk("The page a bookmark is on")),
    ("=", tk("A formula: numbers, bookmarks, table cells")),
    ("IF", tk("One text or another, by a comparison")),
    ("COMPARE", tk("1 or 0, by a comparison")),
    ("SEQ", tk("Numbers figures, tables and other items")),
    ("STYLEREF", tk("The nearest paragraph in a style")),
    ("LISTNUM", tk("A list number inside a paragraph")),
    ("SET", tk("Gives a bookmark a value")),
    ("TOC", tk("A table of contents")),
    ("TC", tk("A table of contents entry")),
    ("INDEX", tk("An index of the XE entries")),
    ("XE", tk("An index entry")),
    ("NUMWORDS", tk("The number of words")),
    ("SECTIONPAGES", tk("The number of pages in the section")),
];

const DATE_PICTURES: [&str; 7] = [
    "d/M/yyyy",
    "dd/MM/yyyy",
    "d MMMM yyyy",
    "dddd, d MMMM yyyy",
    "M/d/yyyy",
    "MMMM d, yyyy",
    "yyyy-MM-dd",
];
const TIME_PICTURES: [&str; 3] = ["HH:mm", "h:mm am/pm", "HH:mm:ss"];
/// Number formats: what they look like and their `\*` switch.
const NUM_FORMATS: [(&str, &str); 5] = [
    ("1, 2, 3", ""),
    ("i, ii, iii", "roman"),
    ("I, II, III", "ROMAN"),
    ("a, b, c", "alphabetic"),
    ("A, B, C", "ALPHABETIC"),
];

/// The open field popover.
pub(crate) enum FieldMenu {
    Insert {
        field: usize,
        format: usize,
        bookmark: Option<String>,
        path: bool,
        input: Entity<InputState>,
        _sub: Subscription,
    },
    Bookmark {
        input: Entity<InputState>,
        _sub: Subscription,
    },
    Xref {
        kind: XrefKind,
        what: XrefWhat,
        link: bool,
        target: Option<usize>,
    },
    /// Format Page Numbers…: the section's page numbering.
    PageNumbers {
        section: usize,
        format: NumFormat,
        chapter: bool,
        level: u8,
        sep: ChapterSep,
        /// Start at the number in `start` (else continue).
        restart: bool,
        start: Entity<InputState>,
        _sub: Subscription,
    },
}

/// What each page number format looks like (Word's list).
const PAGE_FORMATS: [(NumFormat, &str); 5] = [
    (NumFormat::Decimal, "1, 2, 3, …"),
    (NumFormat::LowerLetter, "a, b, c, …"),
    (NumFormat::UpperLetter, "A, B, C, …"),
    (NumFormat::LowerRoman, "i, ii, iii, …"),
    (NumFormat::UpperRoman, "I, II, III, …"),
];

/// Chapter separators with Word's names.
const CHAPTER_SEPS: [(ChapterSep, &str); 5] = [
    (ChapterSep::Hyphen, tk("- (hyphen)")),
    (ChapterSep::Period, tk(". (period)")),
    (ChapterSep::Colon, tk(": (colon)")),
    (ChapterSep::EmDash, tk("— (em dash)")),
    (ChapterSep::EnDash, tk("– (en dash)")),
];

/// The code Insert Field… makes from its choices.
pub(crate) fn field_code(
    field: usize,
    format: usize,
    bookmark: Option<&str>,
    path: bool,
) -> String {
    let (name, _) = FIELDS[field.min(FIELDS.len() - 1)];
    match name {
        "DATE" => format!(
            "DATE \\@ \"{}\"",
            DATE_PICTURES[format.min(DATE_PICTURES.len() - 1)]
        ),
        "TIME" => format!(
            "TIME \\@ \"{}\"",
            TIME_PICTURES[format.min(TIME_PICTURES.len() - 1)]
        ),
        "PAGE" | "NUMPAGES" => match NUM_FORMATS[format.min(NUM_FORMATS.len() - 1)].1 {
            "" => name.to_owned(),
            f => format!("{name} \\* {f}"),
        },
        "FILENAME" if path => "FILENAME \\p".into(),
        "REF" | "PAGEREF" => format!("{name} {} \\h", bookmark.unwrap_or("")),
        // A starting point to edit in the code box, as Word's Field dialog
        // fills in.
        "=" => "= SUM(ABOVE)".into(),
        "IF" => "IF 1 = 1 \"Yes\" \"No\"".into(),
        "COMPARE" => "COMPARE 1 = 1".into(),
        "SEQ" => "SEQ Figure \\* ARABIC".into(),
        "STYLEREF" => "STYLEREF \"Heading 1\"".into(),
        "SET" => "SET Name 1".into(),
        "TOC" => "TOC \\o \"1-3\" \\h".into(),
        "TC" => "TC \"Entry\" \\l 1".into(),
        "INDEX" => "INDEX \\e \", \"".into(),
        "XE" => "XE \"Entry\"".into(),
        _ => name.to_owned(),
    }
}

/// Word's names for what a cross-reference shows, per kind.
fn xref_whats(kind: &XrefKind) -> Vec<(XrefWhat, &'static str)> {
    let above = (XrefWhat::AboveBelow, tk("Above/below"));
    let page = (XrefWhat::PageNumber, tk("Page number"));
    match kind {
        XrefKind::Heading => vec![
            (XrefWhat::Text, tk("Heading text")),
            page,
            (XrefWhat::Number, tk("Heading number")),
            above,
        ],
        XrefKind::Bookmark => vec![
            (XrefWhat::Text, tk("Bookmark text")),
            page,
            (XrefWhat::Number, tk("Paragraph number")),
            above,
        ],
        XrefKind::Footnote => vec![(XrefWhat::Number, tk("Footnote number")), page, above],
        XrefKind::Endnote => vec![(XrefWhat::Number, tk("Endnote number")), page, above],
        XrefKind::Caption(_) => vec![(XrefWhat::Text, tk("Entire caption")), page, above],
    }
}

fn kind_name(kind: &XrefKind) -> String {
    match kind {
        XrefKind::Heading => t("Heading").into(),
        XrefKind::Bookmark => t("Bookmark").into(),
        XrefKind::Footnote => t("Footnote").into(),
        XrefKind::Endnote => t("Endnote").into(),
        XrefKind::Caption(l) => match l.as_str() {
            "Figure" => t("Figure").into(),
            "Table" => t("Table").into(),
            "Equation" => t("Equation").into(),
            other => other.to_owned(),
        },
    }
}

impl DocsView {
    /// The fields showing their codes now.
    pub(crate) fn codes(&self) -> CodeView {
        self.main_doc()
            .field_codes()
            .map(|v| (**v).clone())
            .unwrap_or_default()
    }

    fn set_codes(&mut self, v: CodeView, cx: &mut Context<Self>) {
        match &mut self.part {
            Some(s) => s.main.set_field_codes(Some(v)),
            None => self.doc.set_field_codes(Some(v)),
        }
        self.relayout();
        self.reveal_caret.set(true);
        cx.notify();
    }

    /// Alt+F9: every field's code, or every result.
    pub fn toggle_field_codes_doc(&mut self, cx: &mut Context<Self>) {
        let v = self.codes();
        let all = !v.all;
        self.set_codes(
            CodeView {
                all,
                ids: Default::default(),
            },
            cx,
        );
        self.status = if all {
            t("Field codes shown: Alt+F9 shows the results again")
        } else {
            t("Field results shown")
        }
        .into();
    }

    /// Shift+F9: the field at the caret shows its code, or its result.
    pub fn toggle_field_code_doc(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.doc.field_at(self.sel.head) else {
            self.status = t("No field at the caret").into();
            cx.notify();
            return;
        };
        let mut v = self.codes();
        if !v.ids.remove(&f.mark_id()) {
            v.ids.insert(f.mark_id());
        }
        self.set_codes(v, cx);
    }

    /// After F9: the updated fields show their results again (Word).
    pub(crate) fn show_results_of(&mut self, ids: &[usize], cx: &mut Context<Self>) {
        let mut v = self.codes();
        if v.all || ids.iter().all(|i| !v.ids.contains(i)) {
            return;
        }
        v.ids.retain(|i| !ids.contains(i));
        self.set_codes(v, cx);
    }

    /// Ctrl+F9: a new field's braces at the caret, its code typed in them
    /// (Word's; selected text becomes the code). F9 then works it out.
    pub fn new_field_code_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (a, b) = self.sel.ordered();
        let code = self.doc.text_in(a, b).replace(docs_engine::OBJECT, "");
        let code = code.lines().next().unwrap_or_default().trim().to_owned();
        let engine = self.engine.clone();
        let sel = self.doc.insert_field(self.sel, &code, &now(), &mut |d| {
            docs_pdf::field_pages(d, &mut engine.borrow_mut())
        });
        self.edited(sel, window, cx);
        let begin = self.doc.clamp(a);
        let Some(f) = self.doc.fields().into_iter().find(|f| f.begin == begin) else {
            return;
        };
        let mut v = self.codes();
        if v.all {
            v.ids.remove(&f.mark_id());
        } else {
            v.ids.insert(f.mark_id());
        }
        self.set_codes(v, cx);
        self.start_code(begin, None, cx);
        self.status = t("Type the field's code; F9 updates it").into();
    }

    /// Insert a field with `code` at the caret, updated.
    pub(crate) fn insert_field_doc(
        &mut self,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = self.engine.clone();
        let sel = self.doc.insert_field(self.sel, code, &now(), &mut |d| {
            docs_pdf::field_pages(d, &mut engine.borrow_mut())
        });
        self.edited(sel, window, cx);
    }

    /// Insert ▸ Field….
    pub fn open_field_dialog_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.apply_field_dialog(window, cx);
            }
        });
        self.field_menu = Some(FieldMenu::Insert {
            field: 0,
            format: 2,
            bookmark: None,
            path: false,
            input,
            _sub: sub,
        });
        self.refresh_field_code(window, cx);
        cx.notify();
    }

    /// Put the code for the chosen options in the code box.
    fn refresh_field_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(FieldMenu::Insert {
            field,
            format,
            bookmark,
            path,
            input,
            ..
        }) = &self.field_menu
        else {
            return;
        };
        let code = field_code(*field, *format, bookmark.as_deref(), *path);
        input.update(cx, |s, cx| s.set_value(code, window, cx));
    }

    fn apply_field_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(FieldMenu::Insert { input, .. }) = &self.field_menu else {
            return;
        };
        let code = input.read(cx).value().to_string();
        self.field_menu = None;
        self.focus(window, cx);
        if !code.trim().is_empty() {
            self.insert_field_doc(&code, window, cx);
        }
    }

    /// Insert ▸ Bookmark….
    pub fn open_bookmark_dialog_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Bookmark name")));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.add_bookmark_doc(window, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.field_menu = Some(FieldMenu::Bookmark { input, _sub: sub });
        cx.notify();
    }

    fn bookmark_name(&self, cx: &App) -> String {
        match &self.field_menu {
            Some(FieldMenu::Bookmark { input, .. }) => input.read(cx).value().trim().to_owned(),
            _ => String::new(),
        }
    }

    fn add_bookmark_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.bookmark_name(cx);
        match self.doc.add_bookmark(self.sel, &name) {
            Ok(sel) => {
                self.field_menu = None;
                self.focus(window, cx);
                self.edited(sel, window, cx);
                self.status = tf("Bookmark “{}” added", &[&name]).into();
            }
            Err(_) => {
                self.status = t(
                    "Bookmark names start with a letter and have only letters, numbers and underscores",
                )
                .into();
            }
        }
        cx.notify();
    }

    fn go_to_bookmark_doc(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(at) = self.doc.bookmark_position(name) {
            self.field_menu = None;
            self.focus(window, cx);
            self.sel = Selection::caret(at);
            self.goal_x = None;
            self.reveal_caret.set(true);
        }
        cx.notify();
    }

    fn delete_bookmark_doc(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        if self.doc.delete_bookmark(name, sel) {
            self.edited(sel, window, cx);
            self.status = tf("Bookmark “{}” deleted", &[&name]).into();
        }
        cx.notify();
    }

    /// Insert ▸ Cross-reference….
    pub fn open_xref_dialog_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.field_menu = Some(FieldMenu::Xref {
            kind: XrefKind::Heading,
            what: XrefWhat::Text,
            link: true,
            target: None,
        });
        cx.notify();
    }

    fn apply_xref(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(FieldMenu::Xref {
            kind,
            what,
            link,
            target,
        }) = &self.field_menu
        else {
            return;
        };
        let (kind, what, link) = (kind.clone(), *what, *link);
        let Some(target) = target.and_then(|i| self.doc.xref_targets(&kind).get(i).cloned()) else {
            self.status = t("Choose what the cross-reference points to").into();
            cx.notify();
            return;
        };
        self.field_menu = None;
        self.focus(window, cx);
        let engine = self.engine.clone();
        let sel = self.doc.insert_cross_reference(
            self.sel,
            &kind,
            &target,
            what,
            link,
            &now(),
            &mut |d| docs_pdf::field_pages(d, &mut engine.borrow_mut()),
        );
        self.edited(sel, window, cx);
    }

    /// Format Page Numbers… for the caret's section (or the section of
    /// the page whose header or footer is being edited).
    pub fn open_page_numbers_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = match &self.part {
            Some(s) => self.laid.paged.pages.get(s.page).map_or(0, |p| p.section),
            None => self.doc.section_of(self.sel.head.para),
        };
        let sections = self.main_doc().sections();
        let n = sections
            .get(section)
            .map(|s| s.props.numbering())
            .unwrap_or_default();
        let start = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(n.start.unwrap_or(1).to_string(), window, cx);
            s
        });
        let sub = cx.subscribe_in(
            &start,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.apply_page_numbers(window, cx),
                InputEvent::Change => {
                    if let Some(FieldMenu::PageNumbers { restart, .. }) = &mut this.field_menu {
                        *restart = true;
                    }
                    cx.notify();
                }
                _ => {}
            },
        );
        let (level, sep) = n.chapter.unwrap_or((1, ChapterSep::Hyphen));
        self.field_menu = Some(FieldMenu::PageNumbers {
            section,
            format: n.format,
            chapter: n.chapter.is_some(),
            level,
            sep,
            restart: n.start.is_some(),
            start,
            _sub: sub,
        });
        cx.notify();
    }

    /// The numbering the Format Page Numbers popover describes.
    fn page_numbering_choice(&self, cx: &App) -> Option<(usize, PageNumbering)> {
        let Some(FieldMenu::PageNumbers {
            section,
            format,
            chapter,
            level,
            sep,
            restart,
            start,
            ..
        }) = &self.field_menu
        else {
            return None;
        };
        let start = restart.then(|| start.read(cx).value().trim().parse::<u32>().ok());
        let start = match start {
            Some(None) => return None,
            Some(Some(n)) => Some(n),
            None => None,
        };
        Some((
            *section,
            PageNumbering {
                format: *format,
                start,
                chapter: chapter.then_some((*level, *sep)),
            },
        ))
    }

    fn apply_page_numbers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, n)) = self.page_numbering_choice(cx) else {
            self.status = t("Start at: type a whole number").into();
            cx.notify();
            return;
        };
        self.field_menu = None;
        // Section properties belong to the main document: leave the
        // header or footer, change them, and come back to it.
        let back = match (self.part_target().cloned(), &self.part) {
            (Some(crate::part::PartTarget::Hf { kind, .. }), Some(s)) => Some((s.page, kind)),
            _ => None,
        };
        self.leave_part(window, cx);
        let sel = self.sel;
        self.doc.set_page_numbering(section, n, sel);
        self.edited(sel, window, cx);
        if let Some((page, kind)) = back {
            self.edit_hf_on_page(page, kind, None, window, cx);
        }
        self.focus(window, cx);
        cx.notify();
    }

    // ── rendering ──────────────────────────────────────────────────────────

    pub(crate) fn render_field_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.field_menu.as_ref()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let close = {
            let v = view.clone();
            move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    this.field_menu = None;
                    this.focus(window, cx);
                    cx.notify();
                });
            }
        };
        let panel = |id: &'static str, w: Pixels| {
            v_flex()
                .id(id)
                .key_context(crate::POPOVER_CONTEXT)
                .on_action(|_: &crate::PopoverKey, _, _| {})
                .occlude()
                .w(w)
                .gap(SPACE_3)
                .p(SPACE_4)
                .bg(tok.elevated)
                .border_1()
                .border_color(tok.border)
                .rounded(RADIUS_LG)
                .shadow(tok.shadow(Elevation::Popover))
                .text_size(FONT_SMALL)
                .text_color(tok.fg)
        };
        let title = |s: &str| {
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tok.fg)
                .child(s.to_owned())
        };
        let button = |id: SharedString, label: String, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_4)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(primary, |d| d.bg(tok.accent).text_color(tok.accent_fg))
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(tok.border)
                        .text_color(tok.fg)
                        .hover(|s| s.bg(tok.hover))
                })
                .child(label)
        };
        let item = |id: SharedString, label: String, detail: String, on: bool| {
            h_flex()
                .id(id)
                .min_h(CONTROL_SM)
                .px(SPACE_3)
                .gap(SPACE_3)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .when(on, |d| d.bg(tok.selected))
                .when(!on, |d| d.hover(|s| s.bg(tok.hover)))
                .child(div().flex_none().text_color(tok.fg).child(label))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(tok.fg_dim)
                        .child(detail),
                )
        };
        let list = |id: &'static str| {
            v_flex()
                .id(id)
                .max_h(REM * 14.)
                .overflow_y_scroll()
                .gap(px(1.))
                .p(SPACE_1)
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(tok.border)
                .bg(tok.surface)
        };
        let check = |id: &'static str, label: &'static str, on: bool| {
            h_flex()
                .id(id)
                .h(CONTROL_SM)
                .gap(SPACE_2)
                .cursor_pointer()
                .child(
                    div()
                        .flex_none()
                        .size(ICON_MD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(RADIUS_SM)
                        .border_1()
                        .border_color(tok.border_strong)
                        .when(on, |d| {
                            d.bg(tok.accent)
                                .border_color(tok.accent)
                                .text_color(tok.accent_fg)
                                .child(Icon::new(Lucide::Check).with_size(ICON_SM))
                        }),
                )
                .child(i18n::t(label))
        };
        let center = {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width / 2., b.origin.y + SPACE_7)
        };

        let (body, at, anchor) = match menu {
            FieldMenu::Insert {
                field,
                format,
                bookmark,
                path,
                input,
                ..
            } => {
                let (name, _) = FIELDS[*field];
                let mut names = list("docs-field-names").max_h(REM * 17.);
                for (i, (n, what)) in FIELDS.iter().enumerate() {
                    let v = view.clone();
                    names = names.child(
                        item(
                            format!("docs-field-{n}").into(),
                            (*n).to_owned(),
                            t_dyn(what).to_owned(),
                            i == *field,
                        )
                        .on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(FieldMenu::Insert { field, format, .. }) =
                                    &mut this.field_menu
                                {
                                    *field = i;
                                    *format = if i == 0 { 2 } else { 0 };
                                }
                                this.refresh_field_code(window, cx);
                                cx.notify();
                            });
                        }),
                    );
                }
                let mut options = v_flex().gap(SPACE_2);
                let pick = |k: usize, label: String, detail: String, on: bool| {
                    let v = view.clone();
                    item(format!("docs-field-opt-{k}").into(), label, detail, on).on_click(
                        move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(FieldMenu::Insert { format, .. }) = &mut this.field_menu
                                {
                                    *format = k;
                                }
                                this.refresh_field_code(window, cx);
                                cx.notify();
                            });
                        },
                    )
                };
                match name {
                    "DATE" | "TIME" => {
                        let pics: &[&str] = if name == "DATE" {
                            &DATE_PICTURES
                        } else {
                            &TIME_PICTURES
                        };
                        let now = now().now;
                        let id = i18n::lang() == i18n::Lang::Id;
                        let mut l = list("docs-field-formats");
                        for (k, p) in pics.iter().enumerate() {
                            l = l.child(pick(
                                k,
                                docs_engine::fields::format_date(now, p, id),
                                (*p).to_owned(),
                                k == *format,
                            ));
                        }
                        options = options
                            .child(div().text_color(tok.fg_muted).child(t("Format")))
                            .child(l);
                    }
                    "PAGE" | "NUMPAGES" => {
                        let mut l = list("docs-field-formats");
                        for (k, (shown, _)) in NUM_FORMATS.iter().enumerate() {
                            l = l.child(pick(k, (*shown).to_owned(), String::new(), k == *format));
                        }
                        options = options
                            .child(div().text_color(tok.fg_muted).child(t("Format")))
                            .child(l);
                    }
                    "FILENAME" => {
                        let v = view.clone();
                        options = options.child(
                            check("docs-field-path", tk("Add path to file name"), *path).on_click(
                                move |_, window, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        if let Some(FieldMenu::Insert { path, .. }) =
                                            &mut this.field_menu
                                        {
                                            *path = !*path;
                                        }
                                        this.refresh_field_code(window, cx);
                                        cx.notify();
                                    });
                                },
                            ),
                        );
                    }
                    "REF" | "PAGEREF" => {
                        let names = self.doc.bookmark_names(false);
                        let mut l = list("docs-field-bookmarks");
                        if names.is_empty() {
                            l = l.child(
                                div()
                                    .p(SPACE_2)
                                    .text_color(tok.fg_muted)
                                    .child(t("This document has no bookmarks")),
                            );
                        }
                        for n in names {
                            let v = view.clone();
                            let on = bookmark.as_deref() == Some(n.as_str());
                            let nm = n.clone();
                            l = l.child(
                                item(format!("docs-field-bm-{n}").into(), n, String::new(), on)
                                    .on_click(move |_, window, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            if let Some(FieldMenu::Insert { bookmark, .. }) =
                                                &mut this.field_menu
                                            {
                                                *bookmark = Some(nm.clone());
                                            }
                                            this.refresh_field_code(window, cx);
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        options = options
                            .child(div().text_color(tok.fg_muted).child(t("Bookmark")))
                            .child(l);
                    }
                    _ => {}
                }
                let v = view.clone();
                (
                    panel("docs-field-insert", REM * 36.)
                        .child(title(t("Insert Field")))
                        .child(
                            h_flex()
                                .items_start()
                                .gap(SPACE_4)
                                .child(
                                    v_flex()
                                        .w(REM * 15.)
                                        .gap(SPACE_2)
                                        .child(div().text_color(tok.fg_muted).child(t("Field")))
                                        .child(names),
                                )
                                .child(options.flex_1().min_w_0()),
                        )
                        .child(div().text_color(tok.fg_muted).child(t("Field code")))
                        .child(Input::new(input).small())
                        .child(
                            h_flex().child(div().flex_1()).child(
                                button("docs-field-insert-ok".into(), t("Insert").into(), true)
                                    .on_click(move |_, window, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.apply_field_dialog(window, cx)
                                        });
                                    }),
                            ),
                        ),
                    center,
                    Anchor::TopCenter,
                )
            }
            FieldMenu::Bookmark { input, .. } => {
                let typed = input.read(cx).value().trim().to_owned();
                let mut l = list("docs-bookmarks");
                let names = self.doc.bookmark_names(false);
                if names.is_empty() {
                    l = l.child(
                        div()
                            .p(SPACE_2)
                            .text_color(tok.fg_muted)
                            .child(t("This document has no bookmarks")),
                    );
                }
                for n in names {
                    let v = view.clone();
                    let input = input.clone();
                    let nm = n.clone();
                    l = l.child(
                        item(
                            format!("docs-bm-{n}").into(),
                            n.clone(),
                            String::new(),
                            n == typed,
                        )
                        .on_click(move |_, window, cx| {
                            input.update(cx, |s, cx| s.set_value(nm.clone(), window, cx));
                            let _ = v.update(cx, |_, cx| cx.notify());
                        }),
                    );
                }
                let add = {
                    let v = view.clone();
                    button("docs-bm-add".into(), t("Add").into(), true).on_click(
                        move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| this.add_bookmark_doc(window, cx));
                        },
                    )
                };
                let go = {
                    let v = view.clone();
                    let name = typed.clone();
                    button("docs-bm-go".into(), t("Go To").into(), false).on_click(
                        move |_, window, cx| {
                            let _ =
                                v.update(cx, |this, cx| this.go_to_bookmark_doc(&name, window, cx));
                        },
                    )
                };
                let del = {
                    let v = view.clone();
                    let name = typed.clone();
                    button("docs-bm-delete".into(), t("Delete").into(), false).on_click(
                        move |_, window, cx| {
                            let _ = v
                                .update(cx, |this, cx| this.delete_bookmark_doc(&name, window, cx));
                        },
                    )
                };
                (
                    panel("docs-bookmark", REM * 22.)
                        .child(title(t("Bookmark")))
                        .child(Input::new(input).small())
                        .child(l)
                        .child(
                            h_flex()
                                .gap(SPACE_2)
                                .child(del)
                                .child(go)
                                .child(div().flex_1())
                                .child(add),
                        ),
                    center,
                    Anchor::TopCenter,
                )
            }
            FieldMenu::Xref {
                kind,
                what,
                link,
                target,
            } => {
                let mut kinds = vec![
                    XrefKind::Heading,
                    XrefKind::Bookmark,
                    XrefKind::Footnote,
                    XrefKind::Endnote,
                ];
                kinds.extend(self.doc.caption_labels().into_iter().map(XrefKind::Caption));
                let mut kind_row = h_flex().flex_wrap().gap(SPACE_1);
                for (k, kd) in kinds.into_iter().enumerate() {
                    let v = view.clone();
                    let on = &kd == kind;
                    kind_row = kind_row.child(
                        div()
                            .id(SharedString::from(format!("docs-xref-kind-{k}")))
                            .h(CONTROL_SM)
                            .px(SPACE_3)
                            .flex()
                            .items_center()
                            .rounded(RADIUS_SM)
                            .cursor_pointer()
                            .when(on, |d| d.bg(tok.selected))
                            .when(!on, |d| {
                                d.text_color(tok.fg_muted).hover(|s| s.bg(tok.hover))
                            })
                            .child(kind_name(&kd))
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(FieldMenu::Xref {
                                        kind, what, target, ..
                                    }) = &mut this.field_menu
                                    {
                                        *what = xref_whats(&kd)[0].0;
                                        *kind = kd.clone();
                                        *target = None;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                let mut what_row = h_flex().flex_wrap().gap(SPACE_1);
                for (k, (w, label)) in xref_whats(kind).into_iter().enumerate() {
                    let v = view.clone();
                    let on = w == *what;
                    what_row = what_row.child(
                        div()
                            .id(SharedString::from(format!("docs-xref-what-{k}")))
                            .h(CONTROL_SM)
                            .px(SPACE_3)
                            .flex()
                            .items_center()
                            .rounded(RADIUS_SM)
                            .cursor_pointer()
                            .when(on, |d| d.bg(tok.selected))
                            .when(!on, |d| {
                                d.text_color(tok.fg_muted).hover(|s| s.bg(tok.hover))
                            })
                            .child(t_dyn(label).to_owned())
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(FieldMenu::Xref { what, .. }) = &mut this.field_menu
                                    {
                                        *what = w;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                let targets = self.doc.xref_targets(kind);
                let mut l = list("docs-xref-targets");
                if targets.is_empty() {
                    l = l.child(
                        div()
                            .p(SPACE_2)
                            .text_color(tok.fg_muted)
                            .child(t("Nothing of this kind in the document")),
                    );
                }
                for (k, tg) in targets.into_iter().enumerate() {
                    let v = view.clone();
                    l = l.child(
                        item(
                            format!("docs-xref-t-{k}").into(),
                            tg.label,
                            String::new(),
                            *target == Some(k),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(FieldMenu::Xref { target, .. }) = &mut this.field_menu {
                                    *target = Some(k);
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                let link_check = {
                    let v = view.clone();
                    check("docs-xref-link", tk("Insert as hyperlink"), *link).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(FieldMenu::Xref { link, .. }) = &mut this.field_menu {
                                    *link = !*link;
                                }
                                cx.notify();
                            });
                        },
                    )
                };
                let insert = {
                    let v = view.clone();
                    button("docs-xref-ok".into(), t("Insert").into(), true).on_click(
                        move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| this.apply_xref(window, cx));
                        },
                    )
                };
                (
                    panel("docs-xref", REM * 30.)
                        .child(title(t("Cross-reference")))
                        .child(div().text_color(tok.fg_muted).child(t("Reference type")))
                        .child(kind_row)
                        .child(
                            div()
                                .text_color(tok.fg_muted)
                                .child(t("Insert reference to")),
                        )
                        .child(what_row)
                        .child(l)
                        .child(
                            h_flex()
                                .child(link_check)
                                .child(div().flex_1())
                                .child(insert),
                        ),
                    center,
                    Anchor::TopCenter,
                )
            }
            FieldMenu::PageNumbers {
                format,
                chapter,
                level,
                sep,
                restart,
                start,
                ..
            } => {
                let set = |f: fn(&mut FieldMenu)| {
                    let v = view.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(m) = &mut this.field_menu {
                                f(m);
                            }
                            cx.notify();
                        });
                    }
                };
                let mut formats = list("docs-pgnum-formats");
                for (k, (f, shown)) in PAGE_FORMATS.iter().enumerate() {
                    let v = view.clone();
                    let f = *f;
                    formats = formats.child(
                        item(
                            format!("docs-pgnum-fmt-{k}").into(),
                            (*shown).to_owned(),
                            String::new(),
                            f == *format,
                        )
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(FieldMenu::PageNumbers { format, .. }) =
                                    &mut this.field_menu
                                {
                                    *format = f;
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                let chapter_check =
                    check("docs-pgnum-chapter", tk("Include chapter number"), *chapter).on_click(
                        set(|m| {
                            if let FieldMenu::PageNumbers { chapter, .. } = m {
                                *chapter = !*chapter;
                            }
                        }),
                    );
                let chapter_rows = chapter.then(|| {
                    let mut levels = h_flex().flex_wrap().gap(px(2.));
                    for l in 1..=9u8 {
                        let v = view.clone();
                        levels = levels.child(
                            item(
                                format!("docs-pgnum-lvl-{l}").into(),
                                l.to_string(),
                                String::new(),
                                l == *level,
                            )
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(FieldMenu::PageNumbers { level, .. }) =
                                        &mut this.field_menu
                                    {
                                        *level = l;
                                    }
                                    cx.notify();
                                });
                            }),
                        );
                    }
                    let mut seps = list("docs-pgnum-seps");
                    for (k, (sp, name)) in CHAPTER_SEPS.iter().enumerate() {
                        let v = view.clone();
                        let sp = *sp;
                        seps = seps.child(
                            item(
                                format!("docs-pgnum-sep-{k}").into(),
                                t_dyn(name).to_owned(),
                                String::new(),
                                sp == *sep,
                            )
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(FieldMenu::PageNumbers { sep, .. }) =
                                        &mut this.field_menu
                                    {
                                        *sep = sp;
                                    }
                                    cx.notify();
                                });
                            }),
                        );
                    }
                    v_flex()
                        .gap(SPACE_2)
                        .pl(SPACE_5)
                        .child(
                            div()
                                .text_color(tok.fg_muted)
                                .child(t("Chapter starts with style: Heading")),
                        )
                        .child(levels)
                        .child(div().text_color(tok.fg_muted).child(t("Use separator")))
                        .child(seps)
                });
                let cont = check(
                    "docs-pgnum-continue",
                    tk("Continue from previous section"),
                    !*restart,
                )
                .on_click(set(|m| {
                    if let FieldMenu::PageNumbers { restart, .. } = m {
                        *restart = false;
                    }
                }));
                let start_row = h_flex()
                    .gap(SPACE_3)
                    .child(
                        check("docs-pgnum-start", tk("Start at:"), *restart).on_click(set(|m| {
                            if let FieldMenu::PageNumbers { restart, .. } = m {
                                *restart = true;
                            }
                        })),
                    )
                    .child(div().w(REM * 5.).child(Input::new(start).small()));
                let ok = {
                    let v = view.clone();
                    button("docs-pgnum-ok".into(), t("OK").into(), true).on_click(
                        move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| this.apply_page_numbers(window, cx));
                        },
                    )
                };
                (
                    panel("docs-pgnum", REM * 22.)
                        .child(title(t("Page Number Format")))
                        .child(div().text_color(tok.fg_muted).child(t("Number format")))
                        .child(formats)
                        .child(chapter_check)
                        .children(chapter_rows)
                        .child(div().text_color(tok.fg_muted).child(t("Page numbering")))
                        .child(cont)
                        .child(start_row)
                        .child(h_flex().child(div().flex_1()).child(ok)),
                    center,
                    Anchor::TopCenter,
                )
            }
        };
        Some(kantor_ui::overlay::popover(
            anchored()
                .position(at)
                .anchor(anchor)
                .snap_to_window_with_margin(SPACE_4)
                .child(body.on_mouse_down_out(close)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn codes_from_the_choices() {
        assert_eq!(field_code(0, 2, None, false), "DATE \\@ \"d MMMM yyyy\"");
        assert_eq!(field_code(2, 1, None, false), "PAGE \\* roman");
        assert_eq!(field_code(3, 0, None, false), "NUMPAGES");
        assert_eq!(field_code(6, 0, None, true), "FILENAME \\p");
        assert_eq!(field_code(8, 0, Some("Intro"), false), "PAGEREF Intro \\h");
        assert_eq!(field_code(5, 0, None, false), "TITLE");
        assert_eq!(field_code(9, 0, None, false), "= SUM(ABOVE)");
        assert_eq!(field_code(10, 0, None, false), "IF 1 = 1 \"Yes\" \"No\"");
        assert_eq!(field_code(16, 0, None, false), "TOC \\o \"1-3\" \\h");
    }
}
