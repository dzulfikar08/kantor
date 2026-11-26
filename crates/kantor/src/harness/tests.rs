use kantor_ai::agent::scripted::{Scripted, call};
use kantor_ai::agent::{self, Ending, Event, Limits};
use serde_json::{Value, json};

use super::*;

fn tools(dir: &Path) -> DocTools {
    DocTools::new(dir, App::ALL.to_vec(), true).unwrap()
}

fn ok(out: &ToolOutput) -> &str {
    assert!(!out.error, "{}", out.text);
    &out.text
}

#[std::prelude::v1::test]
fn paths_stay_in_the_work_folder() {
    let dir = tempfile::tempdir().unwrap();
    let t = tools(dir.path());
    assert!(t.resolve("a/b.json").is_ok());
    assert!(t.resolve("./a.json").is_ok());
    assert!(t.resolve("../a.json").is_err());
    assert!(t.resolve("a/../../b").is_err());
    assert!(t.resolve("/etc/passwd").is_err());
    let inside = t.root().join("x.json");
    assert_eq!(t.resolve(&inside.to_string_lossy()).unwrap(), inside);
    assert!(t.resolve("").is_err());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(t.resolve("link/x.json").is_err());
    }
    // Pictures named in JSON too.
    let root = t.root().to_path_buf();
    assert!(
        t.check_paths(
            &json!({"elements": [{"type": "picture", "path": "../x.png"}]}),
            &root
        )
        .is_err()
    );
    assert!(
        t.check_paths(&json!({"cover": {"logo": "/etc/x.png"}}), &root)
            .is_err()
    );
    assert!(
        t.check_paths(&json!({"background": {"picture": "img/a.png"}}), &root)
            .is_ok()
    );
}

#[std::prelude::v1::test]
fn the_users_files_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("data.csv"), "a,b\n1,2\n").unwrap();
    let mut t = tools(dir.path());
    let c = Cancel::default();
    let out = t.call(
        "write_file",
        &json!({"path": "data.csv", "content": "x"}),
        &c,
    );
    assert!(out.error);
    assert!(out.text.contains("data-new.csv"), "{}", out.text);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("data.csv")).unwrap(),
        "a,b\n1,2\n"
    );
    // Reading it is fine, and new files can be written and changed.
    assert!(ok(&t.call("read_file", &json!({"path": "data.csv"}), &c)).contains("1,2"));
    ok(&t.call(
        "write_file",
        &json!({"path": "out/notes.md", "content": "one two two"}),
        &c,
    ));
    let r = t.call(
        "replace_in_file",
        &json!({"path": "out/notes.md", "old": "two", "new": "three"}),
        &c,
    );
    assert!(r.error && r.text.contains("2 times"), "{}", r.text);
    ok(&t.call(
        "replace_in_file",
        &json!({"path": "out/notes.md", "old": "two", "new": "three", "all": true}),
        &c,
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out/notes.md")).unwrap(),
        "one three three"
    );
    let listed = ok(&t.call("list_files", &json!({}), &c)).to_owned();
    assert!(
        listed.contains("data.csv  8 B  (user's file: read-only)"),
        "{listed}"
    );
    assert!(listed.contains("out/notes.md"), "{listed}");
    // Invalid JSON is written but reported.
    let bad = t.call("write_file", &json!({"path": "d.json", "content": "{"}), &c);
    assert!(bad.error && bad.text.contains("not valid JSON"));
}

fn deck() -> Value {
    json!({"units": "in", "slides": [
        {"title": "Kopi Senja", "notes": "Open with the question.",
         "elements": [{"type": "text", "x": 1, "y": 3, "w": 8, "h": 1, "text": "Hello", "size": 24}]},
        {"title": "Plan", "notes": "The plan.",
         "elements": [{"type": "text", "x": 1, "y": 3, "w": 8, "h": 1, "text": "Plan", "size": 24}]},
    ]})
}

fn run_job(
    dir: &Path,
    turns: Vec<kantor_ai::agent::scripted::Turn>,
) -> (agent::Outcome, Vec<Event>, Scripted) {
    let model = Scripted::new(turns);
    let mut t = tools(dir);
    let mut events = Vec::new();
    let out = agent::run(
        &model,
        &mut t,
        None,
        "make a deck",
        &Limits::default(),
        &Cancel::default(),
        &mut |e| events.push(e.clone()),
    );
    (out, events, model)
}

