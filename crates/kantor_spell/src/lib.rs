//! Offline spell checking for Kantor (no GPUI).
//!
//! Two Hunspell dictionaries are bundled in the binary: American English
//! (`en_US`, SCOWL) and Indonesian (`id_ID`, hunspell-id). They're checked
//! with [`spellbook`], a pure-Rust Hunspell-compatible checker, so there are
//! no C libraries to build on any platform. See `dict/README.md` for where
//! the dictionaries come from and their licences.
//!
//! - [`words`] splits text into the words worth checking: URLs, e-mail
//!   addresses, file names, words with digits and ALL-CAPS words are left
//!   alone, as in Word's defaults.
//! - [`Speller`] is the process-wide checker: dictionaries load on first use
//!   (off the UI thread via [`Speller::load`]), plus the user's personal
//!   dictionary (a text file in Kantor's config folder) and the session's
//!   "Ignore All" words.
//! - [`SpellMenu`] is what the right-click menu offers for a flagged word.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

use spellbook::Dictionary;

/// A bundled dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    /// American English (`en_US`): used for every English variant.
    EnUs,
    /// Indonesian (`id_ID`).
    IdId,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::EnUs, Lang::IdId];

    /// The dictionary's code, as in settings (`en_US`, `id_ID`).
    pub fn code(self) -> &'static str {
        match self {
            Lang::EnUs => "en_US",
            Lang::IdId => "id_ID",
        }
    }

    /// Display name for menus.
    pub fn label(self) -> &'static str {
        match self {
            Lang::EnUs => "English (United States)",
            Lang::IdId => "Indonesian",
        }
    }

    /// The dictionary for a language tag (`en-US`, `en_GB`, `id`, `in-ID`
    /// as old Java/Word files write Indonesian); `None` for languages
    /// without a bundled dictionary.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Lang::EnUs),
            "id" | "in" => Some(Lang::IdId),
            _ => None,
        }
    }

    /// The default for text without a language: the setting if it names a
    /// bundled dictionary, else the system language (Indonesian → `id_ID`,
    /// anything else `en_US`).
    pub fn default_for(setting: Option<&str>) -> Lang {
        setting
            .and_then(Lang::from_tag)
            .unwrap_or_else(Lang::from_system)
    }

    /// From the system locale: Indonesian systems get `id_ID`, every other
    /// one `en_US`.
    pub fn from_system() -> Lang {
        let env = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find(|v| !v.is_empty() && v != "C" && v != "POSIX");
        let locale = env.or_else(sys_locale::get_locale).unwrap_or_default();
        Self::from_locale(&locale)
    }

    fn from_locale(locale: &str) -> Lang {
        match Lang::from_tag(locale) {
            Some(Lang::IdId) => Lang::IdId,
            _ => Lang::EnUs,
        }
    }

    fn files(self) -> (&'static str, &'static str) {
        match self {
            Lang::EnUs => (
                include_str!("../dict/en_US.aff"),
                include_str!("../dict/en_US.dic"),
            ),
            Lang::IdId => (
                include_str!("../dict/id_ID.aff"),
                include_str!("../dict/id_ID.dic"),
            ),
        }
    }

    fn index(self) -> usize {
        match self {
            Lang::EnUs => 0,
            Lang::IdId => 1,
        }
    }
}

// ── splitting text into words ────────────────────────────────────────────

/// Apostrophes that can sit inside a word (`don't`, `Jum’at`).
fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c)
}

/// Combining marks (accents typed as a separate code point).
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF)
}

/// A whitespace-free chunk that is an address or a file name: URLs
/// (`https://…`, `www.…`), e-mail addresses, paths and `name.ext`.
fn is_address(chunk: &str) -> bool {
    let c = chunk.trim_matches(|c: char| !is_word_char(c) && c != '/' && c != '\\');
    if c.contains("://") || c.starts_with("www.") || c.contains('/') || c.contains('\\') {
        return true;
    }
    if c.contains('@') || c.contains('_') {
        return true;
    }
    // A dot with a letter or digit on both sides: `kantor.app`, `v1.2`,
    // `e.g` (abbreviations are skipped too, which is harmless).
    let b = c.as_bytes();
    c.char_indices().any(|(i, ch)| {
        ch == '.'
            && i > 0
            && c[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
            && b.get(i + 1).is_some()
            && c[i + 1..].chars().next().is_some_and(char::is_alphanumeric)
    })
}

/// Whether `c` is a Latin-script letter (basic, Latin-1, Extended-A/B,
/// Extended Additional), the only script the bundled dictionaries cover.
fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}') && c.is_alphabetic()
}

