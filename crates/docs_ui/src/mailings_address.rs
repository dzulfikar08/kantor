//! Mailings' Address Block…, Greeting Line… and Match Fields…, as Word's
//! dialogs: the choices on the left, a preview of a recipient's block or
//! greeting under them, and the address fields no column is matched to.

use std::rc::Rc;

use docs_engine::address::{
    self, ADDRESS_FIELDS, BLOCK_FIELDS, CountryRule, FieldMap, GREETING_NAMES, HOME_COUNTRIES,
    NAME_FORMATS,
};
use docs_engine::fields::Instr;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, Lang, t, t_dyn, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// Greeting Line's words before the name (Word's list, and Indonesian):
/// document text, not translated.
const BEFORE: &[&str] = &["Dear", "To", "Yth.", "Kepada Yth.", ""];
/// …and after it.
const AFTER: &[&str] = &[",", ":", "!", ".", ""];
/// Greeting lines for recipients with no usable name.
const INVALID: &[&str] = &[
    "Dear Sir or Madam,",
    "To Whom It May Concern:",
    "Dengan hormat,",
    "",
];

/// The dialog open over the Mailings bar.
#[derive(Clone, Debug)]
pub(crate) enum AddrDialog {
    Block {
        /// Index into [`NAME_FORMATS`].
        name: usize,
        company: bool,
        address: bool,
        country: CountryRule,
        /// Index into [`HOME_COUNTRIES`].
        home: usize,
        by_destination: bool,
    },
    Greeting {
        /// Indices into [`BEFORE`], [`GREETING_NAMES`], [`AFTER`],
        /// [`INVALID`].
        before: usize,
        name: usize,
        after: usize,
        invalid: usize,
    },
    /// Match Fields: each address field's column; `back` is the dialog it
    /// was opened from.
    Match {
        cols: Vec<(&'static str, Option<usize>)>,
        back: Option<Box<AddrDialog>>,
    },
}

fn indonesian() -> bool {
    i18n::lang() == Lang::Id
}

impl AddrDialog {
    fn block() -> Self {
        let home = if indonesian() { 2 } else { 0 };
        Self::Block {
            name: if indonesian() { 1 } else { 3 },
            company: true,
            address: true,
            country: CountryRule::UnlessHome,
            home,
            by_destination: true,
        }
    }

    fn greeting() -> Self {
        let id = indonesian();
        Self::Greeting {
            before: if id { 2 } else { 0 },
            name: if id { 3 } else { 0 },
            after: if id { 4 } else { 0 },
            invalid: if id { 2 } else { 0 },
        }
    }

    /// The field code the dialog would insert.
    fn code(&self) -> Option<String> {
        match self {
            Self::Block {
                name,
                company,
                address,
                country,
                home,
                by_destination,
            } => {
                let (home_name, lid) = HOME_COUNTRIES[*home];
                let mut f =
                    address::home_format(home_name, NAME_FORMATS[*name].1, *company, *address);
                if *country == CountryRule::Never {
                    f = f.replace("<<\r_COUNTRY_>>", "");
                }
                Some(address::address_block_code(
                    &f,
                    lid,
                    *country,
                    home_name,
                    *by_destination,
                ))
            }
            Self::Greeting {
                before,
                name,
                after,
                invalid,
            } => {
                let f = address::greeting_format(
                    BEFORE[*before],
                    GREETING_NAMES[*name].1,
                    AFTER[*after],
                );
                let lid = if indonesian() { 1057 } else { 1033 };
                Some(address::greeting_line_code(&f, lid, INVALID[*invalid]))
            }
            Self::Match { .. } => None,
        }
    }
}

impl DocsView {
    /// Address Block…, Greeting Line… or Match Fields… (`which`: 0, 1, 2).
    pub fn open_address_dialog_doc(
        &mut self,
        which: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mailings.is_none() {
            self.toggle_mailings_doc(window, cx);
        }
        let d = match which {
            0 => AddrDialog::block(),
            1 => AddrDialog::greeting(),
            _ => self.match_dialog(None),
        };
        if let Some(m) = &mut self.mailings {
            m.list_open = false;
            m.dialog_at = m.at;
            m.dialog = Some(d);
        }
        cx.notify();
    }

