use crate::test::util::new_empty_model;

// ── REGEXTEST ─────────────────────────────────────────────────────────────────

#[test]
fn test_regextest_basic_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"hello world\", \"world\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "TRUE");
}

#[test]
fn test_regextest_no_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"hello world\", \"xyz\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "FALSE");
}

#[test]
fn test_regextest_anchored() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"abc123\", \"^[a-z]+$\")");
    model._set("A2", "=REGEXTEST(\"abc123\", \"^[a-z0-9]+$\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "FALSE");
    assert_eq!(model._get_text("A2"), "TRUE");
}

#[test]
fn test_regextest_invalid_regex() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"text\", \"[\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
}

#[test]
fn test_regextest_case_sensitive() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"Hello\", \"hello\")");
    model._set("A2", "=REGEXTEST(\"Hello\", \"Hello\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "FALSE");
    assert_eq!(model._get_text("A2"), "TRUE");
}

#[test]
fn test_regextest_wrong_arg_count() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"text\")");
    model._set("A2", "=REGEXTEST(\"text\", \".\", 0, \"extra\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#ERROR!");
    assert_eq!(model._get_text("A2"), "#ERROR!");
}

// ── REGEXEXTRACT ──────────────────────────────────────────────────────────────

#[test]
fn test_regexextract_full_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"foo 42 bar\", \"[0-9]+\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "42");
}

#[test]
fn test_regexextract_capture_group() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=REGEXEXTRACT(\"2024-01-15\", \"(\\d{4})-\\d{2}-\\d{2}\")",
    );
    model.evaluate();
    // Mode 0 gives the whole match, as Excel does, groups or not.
    assert_eq!(model._get_text("A1"), "2024-01-15");
}

#[test]
fn test_regexextract_no_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"hello\", \"[0-9]+\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#N/A");
}

#[test]
fn test_regexextract_invalid_regex() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"text\", \"[\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
}

#[test]
fn test_regexextract_wrong_arg_count() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"text\")");
    model._set("A2", "=REGEXEXTRACT(\"text\", \".\", 0, 0, \"extra\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#ERROR!");
    assert_eq!(model._get_text("A2"), "#ERROR!");
}

#[test]
fn test_regexextract_all_matches() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"LonelyPlanet\", \"[A-Z][a-z]+\", 1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "Lonely");
    assert_eq!(model._get_text("B1"), "Planet");
}

#[test]
fn test_regexextract_all_matches_no_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"nouppercase\", \"[A-Z][a-z]+\", 1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#N/A");
}

// ── REGEXREPLACE ──────────────────────────────────────────────────────────────

#[test]
fn test_regexreplace_basic() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"hello world\", \"world\", \"Rust\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "hello Rust");
}

#[test]
fn test_regexreplace_all_occurrences() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"aabbcc\", \"[a-c]\", \"x\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "xxxxxx");
}

#[test]
fn test_regexreplace_no_match() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"hello\", \"[0-9]+\", \"NUM\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "hello");
}

#[test]
fn test_regexreplace_digits() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=REGEXREPLACE(\"abc 123 def 456\", \"[0-9]+\", \"N\")",
    );
    model.evaluate();
    assert_eq!(model._get_text("A1"), "abc N def N");
}

#[test]
fn test_regexreplace_invalid_regex() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"text\", \"[\", \"x\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
}

#[test]
fn test_regexreplace_wrong_arg_count() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"text\", \".\")");
    model._set(
        "A2",
        "=REGEXREPLACE(\"text\", \".\", \"x\", 0, 0, \"extra\")",
    );
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#ERROR!");
    assert_eq!(model._get_text("A2"), "#ERROR!");
}

// ── Kantor: Excel's optional arguments, PCRE features, ranges ─────────────────

#[test]
fn test_regextest_case_insensitive() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"Hello\", \"hello\", 1)");
    model._set("A2", "=REGEXTEST(\"Hello\", \"hello\", 0)");
    model._set("A3", "=REGEXTEST(\"Hello\", \"hello\", 2)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "TRUE");
    assert_eq!(model._get_text("A2"), "FALSE");
    assert_eq!(model._get_text("A3"), "#VALUE!");
}

#[test]
fn test_regextest_lookaround_and_backreference() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXTEST(\"price: 100 USD\", \"\\d+(?= USD)\")");
    model._set("A2", "=REGEXTEST(\"price: 100 EUR\", \"\\d+(?= USD)\")");
    model._set("A3", "=REGEXTEST(\"hello\", \"(l)\\1\")");
    model._set("A4", "=REGEXTEST(\"helo\", \"(l)\\1\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "TRUE");
    assert_eq!(model._get_text("A2"), "FALSE");
    assert_eq!(model._get_text("A3"), "TRUE");
    assert_eq!(model._get_text("A4"), "FALSE");
}