/// Whether a word should be checked at all: not a single letter, no digits,
/// not ALL CAPS (acronyms), and only Latin letters (no dictionary covers
/// Chinese, Japanese, Korean, Arabic, Cyrillic… so those are never flagged).
fn worth_checking(word: &str) -> bool {
    let mut letters = 0;
    let mut upper = 0;
    for c in word.chars() {
        if c.is_numeric() {
            return false;
        }
        if c.is_alphabetic() && !is_latin_letter(c) {
            return false;
        }
        if c.is_alphabetic() {
            letters += 1;
            if c.is_uppercase() {
                upper += 1;
            }
        }
    }
    letters >= 2 && upper < letters
}

/// Byte ranges of the words in `text` that should be spell checked, in
/// order. Words keep inner apostrophes and hyphens (`don't`, `anak-anak`:
/// the dictionaries know reduplicated and hyphenated forms and check the
/// parts otherwise).
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut chunk_start = None;
    for (i, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let sep = c.is_whitespace() || c == '\u{FFFC}' || c.is_control();
        match (sep, chunk_start) {
            (true, Some(s)) => {
                chunk_words(text, s..i, &mut out);
                chunk_start = None;
            }
            (false, None) => chunk_start = Some(i),
            _ => {}
        }
    }
    out
}

fn chunk_words(text: &str, chunk: Range<usize>, out: &mut Vec<Range<usize>>) {
    let s = &text[chunk.clone()];
    if is_address(s) {
        return;
    }
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !is_word_char(chars[i].1) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        loop {
            if end < chars.len() && is_word_char(chars[end].1) {
                end += 1;
            } else if end + 1 < chars.len()
                && (is_apostrophe(chars[end].1) || chars[end].1 == '-')
                && is_word_char(chars[end + 1].1)
            {
                end += 2;
            } else {
                break;
            }
        }
        let a = chunk.start + chars[start].0;
        let b = chunk.start + chars.get(end).map_or(s.len(), |c| c.0);
        if worth_checking(&text[a..b]) {
            out.push(a..b);
        }
        i = end;
    }
}

// ── the checker ──────────────────────────────────────────────────────────

/// Words the user accepted: the personal dictionary and "Ignore All".
#[derive(Default)]
struct Accepted {
    /// Lower-cased personal words (as written in the file).
    personal: HashSet<String>,
    personal_raw: Vec<String>,
    /// Lower-cased words ignored for this session.
    ignored: HashSet<String>,
}

/// The process-wide spell checker. Cheap to query from any thread.
pub struct Speller {
    dicts: [OnceLock<Dictionary>; 2],
    accepted: RwLock<Accepted>,
    personal_path: Option<PathBuf>,
    /// Dictionary answers for words seen before (documents repeat words a
    /// lot, and Indonesian affix checks cost ~10 µs a word).
    memo: Mutex<HashMap<(Lang, String), bool>>,
}

/// Memo entries kept before it's cleared.
const MEMO_MAX: usize = 100_000;

/// Most suggestions the menu shows (as Word).
pub const MAX_SUGGESTIONS: usize = 5;

/// The personal dictionary's file name in Kantor's config folder.
pub const PERSONAL_FILE: &str = "dictionary.txt";

