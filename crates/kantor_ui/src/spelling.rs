//! Spell checking pieces shared by Docs and Slides: the "Check Spelling as
//! You Type" switch (`[spelling]` in settings, on by default), the default
//! dictionary, the red wavy underline and the right-click menu's spelling
//! items. The checker itself is [`kantor_spell`].

use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_spell::{Lang, SpellItem, SpellMenu, Speller};

use crate::i18n::t;
use crate::tokens::tokens;

/// Whether misspelled words are underlined, and the default dictionary.
struct Spelling {
    on: bool,
    lang: Lang,
}
impl Global for Spelling {}

/// Read the settings (once; later calls do nothing) and load the default
/// dictionary in the background so the first document is checked quickly.
pub fn init(cx: &mut App) {
    if cx.has_global::<Spelling>() {
        return;
    }
    let s = Settings::load().spelling;
    let lang = Lang::default_for(s.language.as_deref());
    cx.set_global(Spelling {
        on: s.check_as_you_type,
        lang,
    });
    if s.check_as_you_type {
        cx.background_spawn(async move {
            Speller::global().load(lang);
        })
        .detach();
    }
}

/// "Check Spelling as You Type" is on.
pub fn enabled(cx: &App) -> bool {
    cx.try_global::<Spelling>().is_some_and(|s| s.on)
}

/// The dictionary for text without a language of its own.
pub fn default_lang(cx: &App) -> Lang {
    cx.try_global::<Spelling>()
        .map_or_else(Lang::from_system, |s| s.lang)
}

/// Turn checking on or off in every view, and remember it.
pub fn set_enabled(on: bool, cx: &mut App) {
    let lang = default_lang(cx);
    cx.set_global(Spelling { on, lang });
    cx.refresh_windows();
    cx.background_spawn(async move {
        let mut s = Settings::load();
        if s.spelling.check_as_you_type != on {
            s.spelling.check_as_you_type = on;
            if let Err(err) = s.save() {
                log::error!("saving settings: {err:#}");
            }
        }
    })
    .detach();
}

pub fn toggle(cx: &mut App) {
    set_enabled(!enabled(cx), cx);
}

/// A red wavy underline from `origin` (its top left, window coordinates)
/// `width` long; `scale` is the view's zoom.
pub fn paint_squiggle(
    window: &mut Window,
    origin: Point<Pixels>,
    width: Pixels,
    scale: f32,
    cx: &App,
) {
    let style = UnderlineStyle {
        thickness: px((1.0 * scale).clamp(1.0, 2.5)),
        color: Some(tokens(cx).paper_misspelled),
        wavy: true,
    };
    window.paint_underline(origin, width, &style);
}

/// What a click on a spelling item does.
pub type OnSpellItem = Rc<dyn Fn(&SpellItem, &mut Window, &mut App)>;

/// The spelling part of a right-click menu: the suggestions (or a greyed
/// "(No Spelling Suggestions)"), Ignore All and Add to Dictionary, then a
/// separator.
pub fn menu_items(mut menu: PopupMenu, m: &SpellMenu, on_click: OnSpellItem) -> PopupMenu {
    for item in &m.items {
        let label = match item {
            SpellItem::Replace(s) => s.clone(),
            SpellItem::NoSuggestions => t("(No Spelling Suggestions)").to_owned(),
            SpellItem::IgnoreAll => t("Ignore All").to_owned(),
            SpellItem::AddToDictionary => t("Add to Dictionary").to_owned(),
        };
        let mut it = PopupMenuItem::new(label);
        match item {
            SpellItem::NoSuggestions => it = it.disabled(true),
            _ => {
                let item = item.clone();
                let f = on_click.clone();
                it = it.on_click(move |_, window, cx| f(&item, window, cx));
            }
        }
        if matches!(item, SpellItem::IgnoreAll) {
            menu = menu.separator();
        }
        menu = menu.item(it);
    }
    menu.separator()
}

/// Apply Ignore All / Add to Dictionary for `word` (saving the personal
/// dictionary in the background) and repaint every window.
pub fn accept_word(item: &SpellItem, word: &str, cx: &mut App) {
    match item {
        SpellItem::IgnoreAll => Speller::global().ignore_all(word),
        SpellItem::AddToDictionary => {
            if Speller::global().add_word(word) {
                cx.background_spawn(async move {
                    if let Err(err) = Speller::global().save_personal() {
                        log::error!("saving the personal dictionary: {err}");
                    }
                })
                .detach();
            }
        }
        _ => return,
    }
    cx.refresh_windows();
}