#[std::prelude::v1::test]
fn a_deck_is_built_checked_and_seen() {
    let dir = tempfile::tempdir().unwrap();
    let (out, events, model) = run_job(
        dir.path(),
        vec![
            call(
                "write_file",
                json!({"path": "deck.json", "content": deck().to_string()}),
            ),
            call(
                "build",
                json!({"source": "deck.json", "output": "deck.pptx"}),
            ),
            call("look", json!({"file": "deck.pptx", "pages": "2"})),
            call(
                "edit",
                json!({"file": "deck.pptx", "output": "deck2.pptx", "ops": [
                    {"tool": "add_slide", "args": {"title": "Ask"}}
                ]}),
            ),
            call("list", json!({"file": "deck2.pptx", "slide": 3})),
            call(
                "finish",
                json!({"summary": "Two decks.", "outputs": ["deck.pptx", "deck2.pptx"]}),
            ),
        ],
    );
    let results: Vec<(String, bool, String, Vec<String>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolResult {
                name,
                ok,
                text,
                images,
                ..
            } => Some((name.clone(), *ok, text.clone(), images.clone())),
            _ => None,
        })
        .collect();
    assert!(
        matches!(&out.ending, Ending::Finished { outputs, .. } if outputs == &["deck.pptx", "deck2.pptx"]),
        "{:?}\n{results:#?}",
        out.ending
    );
    let build = &results[1];
    assert!(build.1, "{}", build.2);
    assert!(
        build.2.contains("built deck.pptx (2 slides)"),
        "{}",
        build.2
    );
    assert!(
        build.2.contains("check: deck.pptx: 2 slides checked"),
        "{}",
        build.2
    );
    assert_eq!(build.3, vec!["deck.pptx contact sheet".to_owned()]);
    assert!(build.2.contains("not part of the document"), "{}", build.2);
    assert!(dir.path().join("shots/deck-sheet.png").is_file());
    let look = &results[2];
    assert_eq!(look.3, vec!["deck.pptx slide 2".to_owned()]);
    assert!(!look.2.contains("not part of the document"), "{}", look.2);
    assert!(dir.path().join("shots/deck-02.png").is_file());
    assert!(
        results[3].2.contains("saved deck2.pptx"),
        "{}",
        results[3].2
    );
    assert!(results[4].2.contains("\"Ask\""), "{}", results[4].2);
    // The pictures reached the model as data URLs.
    let sent = serde_json::to_string(&model.sent(3)).unwrap();
    assert!(sent.contains("data:image/png;base64,iVBORw0KGgo"));
    // No temporary folders left behind.
    let left: Vec<_> = std::fs::read_dir(dir.path().join("shots"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(left.is_empty());
}

#[std::prelude::v1::test]
fn finish_is_refused_while_a_check_finds_errors() {
    let dir = tempfile::tempdir().unwrap();
    let book = |second: &str| {
        json!({"sheets": [{"name": "S", "rows": [["Item", "Value"], ["A", 1], ["B", second]]}]})
            .to_string()
    };
    let (out, events, model) = run_job(
        dir.path(),
        vec![
            call(
                "finish",
                json!({"summary": "Done.", "outputs": ["book.xlsx"]}),
            ),
            call(
                "write_file",
                json!({"path": "book.json", "content": book("=1/0")}),
            ),
            call(
                "build",
                json!({"source": "book.json", "output": "book.xlsx", "look": "none"}),
            ),
            call(
                "finish",
                json!({"summary": "Done.", "outputs": ["book.xlsx"]}),
            ),
            call(
                "write_file",
                json!({"path": "book.json", "content": book("=2*3")}),
            ),
            call(
                "build",
                json!({"source": "book.json", "output": "book.xlsx", "look": "none"}),
            ),
            call(
                "finish",
                json!({"summary": "Done.", "outputs": ["book.xlsx"]}),
            ),
        ],
    );
    let refusals: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolResult {
                name,
                ok: false,
                text,
                ..
            } if name == "finish" => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 2, "{refusals:#?}");
    assert!(refusals[0].contains("book.xlsx doesn't exist"));
    assert!(
        refusals[1].contains("the check reports errors"),
        "{}",
        refusals[1]
    );
    assert!(refusals[1].contains("formula_error"), "{}", refusals[1]);
    assert!(
        matches!(out.ending, Ending::Finished { .. }),
        "{:?}",
        out.ending
    );
    assert_eq!(model.steps(), 7);
}