impl Speller {
    /// The shared checker. The personal dictionary is read on first call.
    pub fn global() -> &'static Speller {
        static S: OnceLock<Speller> = OnceLock::new();
        S.get_or_init(|| {
            Speller::with_personal(
                kantor_core::settings::config_dir().map(|d| d.join(PERSONAL_FILE)),
            )
        })
    }

    /// A checker with its personal dictionary at `path` (read now if it exists).
    pub fn with_personal(path: Option<PathBuf>) -> Self {
        let mut accepted = Accepted::default();
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            for w in text.lines().map(str::trim).filter(|w| !w.is_empty()) {
                if accepted.personal.insert(w.to_lowercase()) {
                    accepted.personal_raw.push(w.to_owned());
                }
            }
        }
        Self {
            dicts: Default::default(),
            memo: Mutex::default(),
            accepted: RwLock::new(accepted),
            personal_path: path,
        }
    }

    /// The dictionary if it's loaded (never blocks).
    pub fn dictionary(&self, lang: Lang) -> Option<&Dictionary> {
        self.dicts[lang.index()].get()
    }

    /// Load a dictionary (blocks for a fraction of a second the first
    /// time; call it off the UI thread).
    pub fn load(&self, lang: Lang) -> &Dictionary {
        self.dicts[lang.index()].get_or_init(|| {
            let t = std::time::Instant::now();
            let (aff, dic) = lang.files();
            let d = Dictionary::new(aff, dic).unwrap_or_else(|e| {
                log::error!("spelling: can't read the {} dictionary: {e}", lang.code());
                Dictionary::new("", "").expect("empty dictionary")
            });
            log::debug!("spelling: loaded {} in {:?}", lang.code(), t.elapsed());
            d
        })
    }

    pub fn is_loaded(&self, lang: Lang) -> bool {
        self.dictionary(lang).is_some()
    }

    /// Whether the user accepted `word` (personal dictionary or Ignore All).
    pub fn is_accepted(&self, word: &str) -> bool {
        let a = self.accepted.read().unwrap_or_else(|e| e.into_inner());
        if a.personal.is_empty() && a.ignored.is_empty() {
            return false;
        }
        let w = normalize(word).to_lowercase();
        a.personal.contains(&w) || a.ignored.contains(&w)
    }

    /// Whether `word` is spelled correctly; `None` while the dictionary
    /// isn't loaded.
    pub fn check(&self, lang: Lang, word: &str) -> Option<bool> {
        self.dictionary(lang)?;
        Some(self.check_in(lang, word))
    }

    /// Check with `lang`'s dictionary, loading it if needed. A hyphenated
    /// word the dictionary doesn't know is right when each part is (parts
    /// with digits or in ALL CAPS count as right: `NDA-free`).
    fn check_in(&self, lang: Lang, word: &str) -> bool {
        if self.check_word(lang, word) {
            return true;
        }
        word.contains('-')
            && word
                .split('-')
                .all(|part| !worth_checking(part) || self.check_word(lang, part))
    }

    fn check_word(&self, lang: Lang, word: &str) -> bool {
        let w = normalize(word);
        let key = (lang, w.into_owned());
        let known = self.memo.lock().ok().and_then(|m| m.get(&key).copied());
        let ok = known.unwrap_or_else(|| {
            let ok = self.load(lang).check(&key.1);
            if let Ok(mut m) = self.memo.lock() {
                if m.len() >= MEMO_MAX {
                    m.clear();
                }
                m.insert(key.clone(), ok);
            }
            ok
        });
        ok || self.is_accepted(&key.1)
    }

    /// The misspelled words of `text` (byte ranges) for `lang`, or `None`
    /// while the dictionary isn't loaded.
    pub fn misspelled(&self, lang: Lang, text: &str) -> Option<Vec<Range<usize>>> {
        self.dictionary(lang)?;
        Some(
            words(text)
                .into_iter()
                .filter(|r| !self.check_in(lang, &text[r.clone()]))
                .collect(),
        )
    }

    /// The misspelled words of `text` whose byte ranges have their own
    /// dictionaries (`spans`, by where each word starts; `None` or no span
    /// means "don't check"). Loads dictionaries as needed, so call it off
    /// the UI thread unless [`Self::is_loaded`] says they're ready.
    pub fn misspelled_in(
        &self,
        text: &str,
        spans: &[(Range<usize>, Option<Lang>)],
    ) -> Vec<Range<usize>> {
        words(text)
            .into_iter()
            .filter(|w| {
                let Some(lang) = lang_at(spans, w.start) else {
                    return false;
                };
                !self.check_in(lang, &text[w.clone()])
            })
            .collect()
    }

    /// Up to [`MAX_SUGGESTIONS`] corrections for `word`, best first (empty
    /// while the dictionary isn't loaded).
    pub fn suggest(&self, lang: Lang, word: &str) -> Vec<String> {
        let Some(dict) = self.dictionary(lang) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        dict.suggest(&normalize(word), &mut out);
        // Keep the curly apostrophe the text used.
        if word.contains('\u{2019}') {
            for s in &mut out {
                *s = s.replace('\'', "\u{2019}");
            }
        }
        out.dedup();
        rank_suggestions(word, &mut out);
        out.truncate(MAX_SUGGESTIONS);
        out
    }

    /// Accept `word` for the rest of the session ("Ignore All").
    pub fn ignore_all(&self, word: &str) {
        let mut a = self.accepted.write().unwrap_or_else(|e| e.into_inner());
        a.ignored.insert(normalize(word).to_lowercase());
    }

    /// Add `word` to the personal dictionary and save it.
    pub fn add_to_dictionary(&self, word: &str) -> std::io::Result<()> {
        if self.add_word(word) {
            self.save_personal()?;
        }
        Ok(())
    }

    /// Add `word` to the personal dictionary in memory (see
    /// [`Self::save_personal`]); false if it was already there.
    pub fn add_word(&self, word: &str) -> bool {
        let word = normalize(word).trim().to_owned();
        if word.is_empty() {
            return false;
        }
        let mut a = self.accepted.write().unwrap_or_else(|e| e.into_inner());
        if !a.personal.insert(word.to_lowercase()) {
            return false;
        }
        a.personal_raw.push(word);
        true
    }

    /// Write the personal dictionary file (one word per line, UTF-8).
    pub fn save_personal(&self) -> std::io::Result<()> {
        let Some(path) = &self.personal_path else {
            return Ok(());
        };
        let mut text = self.personal_words().join("\n");
        text.push('\n');
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// The personal dictionary's words, as added.
    pub fn personal_words(&self) -> Vec<String> {
        let a = self.accepted.read().unwrap_or_else(|e| e.into_inner());
        a.personal_raw.clone()
    }
}

