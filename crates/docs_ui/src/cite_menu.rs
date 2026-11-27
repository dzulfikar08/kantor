//! References ▸ citations in the Docs view (see
//! `docs/decisions/0007-citations.md`):
//!
//! - Insert Citation…: with Better BibTeX in a running Zotero, Zotero's own
//!   picker chooses the items; otherwise a popover searches Zotero's
//!   library (its local API) or, without Zotero, the document's own source
//!   list. The citation is written as a Zotero field.
//! - Insert Bibliography, Refresh Citations, Citation Style… (the style
//!   and language, kept as Zotero keeps them), Import Sources… (a `.bib`,
//!   RIS or CSL-JSON file into the document's source list).
//!
//! Every change to the text is one undo step.

use docs_engine::CiteItem;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_cite::zotero::{Status, Zotero};
use kantor_ui::i18n::{self, Lang, t, tf};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use serde_json::Value;

use crate::DocsView;

/// Where Insert Citation… looks for items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Library {
    /// Zotero's local API.
    Zotero,
    /// The document's own source list.
    Document,
}

/// An item to pick.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    data: Value,
    uri: Option<String>,
    summary: String,
}

impl Found {
    fn new(data: Value, uri: Option<String>) -> Self {
        Self {
            summary: docs_engine::citations::summary(&data),
            data,
            uri,
        }
    }

    fn item(&self) -> CiteItem {
        let mut it = CiteItem::new(self.data.clone());
        it.uris = self.uri.clone().into_iter().collect();
        it
    }
}

/// The open citation popover.
pub(crate) enum CiteMenu {
    Insert {
        from: Library,
        query: Entity<InputState>,
        page: Entity<InputState>,
        results: Vec<Found>,
        picked: Vec<Found>,
        /// A search in Zotero is running.
        busy: bool,
        /// Why there are no results (Zotero's local API off…).
        note: Option<String>,
        /// Tells the latest search's answer from older ones.
        seq: u64,
        _subs: Vec<Subscription>,
    },
    Style {
        /// Style ids offered (the bundled ones, the document's own first
        /// when it isn't one of them).
        styles: Vec<(String, String)>,
        choice: usize,
        /// 0: Indonesian, 1: English.
        locale: usize,
    },
}

/// The CSL locale for the interface language.
pub(crate) fn ui_locale() -> &'static str {
    match i18n::lang() {
        Lang::Id => "id-ID",
        Lang::En => "en-US",
    }
}

const LOCALES: [(&str, &str); 2] = [("id-ID", "Bahasa Indonesia"), ("en-US", "English")];

