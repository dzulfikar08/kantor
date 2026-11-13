use super::*;

fn texts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
    ranges.iter().map(|r| &text[r.clone()]).collect()
}

fn speller() -> &'static Speller {
    // One loaded checker for every test (parsing a dictionary takes a while
    // in debug builds); no personal file.
    static S: OnceLock<Speller> = OnceLock::new();
    S.get_or_init(|| {
        let s = Speller::with_personal(None);
        s.load(Lang::EnUs);
        s.load(Lang::IdId);
        s
    })
}

#[test]
fn splits_words_and_skips_what_word_skips() {
    let t = "Hello wrld, don't  re-enter NASA x 3rd v2 https://kantor.app/x mail@kantor.app \
             readme.txt C:\\Temp snake_case e.g. (Jum\u{2019}at) \u{FFFC}objek";
    assert_eq!(
        texts(t, &words(t)),
        [
            "Hello",
            "wrld",
            "don't",
            "re-enter",
            "Jum\u{2019}at",
            "objek"
        ]
    );
    // Trailing punctuation and a trailing hyphen or apostrophe aren't part
    // of the word.
    let t = "kata-, 'quoted' end.";
    assert_eq!(texts(t, &words(t)), ["kata", "quoted", "end"]);
    assert!(words("").is_empty());
    assert!(words("ABC DEF 1234 I a").is_empty());
    // Scripts no bundled dictionary covers are never flagged; Latin with
    // accents still is.
    let t = "中文 日本語 한국어 Привет مرحبا café naïve";
    assert_eq!(texts(t, &words(t)), ["café", "naïve"]);
}

#[test]
fn english_checks_and_suggests() {
    let s = speller();
    for ok in [
        "hello",
        "Hello",
        "don't",
        "don\u{2019}t",
        "running",
        "colors",
        "re-enter",
    ] {
        assert_eq!(s.check(Lang::EnUs, ok), Some(true), "{ok}");
    }
    for bad in ["wrld", "teh", "recieve", "colrs"] {
        assert_eq!(s.check(Lang::EnUs, bad), Some(false), "{bad}");
    }
    // Hyphenated words: each part is checked, and ALL-CAPS parts skipped.
    assert_eq!(s.check(Lang::EnUs, "NDA-free"), Some(true));
    assert_eq!(s.check(Lang::EnUs, "well-knwon"), Some(false));
    let t = "Teh quick brown fox recieves the mesage.";
    let bad = s.misspelled(Lang::EnUs, t).unwrap();
    assert_eq!(texts(t, &bad), ["Teh", "recieves", "mesage"]);

    let sug = s.suggest(Lang::EnUs, "recieve");
    assert!(sug.len() <= MAX_SUGGESTIONS);
    assert_eq!(sug.first().map(String::as_str), Some("receive"), "{sug:?}");
    let sug = s.suggest(Lang::EnUs, "Teh");
    assert!(sug.iter().any(|x| x == "The"), "{sug:?}");
    // A curly apostrophe stays curly in the suggestions.
    let sug = s.suggest(Lang::EnUs, "dont\u{2019}");
    assert!(sug.iter().all(|x| !x.contains('\'')), "{sug:?}");
}

#[test]
fn indonesian_checks_and_suggests() {
    let s = speller();
    for ok in [
        "saya",
        "rumah",
        "berjalan",
        "pekerjaan",
        "dimakan",
        "mempersiapkan",
        "anak-anak",
        "Indonesia",
    ] {
        assert_eq!(s.check(Lang::IdId, ok), Some(true), "{ok}");
    }
    for bad in ["rumha", "pekerjan", "sayaa"] {
        assert_eq!(s.check(Lang::IdId, bad), Some(false), "{bad}");
    }
    let t = "Saya sedang mengerjakn tugas di rumha.";
    let bad = s.misspelled(Lang::IdId, t).unwrap();
    assert_eq!(texts(t, &bad), ["mengerjakn", "rumha"]);
    let sug = s.suggest(Lang::IdId, "rumha");
    assert!(sug.iter().any(|x| x == "rumah"), "{sug:?}");
    // English words aren't Indonesian and vice versa.
    assert_eq!(s.check(Lang::IdId, "house"), Some(false));
    assert_eq!(s.check(Lang::EnUs, "rumah"), Some(false));
}