#[test]
fn test_regextest_over_a_range() {
    let mut model = new_empty_model();
    model._set("A1", "INV-001");
    model._set("A2", "note");
    model._set("A3", "INV-002");
    model._set("B1", "=REGEXTEST(A1:A3, \"^INV-\\d+$\")");
    model._set("D1", "=ROWS(FILTER(A1:A3, REGEXTEST(A1:A3, \"^inv\", 1)))");
    model.evaluate();
    assert_eq!(model._get_text("B1"), "TRUE");
    assert_eq!(model._get_text("B2"), "FALSE");
    assert_eq!(model._get_text("B3"), "TRUE");
    assert_eq!(model._get_text("D1"), "2");
}

#[test]
fn test_regexextract_case_insensitive() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXEXTRACT(\"Order ABC-12\", \"abc-\\d+\", 0, 1)");
    model._set("A2", "=REGEXEXTRACT(\"Order ABC-12\", \"abc-\\d+\", 0, 0)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "ABC-12");
    assert_eq!(model._get_text("A2"), "#N/A");
}

#[test]
fn test_regexextract_capture_groups_mode() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=REGEXEXTRACT(\"2024-01-15\", \"(\\d{4})-(\\d{2})-(\\d{2})\", 2)",
    );
    model._set("A2", "=REGEXEXTRACT(\"ab\", \"(a)(x)?(b)\", 2)");
    model._set("A3", "=REGEXEXTRACT(\"no digits\", \"(\\d+)\", 2)");
    model._set("A4", "=REGEXEXTRACT(\"abc\", \"b\", 2)");
    model._set("A5", "=REGEXEXTRACT(\"abc\", \"b\", 3)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "2024");
    assert_eq!(model._get_text("B1"), "01");
    assert_eq!(model._get_text("C1"), "15");
    // A group that took no part in the match is empty text.
    assert_eq!(model._get_text("A2"), "a");
    assert_eq!(model._get_text("B2"), "");
    assert_eq!(model._get_text("C2"), "b");
    assert_eq!(model._get_text("A3"), "#N/A");
    assert_eq!(model._get_text("A4"), "b");
    assert_eq!(model._get_text("A5"), "#VALUE!");
}

#[test]
fn test_regexextract_over_a_range() {
    let mut model = new_empty_model();
    model._set("A1", "Invoice 120");
    model._set("A2", "Invoice 7");
    model._set("A3", "none");
    model._set("B1", "=REGEXEXTRACT(A1:A3, \"\\d+\")");
    model.evaluate();
    assert_eq!(model._get_text("B1"), "120");
    assert_eq!(model._get_text("B2"), "7");
    assert_eq!(model._get_text("B3"), "#N/A");
}

#[test]
fn test_regexreplace_groups() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=REGEXREPLACE(\"SoniaBrown\", \"([A-Z][a-z]+)([A-Z][a-z]+)\", \"$2, $1\")",
    );
    model._set("A2", "=REGEXREPLACE(\"ab\", \"(a)\", \"$1x\")");
    model._set("A3", "=REGEXREPLACE(\"5\", \"(\\d)\", \"US$ $1\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "Brown, Sonia");
    // `$1x` is group 1 then "x", not a group named "1x".
    assert_eq!(model._get_text("A2"), "axb");
    assert_eq!(model._get_text("A3"), "US$ 5");
}

#[test]
fn test_regexreplace_occurrence() {
    let mut model = new_empty_model();
    model._set("A1", "=REGEXREPLACE(\"a1b2c3\", \"\\d\", \"#\", 0)");
    model._set("A2", "=REGEXREPLACE(\"a1b2c3\", \"\\d\", \"#\", 2)");
    model._set("A3", "=REGEXREPLACE(\"a1b2c3\", \"\\d\", \"#\", -1)");
    model._set("A4", "=REGEXREPLACE(\"a1b2c3\", \"\\d\", \"#\", 4)");
    model._set("A5", "=REGEXREPLACE(\"a1b2c3\", \"\\d\", \"#\", -3)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "a#b#c#");
    assert_eq!(model._get_text("A2"), "a1b#c3");
    assert_eq!(model._get_text("A3"), "a1b2c#");
    assert_eq!(model._get_text("A4"), "a1b2c3");
    assert_eq!(model._get_text("A5"), "a#b2c3");
}

#[test]
fn test_regexreplace_case_insensitive() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=REGEXREPLACE(\"Cat cat CAT\", \"cat\", \"dog\", 0, 1)",
    );
    model._set("A2", "=REGEXREPLACE(\"Cat cat CAT\", \"cat\", \"dog\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "dog dog dog");
    assert_eq!(model._get_text("A2"), "Cat dog CAT");
}

#[test]
fn test_regexreplace_over_a_range() {
    let mut model = new_empty_model();
    model._set("A1", "0812-555");
    model._set("A2", "0813-777");
    model._set("B1", "=REGEXREPLACE(A1:A2, \"-\", \"\")");
    model.evaluate();
    assert_eq!(model._get_text("B1"), "0812555");
    assert_eq!(model._get_text("B2"), "0813777");
}