/// Swapped letters are the most common typo (`teh`, `recieve`): move
/// suggestions made of exactly the word's letters to the front, keeping
/// the checker's order otherwise.
fn rank_suggestions(word: &str, out: &mut [String]) {
    let key = |w: &str| {
        let mut v: Vec<char> = w.to_lowercase().chars().collect();
        v.sort_unstable();
        v
    };
    let letters = key(word);
    out.sort_by_key(|s| key(s) != letters);
}

/// The dictionary of the span holding byte `at` (see [`Speller::misspelled_in`]).
pub fn lang_at(spans: &[(Range<usize>, Option<Lang>)], at: usize) -> Option<Lang> {
    spans
        .iter()
        .find(|(r, _)| r.contains(&at))
        .and_then(|s| s.1)
}

/// Typographic apostrophes as the dictionaries spell them.
fn normalize(word: &str) -> std::borrow::Cow<'_, str> {
    if word.contains('\u{2019}') {
        word.replace('\u{2019}', "'").into()
    } else {
        word.into()
    }
}

// ── the right-click menu ─────────────────────────────────────────────────

/// One entry of the spelling part of the right-click menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpellItem {
    /// Replace the word with this.
    Replace(String),
    /// Shown, greyed out, when there's nothing to offer.
    NoSuggestions,
    IgnoreAll,
    AddToDictionary,
}

impl SpellItem {
    pub fn label(&self) -> String {
        match self {
            SpellItem::Replace(s) => s.clone(),
            SpellItem::NoSuggestions => "(No Spelling Suggestions)".into(),
            SpellItem::IgnoreAll => "Ignore All".into(),
            SpellItem::AddToDictionary => "Add to Dictionary".into(),
        }
    }
}

/// The menu for a flagged word: where it is, and what to offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpellMenu {
    /// The word as written.
    pub word: String,
    /// Its byte range in the paragraph or text box.
    pub range: Range<usize>,
    pub lang: Lang,
    pub items: Vec<SpellItem>,
}

impl SpellMenu {
    /// The menu for `word` with `suggestions` (at most five are kept;
    /// "(No Spelling Suggestions)" when there are none), then Ignore All and
    /// Add to Dictionary, as in Word.
    pub fn new(word: &str, range: Range<usize>, lang: Lang, suggestions: Vec<String>) -> Self {
        let mut items: Vec<SpellItem> = suggestions
            .into_iter()
            .filter(|s| s != word)
            .take(MAX_SUGGESTIONS)
            .map(SpellItem::Replace)
            .collect();
        if items.is_empty() {
            items.push(SpellItem::NoSuggestions);
        }
        items.push(SpellItem::IgnoreAll);
        items.push(SpellItem::AddToDictionary);
        Self {
            word: word.to_owned(),
            range,
            lang,
            items,
        }
    }

    /// The menu for the flagged word at byte `offset` of `text`, if any
    /// (`flags` are the text's misspelled ranges). Suggestions come from
    /// the global checker.
    pub fn at(text: &str, offset: usize, flags: &[Range<usize>], lang: Lang) -> Option<Self> {
        let r = flagged_at(flags, offset)?;
        let word = text.get(r.clone())?;
        if Speller::global().is_accepted(word) {
            return None;
        }
        let suggestions = Speller::global().suggest(lang, word);
        Some(Self::new(word, r, lang, suggestions))
    }
}

/// The flagged range touching byte `offset` (a click just after a word's
/// last letter still counts, like Word).
pub fn flagged_at(flags: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    flags
        .iter()
        .find(|r| r.start <= offset && offset <= r.end)
        .cloned()
}

#[cfg(test)]
mod tests;