#[test]
fn personal_dictionary_and_ignore_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join(PERSONAL_FILE);
    let s = Speller::with_personal(Some(path.clone()));
    s.load(Lang::EnUs);
    assert_eq!(s.check(Lang::EnUs, "Kantor"), Some(false));
    s.add_to_dictionary("Kantor").unwrap();
    s.add_to_dictionary("Kantor").unwrap();
    assert_eq!(s.check(Lang::EnUs, "Kantor"), Some(true));
    assert_eq!(s.check(Lang::EnUs, "kantor"), Some(true));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Kantor\n");

    // A new session reads the file back; Ignore All doesn't persist.
    s.ignore_all("Zzyzx");
    assert_eq!(s.check(Lang::EnUs, "Zzyzx"), Some(true));
    let again = Speller::with_personal(Some(path));
    again.load(Lang::EnUs);
    assert_eq!(again.personal_words(), ["Kantor"]);
    assert_eq!(again.check(Lang::EnUs, "Kantor"), Some(true));
    assert_eq!(again.check(Lang::EnUs, "Zzyzx"), Some(false));
}

#[test]
fn not_loaded_means_unknown() {
    let s = Speller::with_personal(None);
    assert_eq!(s.check(Lang::IdId, "rumah"), None);
    assert_eq!(s.misspelled(Lang::IdId, "rumha"), None);
    assert!(s.suggest(Lang::IdId, "rumha").is_empty());
}

#[test]
fn language_tags_and_defaults() {
    assert_eq!(Lang::from_tag("en-US"), Some(Lang::EnUs));
    assert_eq!(Lang::from_tag("en_GB"), Some(Lang::EnUs));
    assert_eq!(Lang::from_tag("id-ID"), Some(Lang::IdId));
    assert_eq!(Lang::from_tag("in-ID"), Some(Lang::IdId));
    assert_eq!(Lang::from_tag("id_ID.UTF-8"), Some(Lang::IdId));
    assert_eq!(Lang::from_tag("fr-FR"), None);
    assert_eq!(Lang::from_locale("id_ID.UTF-8"), Lang::IdId);
    assert_eq!(Lang::from_locale("de_DE.UTF-8"), Lang::EnUs);
    assert_eq!(Lang::default_for(Some("id_ID")), Lang::IdId);
    assert_eq!(Lang::default_for(Some("en_US")), Lang::EnUs);
}

#[test]
fn menu_offers_up_to_five_suggestions_then_ignore_and_add() {
    let many: Vec<String> = ["a", "b", "c", "d", "e", "f", "g"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let m = SpellMenu::new("x", 0..1, Lang::EnUs, many);
    assert_eq!(m.items.len(), MAX_SUGGESTIONS + 2);
    assert_eq!(m.items[0], SpellItem::Replace("a".into()));
    assert_eq!(m.items[MAX_SUGGESTIONS], SpellItem::IgnoreAll);
    assert_eq!(m.items[MAX_SUGGESTIONS + 1], SpellItem::AddToDictionary);

    let m = SpellMenu::new("qqqq", 3..7, Lang::IdId, Vec::new());
    assert_eq!(
        m.items,
        [
            SpellItem::NoSuggestions,
            SpellItem::IgnoreAll,
            SpellItem::AddToDictionary
        ]
    );
    assert_eq!(m.items[0].label(), "(No Spelling Suggestions)");
    // The word itself is never offered as its own correction.
    let m = SpellMenu::new("Teh", 0..3, Lang::EnUs, vec!["Teh".into(), "The".into()]);
    assert_eq!(m.items[0], SpellItem::Replace("The".into()));

    let flags = [4..7, 10..14];
    assert_eq!(flagged_at(&flags, 5), Some(4..7));
    assert_eq!(flagged_at(&flags, 7), Some(4..7), "just after the word");
    assert_eq!(flagged_at(&flags, 8), None);
    assert_eq!(flagged_at(&flags, 10), Some(10..14));
}