    fn match_dialog(&self, back: Option<Box<AddrDialog>>) -> AddrDialog {
        let fields = self
            .mailings
            .as_ref()
            .map(|m| m.fields())
            .unwrap_or_default();
        let map = self
            .doc
            .mail_merge()
            .map(docs_engine::MailMergeLink::field_map)
            .unwrap_or_default();
        AddrDialog::Match {
            cols: map.resolve(&fields),
            back,
        }
    }

    fn set_dialog(&mut self, d: Option<AddrDialog>, cx: &mut Context<Self>) {
        if let Some(m) = &mut self.mailings {
            m.dialog = d;
        }
        cx.notify();
    }

    fn edit_dialog(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut AddrDialog)) {
        if let Some(d) = self.mailings.as_mut().and_then(|m| m.dialog.as_mut()) {
            f(d);
        }
        cx.notify();
    }

    /// OK: insert the field, or save the matching.
    fn dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.mailings.as_ref().and_then(|m| m.dialog.clone()) else {
            return;
        };
        match d {
            AddrDialog::Match { cols, back } => {
                let fields = self
                    .mailings
                    .as_ref()
                    .map(|m| m.fields())
                    .unwrap_or_default();
                let map = FieldMap {
                    matched: cols
                        .iter()
                        .filter_map(|(f, c)| {
                            c.and_then(|c| fields.get(c))
                                .map(|n| (f.to_string(), n.clone()))
                        })
                        .collect(),
                    saved: true,
                };
                if let Some(link) = self.doc.mail_merge().cloned() {
                    let lid = if indonesian() { "id-ID" } else { "en-US" };
                    self.doc
                        .set_mail_merge(Some(link.with_field_map(&map, &fields, lid)));
                }
                self.set_dialog(back.map(|b| *b), cx);
                self.refresh_merge_view();
                self.relayout();
            }
            d => {
                let Some(code) = d.code() else {
                    return;
                };
                self.set_dialog(None, cx);
                self.insert_field_doc(&code, window, cx);
                self.refresh_merge_view();
                self.relayout();
                self.focus(window, cx);
            }
        }
        cx.notify();
    }

    /// The open Address Block, Greeting Line or Match Fields dialog.
    pub(crate) fn render_address_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let m = self.mailings.as_ref()?;
        let d = m.dialog.clone()?;
        let tok = tokens(cx);
        let view = cx.entity().downgrade();
        let fields = m.fields();
        let label = |s: &str| {
            div()
                .flex_none()
                .w(REM * 12.)
                .text_color(tok.fg_muted)
                .child(t_dyn(s).to_owned())
        };
        // A dropdown: `items` shown, `set` called with the index picked.
        let choice = |id: &'static str,
                      current: String,
                      items: Vec<String>,
                      set: Rc<dyn Fn(&mut AddrDialog, usize)>| {
            let v = view.clone();
            Button::new(id)
                .outline()
                .xsmall()
                .label(current)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (k, s) in items.iter().enumerate() {
                        let v = v.clone();
                        let set = set.clone();
                        menu =
                            menu.item(PopupMenuItem::new(s.clone()).on_click(move |_, _, cx| {
                                let set = set.clone();
                                let _ =
                                    v.update(cx, |this, cx| this.edit_dialog(cx, |d| set(d, k)));
                            }));
                    }
                    menu
                })
        };
        let check =
            |id: &'static str, text: &'static str, on: bool, set: Rc<dyn Fn(&mut AddrDialog)>| {
                let v = view.clone();
                h_flex()
                    .id(id)
                    .gap(SPACE_2)
                    .items_center()
                    .cursor_pointer()
                    .child(crate::mailings::check_box(on, &tok))
                    .child(t(text))
                    .on_click(move |_, _, cx| {
                        let set = set.clone();
                        let _ = v.update(cx, |this, cx| this.edit_dialog(cx, |d| set(d)));
                    })
            };
        let none = || t("(none)").to_owned();
        let shown = |s: &str| if s.is_empty() { none() } else { s.to_owned() };

        let mut body = v_flex().gap(SPACE_3);
        let title;
        match &d {
            AddrDialog::Block {
                name,
                company,
                address,
                country,
                home,
                by_destination,
            } => {
                title = t("Insert Address Block");
                body = body
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .items_center()
                            .child(label(tk("Recipient's name")))
                            .child(choice(
                                "ab-name",
                                NAME_FORMATS[*name].0.to_owned(),
                                NAME_FORMATS.iter().map(|(s, _)| s.to_string()).collect(),
                                Rc::new(|d, k| {
                                    if let AddrDialog::Block { name, .. } = d {
                                        *name = k;
                                    }
                                }),
                            )),
                    )
                    .child(check(
                        "ab-company",
                        tk("Insert company name"),
                        *company,
                        Rc::new(|d| {
                            if let AddrDialog::Block { company, .. } = d {
                                *company = !*company;
                            }
                        }),
                    ))
                    .child(check(
                        "ab-address",
                        tk("Insert postal address"),
                        *address,
                        Rc::new(|d| {
                            if let AddrDialog::Block { address, .. } = d {
                                *address = !*address;
                            }
                        }),
                    ));
                let rules = [
                    (
                        CountryRule::Never,
                        tk("Never include the country/region in the address"),
                    ),
                    (
                        CountryRule::Always,
                        tk("Always include the country/region in the address"),
                    ),
                    (
                        CountryRule::UnlessHome,
                        tk("Only include the country/region if different than:"),
                    ),
                ];
                for (k, (rule, text)) in rules.into_iter().enumerate() {
                    let v = view.clone();
                    let on = *country == rule;
                    let mut row = h_flex()
                        .gap(SPACE_2)
                        .items_center()
                        .when(!*address, |d| d.opacity(0.5))
                        .child(
                            h_flex()
                                .id(("ab-country", k))
                                .gap(SPACE_2)
                                .items_center()
                                .cursor_pointer()
                                .child(radio(on, &tok))
                                .child(t(text))
                                .on_click(move |_, _, cx| {
                                    let _ = v.update(cx, |this, cx| {
                                        this.edit_dialog(cx, |d| {
                                            if let AddrDialog::Block { country, .. } = d {
                                                *country = rule;
                                            }
                                        })
                                    });
                                }),
                        );
                    if rule == CountryRule::UnlessHome {
                        row = row.child(choice(
                            "ab-home",
                            t_dyn(HOME_COUNTRIES[*home].0).to_owned(),
                            HOME_COUNTRIES
                                .iter()
                                .map(|(c, _)| t_dyn(c).to_owned())
                                .collect(),
                            Rc::new(|d, k| {
                                if let AddrDialog::Block { home, .. } = d {
                                    *home = k;
                                }
                            }),
                        ));
                    }
                    body = body.child(row);
                }
                body = body.child(check(
                    "ab-dest",
                    tk("Format address according to the destination country/region"),
                    *by_destination,
                    Rc::new(|d| {
                        if let AddrDialog::Block { by_destination, .. } = d {
                            *by_destination = !*by_destination;
                        }
                    }),
                ));
            }
            AddrDialog::Greeting {
                before,
                name,
                after,
                invalid,
            } => {
                title = t("Insert Greeting Line");
                body = body
                    .child(
                        div()
                            .text_color(tok.fg_muted)
                            .child(t("Greeting line format")),
                    )
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .items_center()
                            .child(choice(
                                "gl-before",
                                shown(BEFORE[*before]),
                                BEFORE.iter().map(|s| shown(s)).collect(),
                                Rc::new(|d, k| {
                                    if let AddrDialog::Greeting { before, .. } = d {
                                        *before = k;
                                    }
                                }),
                            ))
                            .child(choice(
                                "gl-name",
                                GREETING_NAMES[*name].0.to_owned(),
                                GREETING_NAMES.iter().map(|(s, _)| s.to_string()).collect(),
                                Rc::new(|d, k| {
                                    if let AddrDialog::Greeting { name, .. } = d {
                                        *name = k;
                                    }
                                }),
                            ))
                            .child(choice(
                                "gl-after",
                                shown(AFTER[*after]),
                                AFTER.iter().map(|s| shown(s)).collect(),
                                Rc::new(|d, k| {
                                    if let AddrDialog::Greeting { after, .. } = d {
                                        *after = k;
                                    }
                                }),
                            )),
                    )
                    .child(
                        div()
                            .text_color(tok.fg_muted)
                            .child(t("Greeting line for invalid recipient names")),
                    )
                    .child(choice(
                        "gl-invalid",
                        shown(INVALID[*invalid]),
                        INVALID.iter().map(|s| shown(s)).collect(),
                        Rc::new(|d, k| {
                            if let AddrDialog::Greeting { invalid, .. } = d {
                                *invalid = k;
                            }
                        }),
                    ));
            }
            AddrDialog::Match { cols, .. } => {
                title = t("Match Fields");
                body = body.child(div().text_color(tok.fg_muted).child(t(
                    "Address Block and Greeting Line take these fields from the columns of your list.",
                )));
                let mut items: Vec<String> = vec![t("(not matched)").to_owned()];
                items.extend(fields.iter().cloned());
                let mut grid = v_flex().gap(SPACE_1);
                for (k, f) in BLOCK_FIELDS.iter().enumerate() {
                    let current = cols
                        .iter()
                        .find(|(x, _)| x == f)
                        .and_then(|(_, c)| *c)
                        .and_then(|c| fields.get(c).cloned())
                        .unwrap_or_else(|| t("(not matched)").to_owned());
                    let field = *f;
                    grid = grid.child(h_flex().gap(SPACE_2).items_center().child(label(f)).child(
                        choice(
                            MATCH_IDS[k],
                            current,
                            items.clone(),
                            Rc::new(move |d, pick| {
                                if let AddrDialog::Match { cols, .. } = d
                                    && let Some(e) = cols.iter_mut().find(|(x, _)| *x == field)
                                {
                                    e.1 = pick.checked_sub(1);
                                }
                            }),
                        ),
                    ));
                }
                body = body.child(grid);
                if self.doc.mail_merge().is_none() {
                    body = body.child(
                        div()
                            .text_color(tok.fg_muted)
                            .child(t("Select recipients first (Mailings ▸ Select Recipients)")),
                    );
                }
            }
        }

        // Preview and what isn't matched (Address Block, Greeting Line).
        if let Some(code) = d.code() {
            let i = Instr::parse(&code);
            let lines: Vec<String> = match &m.recipients {
                Some(r) if !m.records.is_empty() => {
                    let row = m.records.get(m.dialog_at.min(m.records.len() - 1)).copied();
                    let cols = self.doc.address_columns(&r.data);
                    let get = |f: &str| cols.value(&r.data, row, f);
                    if matches!(d, AddrDialog::Block { .. }) {
                        address::address_block(&i, &get)
                    } else {
                        vec![address::greeting_line(&i, &get)]
                    }
                }
                _ => vec![t("Select recipients first (Mailings ▸ Select Recipients)").to_owned()],
            };
            let count = m.records.len();
            let step = |id: &'static str, icon: Lucide, by: isize| {
                let v = view.clone();
                div()
                    .id(id)
                    .size(CONTROL_SM)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(tok.hover))
                    .child(Icon::new(icon).with_size(ICON_MD))
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(m) = &mut this.mailings {
                                let n = m.records.len() as isize;
                                m.dialog_at =
                                    (m.dialog_at as isize + by).clamp(0, (n - 1).max(0)) as usize;
                            }
                            cx.notify();
                        });
                    })
            };
            body = body.child(
                h_flex()
                    .gap(SPACE_2)
                    .items_center()
                    .child(div().flex_1().text_color(tok.fg_muted).child(t("Preview")))
                    .when(count > 0, |d| {
                        d.child(step("ad-prev", Lucide::ChevronLeft, -1))
                            .child(div().text_color(tok.fg_muted).child(tf(
                                "Record {} of {}",
                                &[&(m.dialog_at.min(count - 1) + 1), &count],
                            )))
                            .child(step("ad-next", Lucide::ChevronRight, 1))
                    }),
            );
            body = body.child(
                v_flex()
                    .id("ad-preview")
                    .min_h(REM * 6.)
                    .p(SPACE_3)
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(tok.border)
                    .bg(tok.surface)
                    .children(lines.into_iter().map(|l| div().child(l))),
            );
            if let Some(r) = &m.recipients {
                let cols = self.doc.address_columns(&r.data);
                let missing: Vec<&str> = address::fields_used(&code)
                    .into_iter()
                    .filter(|f| !cols.has(f))
                    .map(t_dyn)
                    .collect();
                if !missing.is_empty() {
                    body = body.child(
                        div()
                            .text_color(tok.fg_muted)
                            .child(tf("Not matched: {}", &[&missing.join(", ")])),
                    );
                }
            }
        }

        let v = view.clone();
        let cancel = Button::new("ad-cancel")
            .ghost()
            .small()
            .label(t("Cancel"))
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| {
                    let back = match this.mailings.as_ref().and_then(|m| m.dialog.clone()) {
                        Some(AddrDialog::Match { back, .. }) => back.map(|b| *b),
                        _ => None,
                    };
                    this.set_dialog(back, cx);
                    this.focus(window, cx);
                });
            });
        let v = view.clone();
        let ok = Button::new("ad-ok")
            .primary()
            .small()
            .label(t("OK"))
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.dialog_ok(window, cx));
            });
        let v = view.clone();
        let is_match = matches!(d, AddrDialog::Match { .. });
        let match_btn = Button::new("ad-match")
            .outline()
            .small()
            .label(t("Match Fields…"))
            .on_click(move |_, _, cx| {
                let _ = v.update(cx, |this, cx| {
                    let back = this
                        .mailings
                        .as_ref()
                        .and_then(|m| m.dialog.clone())
                        .map(Box::new);
                    let d = this.match_dialog(back);
                    this.set_dialog(Some(d), cx);
                });
            });
        let panel = v_flex()
            .id("docs-mm-address")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 34.)
            .gap(SPACE_4)
            .p(SPACE_4)
            .bg(tok.elevated)
            .border_1()
            .border_color(tok.border)
            .rounded(RADIUS_LG)
            .shadow(tok.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(tok.fg)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(body)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .when(!is_match, |d| d.child(match_btn))
                    .child(div().flex_1())
                    .child(cancel)
                    .child(ok),
            );
        let at = {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width / 2., b.origin.y + SPACE_7)
        };
        Some(kantor_ui::overlay::popover(
            anchored()
                .position(at)
                .anchor(Anchor::TopCenter)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

/// Address field and country names, shown with `t_dyn`.
const _LABELS: [&str; 23] = [
    tk("Courtesy Title"),
    tk("First Name"),
    tk("Middle Name"),
    tk("Last Name"),
    tk("Suffix"),
    tk("Nickname"),
    tk("Company"),
    tk("Address 1"),
    tk("Address 2"),
    tk("City"),
    tk("State"),
    tk("Postal Code"),
    tk("Country or Region"),
    tk("Spouse/Partner Courtesy Title"),
    tk("Spouse/Partner First Name"),
    tk("United States of America"),
    tk("United Kingdom"),
    tk("Indonesia"),
    tk("Germany"),
    tk("France"),
    tk("Japan"),
    tk("Australia"),
    tk("Netherlands"),
];

/// Element ids of Match Fields' dropdowns (one per [`BLOCK_FIELDS`]).
const MATCH_IDS: [&str; 15] = [
    "mf-0", "mf-1", "mf-2", "mf-3", "mf-4", "mf-5", "mf-6", "mf-7", "mf-8", "mf-9", "mf-10",
    "mf-11", "mf-12", "mf-13", "mf-14",
];

const _: () = assert!(MATCH_IDS.len() == BLOCK_FIELDS.len());
const _: () = assert!(ADDRESS_FIELDS.len() > BLOCK_FIELDS.len());

fn radio(on: bool, tok: &kantor_ui::Tokens) -> Div {
    div()
        .flex_none()
        .size(ICON_MD)
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if on { tok.accent } else { tok.border_strong })
        .when(on, |d| {
            d.child(div().size(px(6.)).rounded_full().bg(tok.accent))
        })
}