#[std::prelude::v1::test]
fn schema_keys_and_the_primer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("brief.md"), "a brief").unwrap();
    let t = DocTools::new(dir.path(), vec![App::Slides], true).unwrap();
    let primer = t.primer();
    assert!(primer.contains("build.sh"));
    assert!(primer.contains("# Keys, slides"));
    assert!(primer.contains("\nstat_cards: "), "{primer}");
    assert!(primer.contains("preset(editorial|swiss|mono)"), "{primer}");
    assert!(!primer.contains("# Keys, docs"));
    assert!(primer.contains("brief.md  7 B"));
    let s = keys::summary(App::Sheets, &App::Sheets.schema());
    assert!(s.contains("write_range_args: "), "{s}");
    let mut t = t;
    let c = Cancel::default();
    let one = ok(&t.call(
        "schema",
        &json!({"kind": "slides", "defs": ["stat_cards"]}),
        &c,
    ))
    .to_owned();
    assert!(one.starts_with("{\"stat_cards\":"), "{one}");
    assert!(
        t.call("schema", &json!({"kind": "slides", "defs": ["nope"]}), &c)
            .error
    );
    assert_eq!(App::for_skill("paparan-deck"), vec![App::Slides]);
    assert_eq!(App::for_skill("paparan-sheet"), vec![App::Sheets]);
    assert_eq!(App::for_skill("paparan-doc"), vec![App::Docs]);
    assert_eq!(App::for_skill("other"), App::ALL.to_vec());
}

#[std::prelude::v1::test]
fn made_outputs_are_the_jobs_documents_without_errors() {
    let dir = tempfile::tempdir().unwrap();
    let book = |second: &str| {
        json!({"sheets": [{"name": "S", "rows": [["Item", "Value"], ["A", 1], ["B", second]]}]})
            .to_string()
    };
    std::fs::write(dir.path().join("user.json"), book("2")).unwrap();
    let mut t = DocTools::new(dir.path(), vec![App::Sheets], true).unwrap();
    let c = Cancel::default();
    for (src, out, second) in [
        ("good.json", "good.xlsx", "2"),
        ("bad.json", "bad.xlsx", "=1/0"),
    ] {
        assert!(
            !t.call(
                "write_file",
                &json!({"path": src, "content": book(second)}),
                &c
            )
            .error
        );
        let built = t.call(
            "build",
            &json!({"source": src, "output": out, "look": "none"}),
            &c,
        );
        assert!(!built.error, "{}", built.text);
        if second == "2" {
            assert!(
                built
                    .text
                    .contains("No errors: the file can be handed over"),
                "{}",
                built.text
            );
        } else {
            assert!(
                built.text.contains("Fix the errors before finish"),
                "{}",
                built.text
            );
        }
    }
    // Only the job's document that passes its check.
    let made = t.made_outputs();
    assert_eq!(made.files, vec!["good.xlsx".to_owned()]);
    let checked = t.check_outputs(&["good.xlsx".to_owned()]).unwrap();
    assert_eq!(checked.files, vec!["good.xlsx".to_owned()]);
    assert!(
        checked
            .warnings
            .iter()
            .all(|w| w.starts_with("good.xlsx: warning"))
    );
}

#[std::prelude::v1::test]
fn a_failed_build_lists_every_schema_problem() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = tools(dir.path());
    let c = Cancel::default();
    let mut bad = deck();
    bad["size"] = json!([13.3, 7.5]);
    bad["slides"][0]["elements"][0]["colour"] = json!("#112233");
    bad["slides"][1]["elements"][0]["sise"] = json!(20);
    let w = t.call(
        "write_file",
        &json!({"path": "deck.json", "content": bad.to_string()}),
        &c,
    );
    ok(&w);
    let built = t.call(
        "build",
        &json!({"source": "deck.json", "output": "deck.pptx", "look": "none"}),
        &c,
    );
    assert!(built.error);
    assert!(built.text.contains("Every problem found"), "{}", built.text);
    for p in ["size:", "\"colour\"", "\"sise\""] {
        assert!(built.text.contains(p), "{p}: {}", built.text);
    }
    // A sheet's rows from a CSV in the folder, never from outside it.
    std::fs::write(dir.path().join("data.csv"), "Item,Value\nA,1\nB,2\n").unwrap();
    for (csv, fine) in [("data.csv", true), ("../data.csv", false)] {
        let book = json!({"sheets": [{"name": "Data", "csv": csv}]});
        ok(&t.call(
            "write_file",
            &json!({"path": "book.json", "content": book.to_string()}),
            &c,
        ));
        let built = t.call(
            "build",
            &json!({"source": "book.json", "output": "book.xlsx", "look": "none"}),
            &c,
        );
        assert_eq!(built.error, !fine, "{csv}: {}", built.text);
    }
}