impl DocsView {
    /// References ▸ Insert Citation….
    pub fn open_citation_dialog_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        self.status = t("Looking for Zotero…").into();
        cx.notify();
        let task = cx.background_spawn(async move { Zotero::new().status() });
        cx.spawn_in(window, async move |this, cx| {
            let st: Status = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if st.better_bibtex {
                    this.pick_in_zotero(window, cx);
                } else {
                    let from = if st.local_api {
                        Library::Zotero
                    } else {
                        Library::Document
                    };
                    let note = (st.running && !st.local_api).then(|| {
                        t("Zotero is running, but its local API is off: tick “Allow other applications on this computer to communicate with Zotero” in Zotero's Settings ▸ Advanced").to_owned()
                    });
                    this.open_cite_search(from, note, window, cx);
                }
            });
        })
        .detach();
    }

    /// Better BibTeX's picker in Zotero's window.
    fn pick_in_zotero(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status = t("Choose the sources in Zotero's window, then press Enter there").into();
        cx.notify();
        let task = cx.background_spawn(async move { Zotero::new().pick() });
        cx.spawn_in(window, async move |this, cx| {
            let picked = task.await;
            let _ = this.update_in(cx, |this, window, cx| match picked {
                Ok(p) if p.is_empty() => {
                    this.status = t("No citation inserted").into();
                    cx.notify();
                }
                Ok(p) => {
                    let items: Vec<CiteItem> = p
                        .into_iter()
                        .map(|p| {
                            let mut it = CiteItem::new(p.item.data);
                            it.uris = p.item.uri.into_iter().collect();
                            it.locator = p.locator;
                            it.label = p.label;
                            it.prefix = p.prefix;
                            it.suffix = p.suffix;
                            it.suppress_author = p.suppress_author;
                            it
                        })
                        .collect();
                    this.insert_citation_items(&items, window, cx);
                }
                Err(e) => {
                    this.status = tf("Zotero: {}", &[&format!("{e:#}")]).into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_cite_search(
        &mut self,
        from: Library,
        note: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match from {
            Library::Zotero => t("Search Zotero: author, title or year"),
            Library::Document => t("Search this document's sources"),
        };
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let page = cx.new(|cx| InputState::new(window, cx).placeholder(t("Page")));
        let subs = vec![
            cx.subscribe_in(
                &query,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => this.search_citations(cx),
                    InputEvent::PressEnter { .. } => this.apply_citation(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&page, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_citation(window, cx);
                }
            }),
        ];
        query.update(cx, |s, cx| s.focus(window, cx));
        self.status = SharedString::default();
        self.cite_menu = Some(CiteMenu::Insert {
            from,
            query,
            page,
            results: Vec::new(),
            picked: Vec::new(),
            busy: false,
            note,
            seq: 0,
            _subs: subs,
        });
        self.search_citations(cx);
    }

    /// Search again for what's typed.
    fn search_citations(&mut self, cx: &mut Context<Self>) {
        let sources = self.doc.sources();
        let Some(CiteMenu::Insert {
            from,
            query,
            results,
            busy,
            seq,
            ..
        }) = &mut self.cite_menu
        else {
            return;
        };
        let q = query.read(cx).value().trim().to_owned();
        match *from {
            Library::Document => {
                let words: Vec<String> = q
                    .to_lowercase()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                *results = sources
                    .into_iter()
                    .map(|v| Found::new(v, None))
                    .filter(|f| {
                        let hay = format!("{} {}", f.summary, f.data).to_lowercase();
                        words.iter().all(|w| hay.contains(w.as_str()))
                    })
                    .take(50)
                    .collect();
                cx.notify();
            }
            Library::Zotero => {
                *seq += 1;
                let mine = *seq;
                *busy = true;
                cx.notify();
                let task = cx.background_spawn(async move { Zotero::new().search(&q, 30) });
                cx.spawn(async move |this, cx| {
                    let found = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(CiteMenu::Insert {
                            results,
                            busy,
                            seq,
                            note,
                            ..
                        }) = &mut this.cite_menu
                            && *seq == mine
                        {
                            *busy = false;
                            match found {
                                Ok(items) => {
                                    *results = items
                                        .into_iter()
                                        .map(|z| Found::new(z.data, z.uri))
                                        .collect();
                                }
                                Err(e) => *note = Some(format!("{e:#}")),
                            }
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn toggle_pick(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(CiteMenu::Insert {
            results, picked, ..
        }) = &mut self.cite_menu
            && let Some(f) = results.get(i)
        {
            match picked
                .iter()
                .position(|p| p.summary == f.summary && p.data == f.data)
            {
                Some(k) => {
                    picked.remove(k);
                }
                None => picked.push(f.clone()),
            }
        }
        cx.notify();
    }

    fn apply_citation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(CiteMenu::Insert {
            results,
            picked,
            page,
            ..
        }) = &self.cite_menu
        else {
            return;
        };
        // Enter with nothing ticked: the first result.
        let chosen: Vec<Found> = if picked.is_empty() {
            results.iter().take(1).cloned().collect()
        } else {
            picked.clone()
        };
        if chosen.is_empty() {
            self.status = t("Choose a source to cite").into();
            cx.notify();
            return;
        }
        let page = page.read(cx).value().trim().to_owned();
        let mut items: Vec<CiteItem> = chosen.iter().map(Found::item).collect();
        if !page.is_empty()
            && let Some(last) = items.last_mut()
        {
            last.locator = Some(page);
            last.label = Some("page".into());
        }
        self.cite_menu = None;
        self.focus(window, cx);
        self.insert_citation_items(&items, window, cx);
    }

    fn insert_citation_items(
        &mut self,
        items: &[CiteItem],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.doc.insert_citation(self.sel, items, Some(ui_locale())) {
            Ok(sel) => self.edited(sel, window, cx),
            Err(e) => {
                self.status = tf("Citation not inserted: {}", &[&format!("{e:#}")]).into();
                cx.notify();
            }
        }
    }

    /// References ▸ Insert Bibliography.
    pub fn insert_bibliography_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        match self.doc.insert_bibliography(self.sel, Some(ui_locale())) {
            Ok(sel) => self.edited(sel, window, cx),
            Err(e) => {
                self.status = tf("Bibliography not inserted: {}", &[&format!("{e:#}")]).into();
                cx.notify();
            }
        }
    }

    /// References ▸ Refresh Citations: every Zotero and Mendeley citation
    /// and bibliography in the document's style.
    pub fn refresh_citations_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let rev = self.doc.revision();
        match self.doc.refresh_citations(self.sel, Some(ui_locale())) {
            Ok((sel, report)) => {
                if self.doc.revision() != rev {
                    self.edited(sel, window, cx);
                }
                self.status = if report.citations == 0 && report.bibliographies == 0 {
                    if report.other > 0 {
                        t("Word's and EndNote's citations are kept as they are: refresh them in Word").into()
                    } else {
                        t("This document has no Zotero or Mendeley citations").into()
                    }
                } else if report.changed == 0 && self.doc.revision() == rev {
                    t("The citations are up to date").into()
                } else {
                    tf(
                        "Citations refreshed: {} changed, {} kept as edited by hand",
                        &[&report.changed, &report.kept],
                    )
                    .into()
                };
                cx.notify();
            }
            Err(e) => {
                self.status = tf("Citations not refreshed: {}", &[&format!("{e:#}")]).into();
                cx.notify();
            }
        }
    }

    /// References ▸ Citation Style….
    pub fn open_citation_style_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let prefs = self.doc.citation_prefs(Some(ui_locale()));
        let mut styles: Vec<(String, String)> = kantor_cite::BUNDLED
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
            .collect();
        let choice = match styles.iter().position(|(id, _)| *id == prefs.style) {
            Some(i) => i,
            None => {
                let name = kantor_cite::styles::display_name(&prefs.style);
                styles.insert(0, (prefs.style.clone(), name));
                0
            }
        };
        let locale = LOCALES
            .iter()
            .position(|(l, _)| Some(*l) == prefs.locale.as_deref())
            .unwrap_or(if ui_locale() == "id-ID" { 0 } else { 1 });
        self.cite_menu = Some(CiteMenu::Style {
            styles,
            choice,
            locale,
        });
        cx.notify();
    }

    fn apply_citation_style(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(CiteMenu::Style {
            styles,
            choice,
            locale,
        }) = &self.cite_menu
        else {
            return;
        };
        let style = styles[*choice].0.clone();
        let locale = LOCALES[*locale].0;
        if let Err(e) = kantor_cite::load_style(&style) {
            self.status = tf("Style not available: {}", &[&format!("{e:#}")]).into();
            cx.notify();
            return;
        }
        self.cite_menu = None;
        self.focus(window, cx);
        self.doc.set_citation_prefs(&style, Some(locale));
        self.refresh_citations_doc(window, cx);
    }

    /// References ▸ Import Sources…: a `.bib`, RIS or CSL-JSON file into
    /// the document's source list.
    pub fn import_sources_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("Import sources (.bib, RIS or CSL-JSON)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |this, _window, cx| {
                let mut items = Vec::new();
                for p in &paths {
                    kantor_sandbox::remember(p);
                    match kantor_cite::import::read_file(p) {
                        Ok(mut v) => items.append(&mut v),
                        Err(e) => {
                            this.status =
                                tf("Sources not imported: {}", &[&format!("{e:#}")]).into();
                            cx.notify();
                            return;
                        }
                    }
                }
                let total = items.len();
                let added = this.doc.add_sources(items);
                this.status = tf(
                    "{} sources imported ({} new): Insert Citation… lists them",
                    &[&total, &added],
                )
                .into();
                if let Some(CiteMenu::Insert { from, .. }) = &this.cite_menu
                    && *from == Library::Document
                {
                    this.search_citations(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_cite_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.cite_menu.as_ref()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let close = {
            let v = view.clone();
            move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    this.cite_menu = None;
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
        let item = |id: SharedString, label: String, on: bool| {
            h_flex()
                .id(id)
                .min_h(CONTROL_SM)
                .px(SPACE_3)
                .gap(SPACE_2)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .when(on, |d| d.bg(tok.selected))
                .when(!on, |d| d.hover(|s| s.bg(tok.hover)))
                .child(
                    div()
                        .flex_none()
                        .size(ICON_MD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |d| d.child(Icon::new(Lucide::Check).with_size(ICON_SM))),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let list = |id: &'static str| {
            v_flex()
                .id(id)
                .max_h(REM * 16.)
                .overflow_y_scroll()
                .gap(px(1.))
                .p(SPACE_1)
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(tok.border)
                .bg(tok.surface)
        };
        let center = {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width / 2., b.origin.y + SPACE_7)
        };

        let body = match menu {
            CiteMenu::Insert {
                from,
                query,
                page,
                results,
                picked,
                busy,
                note,
                ..
            } => {
                let mut rows = list("docs-cite-results");
                for (i, f) in results.iter().enumerate() {
                    let on = picked
                        .iter()
                        .any(|p| p.summary == f.summary && p.data == f.data);
                    let v = view.clone();
                    rows = rows.child(
                        item(format!("docs-cite-{i}").into(), f.summary.clone(), on).on_click(
                            move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| this.toggle_pick(i, cx));
                            },
                        ),
                    );
                }
                let empty = results.is_empty().then(|| {
                    let msg = if *busy {
                        t("Searching…")
                    } else if *from == Library::Document && self.doc.sources().is_empty() {
                        t("No sources yet: import a .bib, RIS or CSL-JSON file, or start Zotero (with its local API on, or Better BibTeX)")
                    } else {
                        t("Nothing found")
                    };
                    div().text_color(tok.fg_muted).child(msg)
                });
                let source = match from {
                    Library::Zotero => t("From your Zotero library"),
                    Library::Document => t("From this document's sources"),
                };
                let import = {
                    let v = view.clone();
                    button(
                        "docs-cite-import".into(),
                        t("Import Sources…").into(),
                        false,
                    )
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.import_sources_doc(window, cx));
                    })
                };
                let insert = {
                    let v = view.clone();
                    let label = if picked.len() > 1 {
                        tf("Insert {} sources", &[&picked.len()])
                    } else {
                        t("Insert").to_owned()
                    };
                    button("docs-cite-ok".into(), label, true).on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.apply_citation(window, cx));
                    })
                };
                panel("docs-cite", REM * 34.)
                    .child(title(t("Insert Citation")))
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .child(div().flex_1().text_color(tok.fg_muted).child(source))
                            .when(*from == Library::Document, |d| d.child(import)),
                    )
                    .children(
                        note.clone()
                            .map(|n| div().text_color(tok.fg_muted).child(n)),
                    )
                    .child(Input::new(query).small())
                    .child(rows.children(empty))
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .child(div().text_color(tok.fg_muted).child(t("Page")))
                            .child(div().w(REM * 6.).child(Input::new(page).small()))
                            .child(div().flex_1())
                            .child(insert),
                    )
            }
            CiteMenu::Style {
                styles,
                choice,
                locale,
            } => {
                let mut rows = list("docs-cite-styles");
                for (i, (_, name)) in styles.iter().enumerate() {
                    let v = view.clone();
                    rows = rows.child(
                        item(
                            format!("docs-cite-style-{i}").into(),
                            name.clone(),
                            i == *choice,
                        )
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(CiteMenu::Style { choice, .. }) = &mut this.cite_menu {
                                    *choice = i;
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                let mut langs = h_flex().gap(SPACE_1);
                for (i, (_, name)) in LOCALES.iter().enumerate() {
                    let v = view.clone();
                    langs = langs.child(
                        item(
                            format!("docs-cite-locale-{i}").into(),
                            (*name).to_owned(),
                            i == *locale,
                        )
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(CiteMenu::Style { locale, .. }) = &mut this.cite_menu {
                                    *locale = i;
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                let ok = {
                    let v = view.clone();
                    button("docs-cite-style-ok".into(), t("Apply").into(), true).on_click(
                        move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| this.apply_citation_style(window, cx));
                        },
                    )
                };
                panel("docs-cite-style", REM * 26.)
                    .child(title(t("Citation Style")))
                    .child(rows)
                    .child(
                        div()
                            .text_color(tok.fg_muted)
                            .child(t("Language of the citations")),
                    )
                    .child(langs)
                    .child(div().text_color(tok.fg_muted).child(t(
                        "Applies to Zotero and Mendeley citations; Zotero keeps the same setting",
                    )))
                    .child(h_flex().child(div().flex_1()).child(ok))
            }
        };
        Some(kantor_ui::overlay::popover(
            anchored()
                .position(center)
                .anchor(Anchor::TopCenter)
                .snap_to_window_with_margin(SPACE_4)
                .child(body.on_mouse_down_out(close)),
        ))
    }
}
