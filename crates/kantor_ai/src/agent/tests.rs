use std::path::PathBuf;

use serde_json::{Value, json};

use super::scripted::{Scripted, Turn, call, calls, text};
use super::*;

/// Tools over a temporary folder: `write` makes a file, `look` hands back a
/// picture, `big` a long text.
struct Fake {
    dir: PathBuf,
}

impl Tools for Fake {
    fn specs(&self) -> Vec<ToolSpec> {
        ["write", "look", "big"]
            .iter()
            .map(|n| ToolSpec::new(*n, "test tool", json!({"type": "object"})))
            .collect()
    }

    fn primer(&self) -> String {
        "PRIMER: keys a, b".into()
    }

    fn call(&mut self, name: &str, args: &Value, _cancel: &Cancel) -> ToolOutput {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
        match name {
            "write" => {
                std::fs::write(self.dir.join(s("path")), s("content")).unwrap();
                ToolOutput::ok("written")
            }
            "look" => ToolOutput::ok("here").with_images(vec![Image {
                file: s("file").into(),
                page: Some(s("page").into()),
                version: s("version").into(),
                label: format!("{} page {}", s("file"), s("page")),
                mime: "image/png",
                bytes: vec![1, 2, 3],
            }]),
            "big" => ToolOutput::ok("x".repeat(20_000)).more("use big with a range"),
            _ => ToolOutput::error("no such tool"),
        }
    }

    fn check_outputs(&mut self, outputs: &[String]) -> Result<Checked, String> {
        let missing: Vec<&String> = outputs
            .iter()
            .filter(|o| !self.dir.join(o).is_file())
            .collect();
        if missing.is_empty() {
            Ok(Checked {
                files: outputs.to_vec(),
                // Files named warn* have a warning left.
                warnings: outputs
                    .iter()
                    .filter(|o| o.starts_with("warn"))
                    .map(|o| format!("{o}: warning text_cut_off A1"))
                    .collect(),
            })
        } else {
            Err(format!("{missing:?} doesn't exist"))
        }
    }

    /// The files written so far (all pass).
    fn made_outputs(&mut self) -> Checked {
        let mut files: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        Checked::files(files)
    }
}

struct Run {
    outcome: Outcome,
    events: Vec<Event>,
}

fn run_script(turns: Vec<Turn>, limits: Limits) -> (Run, Scripted, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let model = Scripted::new(turns);
    let mut tools = Fake {
        dir: dir.path().to_path_buf(),
    };
    let mut events = Vec::new();
    let outcome = run(
        &model,
        &mut tools,
        None,
        "make a deck",
        &limits,
        &Cancel::default(),
        &mut |e| events.push(e.clone()),
    );
    (Run { outcome, events }, model, dir)
}

fn finish(outputs: &[&str]) -> Turn {
    call(
        "finish",
        json!({"summary": "Done: one deck.", "outputs": outputs}),
    )
}

fn images_in(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|p| p["type"] == "image_url")
        .count()
}

#[test]
fn text_only_turns_are_nudged_until_finish() {
    let (r, model, _dir) = run_script(
        vec![
            text("Let me restructure the deck so the story flows better."),
            text("All done, the deck is great."),
            finish(&[]),
        ],
        Limits::default(),
    );
    assert_eq!(
        r.outcome.ending,
        Ending::Finished {
            summary: "Done: one deck.".into(),
            outputs: vec![]
        }
    );
    assert_eq!(r.outcome.steps, 3);
    let nudges: Vec<&Event> = r
        .events
        .iter()
        .filter(|e| matches!(e, Event::Nudge { .. }))
        .collect();
    assert_eq!(nudges.len(), 2);
    assert!(matches!(nudges[0], Event::Nudge { reason, .. } if reason.contains("announced")));
    assert!(matches!(nudges[1], Event::Nudge { reason, .. } if reason == "ended without finish"));
    // The second step saw the nudge after the model's text.
    let sent = model.sent(2);
    assert_eq!(sent[1]["role"], "assistant");
    assert_eq!(sent[2]["role"], "user");
    assert!(sent[2]["content"].as_str().unwrap().contains("finish"));
    assert!(matches!(r.events.last(), Some(Event::Done { .. })));
}

#[test]
fn too_many_text_turns_stop_the_job() {
    let limits = Limits {
        max_nudges: 2,
        ..Limits::default()
    };
    let (r, _, _dir) = run_script(vec![text("a"), text("b"), text("c"), finish(&[])], limits);
    assert!(matches!(
        r.outcome.ending,
        Ending::Stopped {
            reason: StopReason::NoFinish,
            ..
        }
    ));
    assert_eq!(r.outcome.steps, 3);
}

#[test]
fn finish_is_refused_while_an_output_is_missing() {
    let (r, model, dir) = run_script(
        vec![
            finish(&["deck.pptx"]),
            call("write", json!({"path": "deck.pptx", "content": "x"})),
            finish(&["deck.pptx"]),
        ],
        Limits::default(),
    );
    assert!(dir.path().join("deck.pptx").is_file());
    assert!(
        matches!(r.outcome.ending, Ending::Finished { ref outputs, .. } if outputs == &["deck.pptx"])
    );
    let refused = r.events.iter().find_map(|e| match e {
        Event::ToolResult {
            name,
            ok: false,
            text,
            ..
        } if name == "finish" => Some(text.clone()),
        _ => None,
    });
    assert!(refused.unwrap().contains("finish refused"));
    // The model was told on the next step.
    let sent = model.sent(2);
    let tool = sent.iter().find(|m| m["role"] == "tool").unwrap();
    assert!(tool["content"].as_str().unwrap().contains("doesn't exist"));
}

#[test]
fn finish_runs_after_the_other_calls_of_its_turn() {
    let (r, _, _dir) = run_script(
        vec![calls(&[
            ("finish", json!({"summary": "ok", "outputs": ["a.txt"]})),
            ("write", json!({"path": "a.txt", "content": "hi"})),
        ])],
        Limits::default(),
    );
    assert!(matches!(r.outcome.ending, Ending::Finished { .. }));
    // Tool messages still follow the calls' order.
    let tools: Vec<&Value> = r
        .outcome
        .messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .collect();
    assert_eq!(tools[0]["tool_call_id"], "c0");
    assert_eq!(tools[1]["tool_call_id"], "c1");
}

#[test]
fn newer_pictures_replace_older_ones() {
    let look = |page: &str, version: &str| {
        call(
            "look",
            json!({"file": "deck.pptx", "page": page, "version": version}),
        )
    };
    let limits = Limits {
        max_images: 2,
        ..Limits::default()
    };
    let (r, model, _dir) = run_script(
        vec![
            look("1", "v1"),
            look("1", "v1"),
            look("2", "v1"),
            look("3", "v1"),
            look("3", "v2"),
            finish(&[]),
        ],
        limits,
    );
    assert!(matches!(r.outcome.ending, Ending::Finished { .. }));
    // Step 3 saw only the newer picture of page 1.
    let sent = model.sent(3);
    assert_eq!(images_in(&sent), 1);
    let all = serde_json::to_string(&sent).unwrap();
    assert!(all.contains("a newer picture of it follows"), "{all}");
    // Step 4: pages 1 and 2. Step 5: the cap of 2 dropped page 1.
    assert_eq!(images_in(&model.sent(4)), 2);
    let s5 = model.sent(5);
    assert_eq!(images_in(&s5), 2);
    assert!(
        serde_json::to_string(&s5)
            .unwrap()
            .contains("removed to save space")
    );
    // A new version of the file drops every older picture of it.
    let s6 = model.sent(6);
    assert_eq!(images_in(&s6), 1);
    assert!(
        serde_json::to_string(&s6)
            .unwrap()
            .contains("the file changed since")
    );
    assert!(
        r.events
            .iter()
            .any(|e| matches!(e, Event::ImagesDropped { .. }))
    );
    // Pictures go in a user message after the tool results, as data URLs.
    let last = s6.last().unwrap();
    assert_eq!(last["role"], "user");
    let url = last["content"][2]["image_url"]["url"].as_str().unwrap();
    assert_eq!(url, "data:image/png;base64,AQID");
}

#[test]
fn large_results_are_cut_with_a_paging_note() {
    let limits = Limits {
        max_result_chars: 1_000,
        ..Limits::default()
    };
    let (_, model, _dir) = run_script(vec![call("big", json!({})), finish(&[])], limits);
    let sent = model.sent(2);
    let tool = sent.iter().find(|m| m["role"] == "tool").unwrap();
    let content = tool["content"].as_str().unwrap();
    assert!(content.len() < 1_200, "{}", content.len());
    assert!(content.contains("[truncated: showed 1000 of 20000 characters. use big with a range]"));
}

#[test]
fn old_results_and_long_arguments_are_shortened() {
    let long = "y".repeat(5_000);
    let limits = Limits {
        max_result_chars: 50_000,
        old_result_steps: 2,
        old_result_chars: 1_000,
        ..Limits::default()
    };
    let (_, model, _dir) = run_script(
        vec![
            call("write", json!({"path": "deck.json", "content": long})),
            call("big", json!({})),
            call("look", json!({"file": "a", "page": "1", "version": "1"})),
            call("look", json!({"file": "a", "page": "2", "version": "1"})),
            finish(&[]),
        ],
        limits,
    );
    // Step 2 no longer carries the 5000 characters written in step 1.
    let s2 = serde_json::to_string(&model.sent(2)).unwrap();
    assert!(!s2.contains(&long));
    assert!(s2.contains("[left out: 5000 characters"));
    // Step 3 still has the whole result of step 2; step 4 a cut one.
    let tool = |n: usize| {
        model
            .sent(n)
            .iter()
            .filter(|m| m["role"] == "tool")
            .nth(1)
            .map(|m| m["content"].as_str().unwrap().to_owned())
            .unwrap()
    };
    assert!(tool(3).len() >= 20_000);
    let cut = tool(4);
    assert!(cut.len() < 1_000, "{}", cut.len());
    assert!(cut.contains("older result cut"));
}

#[test]
fn budgets_stop_before_the_step_that_would_go_over() {
    // Every scripted call step uses 1000 tokens in (200 cached), 100 out.
    let limits = Limits {
        max_tokens: Some(4_000),
        ..Limits::default()
    };
    let many: Vec<Turn> = (0..10)
        .map(|_| call("write", json!({"path": "a.txt", "content": "x"})))
        .collect();
    let (r, model, _dir) = run_script(many, limits);
    // Step 3 starts with 2200 used and about 1000 more in: 800 left for
    // its output, too little for a step. The job hands over what it made.
    assert_eq!(r.outcome.steps, 2);
    assert_eq!(r.outcome.limit, Some(StopReason::Budget));
    assert!(matches!(
        &r.outcome.ending,
        Ending::Finished { outputs, .. } if outputs == &["a.txt"]
    ));
    // Step 2 could afford 4000 - 1100 - ~1000 in: its output was capped.
    let cap = model.opts_on(2).max_output.unwrap();
    assert!((1_500..3_000).contains(&cap), "{cap}");
    // Step 1: the whole budget less the first input (system prompt, tools
    // and request, estimated).
    let first = model.opts_on(1).max_output.unwrap();
    assert!((2_000..3_800).contains(&first), "{first}");
    // The budget note came before step 2 (half the budget is gone after
    // step 1, but less than three typical steps are left).
    let s2 = serde_json::to_string(&model.sent(2)).unwrap();
    assert!(s2.contains("budget for this job is nearly spent"), "{s2}");

    // Nothing made: stopped.
    let prices = Prices {
        input: 0.30,
        cached: 0.006,
        output: 1.20,
    };
    let limits = Limits {
        max_cost: Some(0.003),
        prices: Some(prices),
        ..Limits::default()
    };
    let many: Vec<Turn> = (0..10).map(|_| call("big", json!({}))).collect();
    let (r, model, _dir) = run_script(many, limits);
    assert!(matches!(
        r.outcome.ending,
        Ending::Stopped {
            reason: StopReason::Budget,
            ..
        }
    ));
    // Every step's input grows by the 12,000-character result (about
    // 3000 tokens): the budget allows two steps, never a third that would
    // go over.
    let cost = r.outcome.cost.unwrap();
    assert!(cost <= 0.003, "{cost}");
    assert!(r.outcome.steps >= 1 && model.steps() == r.outcome.steps as usize);
}

#[test]
fn reasoning_effort_goes_with_every_step() {
    let limits = Limits {
        reasoning: Some("low".into()),
        ..Limits::default()
    };
    let (_, model, _dir) = run_script(vec![call("big", json!({})), finish(&[])], limits);
    assert_eq!(model.opts_on(1).reasoning.as_deref(), Some("low"));
    assert_eq!(model.opts_on(2).reasoning.as_deref(), Some("low"));
    assert_eq!(model.opts_on(2).max_output, None, "no budget: no cap");
}

#[test]
fn the_step_limit_hands_over_what_was_made() {
    let limits = Limits {
        max_steps: 4,
        ..Limits::default()
    };
    let mut turns = vec![call("write", json!({"path": "book.xlsx", "content": "v1"}))];
    turns.extend((0..5).map(|_| call("big", json!({}))));
    let (r, model, _dir) = run_script(turns, limits);
    // The last step offered only `finish`; the model called `big`
    // anyway, which didn't run.
    assert_eq!(model.tools_on(4), vec!["finish"]);
    assert!(model.tools_on(3).len() > 1);
    assert_eq!(r.outcome.steps, 4);
    assert_eq!(r.outcome.limit, Some(StopReason::Steps));
    match &r.outcome.ending {
        Ending::Finished { summary, outputs } => {
            assert_eq!(outputs, &["book.xlsx"]);
            assert!(summary.contains("step limit"), "{summary}");
            assert!(summary.contains("book.xlsx"), "{summary}");
        }
        e => panic!("{e:?}"),
    }
    assert!(matches!(
        r.events.last(),
        Some(Event::Done {
            limit: Some(StopReason::Steps),
            ..
        })
    ));
}

#[test]
fn finish_with_warnings_reports_them() {
    let (r, _, _dir) = run_script(
        vec![
            call("write", json!({"path": "warn.xlsx", "content": "x"})),
            finish(&["warn.xlsx"]),
        ],
        Limits::default(),
    );
    match &r.outcome.ending {
        Ending::Finished { summary, .. } => {
            assert!(summary.starts_with("Done: one deck."), "{summary}");
            assert!(summary.contains("Check warnings left (1)"), "{summary}");
            assert!(summary.contains("warn.xlsx: warning text_cut_off A1"));
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(r.outcome.limit, None);
}

#[test]
fn indonesian_requests_get_indonesian_narration() {
    assert_eq!(
        request_language("Buatkan rekap penjualan per bulan untuk tiap kota, dengan grafik garis"),
        Some("Indonesian")
    );
    assert_eq!(
        request_language("Tolong buat surat undangan rapat"),
        Some("Indonesian")
    );
    assert_eq!(
        request_language(
            "Make a 6-slide board deck on Q3 sales performance by branch (target vs actual) from penjualan-q3.csv"
        ),
        Some("English")
    );
    assert_eq!(request_language("Q3 2026"), None);
    assert_eq!(request_language(""), None);

    let dir = tempfile::tempdir().unwrap();
    let model = Scripted::new(vec![finish(&[])]);
    let mut tools = Fake {
        dir: dir.path().to_path_buf(),
    };
    run(
        &model,
        &mut tools,
        None,
        "Buatkan paparan 5 slide tentang hasil penjualan cabang",
        &Limits::default(),
        &Cancel::default(),
        &mut |_| {},
    );
    let system = model.system.lock().unwrap().clone();
    assert!(
        system.ends_with(
            "The documents follow the request (its language, unless it asks for another)."
        )
    );
    assert!(system.contains("The user wrote in Indonesian."));
}

#[test]
fn step_limit_and_cancel() {
    let limits = Limits {
        max_steps: 3,
        ..Limits::default()
    };
    let many: Vec<Turn> = (0..10).map(|_| call("big", json!({}))).collect();
    let (r, _, _dir) = run_script(many, limits);
    assert!(matches!(
        r.outcome.ending,
        Ending::Stopped {
            reason: StopReason::Steps,
            ..
        }
    ));
    assert_eq!(r.outcome.steps, 3);

    let dir = tempfile::tempdir().unwrap();
    let model = Scripted::new(vec![finish(&[])]);
    let mut tools = Fake {
        dir: dir.path().to_path_buf(),
    };
    let cancel = Cancel::default();
    cancel.cancel();
    let out = run(
        &model,
        &mut tools,
        None,
        "x",
        &Limits::default(),
        &cancel,
        &mut |_| {},
    );
    assert!(matches!(
        out.ending,
        Ending::Stopped {
            reason: StopReason::Cancelled,
            ..
        }
    ));
    assert_eq!(model.steps(), 0);
}

#[test]
fn errors_end_the_job_and_bad_arguments_are_reported() {
    let (r, model, _dir) = run_script(
        vec![
            Turn::Reply(crate::client::Reply {
                tool_calls: vec![crate::client::ToolCall {
                    id: "x".into(),
                    name: "write".into(),
                    arguments: "{not json".into(),
                }],
                ..Default::default()
            }),
            call("nope", json!({})),
            Turn::Fail(AiError::Http {
                status: 401,
                message: "bad key".into(),
            }),
        ],
        Limits::default(),
    );
    let s2 = serde_json::to_string(&model.sent(2)).unwrap();
    assert!(s2.contains("not valid JSON"));
    let s3 = serde_json::to_string(&model.sent(3)).unwrap();
    assert!(s3.contains("there is no tool nope"));
    assert!(matches!(
        r.outcome.ending,
        Ending::Failed(AiError::Http { status: 401, .. })
    ));
    assert!(!r.outcome.usage_reported, "step 1 had no usage: estimated");
    assert!(matches!(r.events.last(), Some(Event::Error { .. })));
}

#[test]
fn local_skills_give_the_prompt_and_files_on_demand() {
    let root = tempfile::tempdir().unwrap();
    let deck = root.path().join("paparan-deck");
    std::fs::create_dir_all(deck.join("references")).unwrap();
    std::fs::write(
        deck.join("SKILL.md"),
        "---\nname: paparan-deck\ndescription: decks\n---\n\n# Paparan deck\nBody text.\n",
    )
    .unwrap();
    std::fs::write(deck.join("references/charts.md"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(deck.join("references/pic.png"), [9u8, 9, 9]).unwrap();
    std::fs::write(root.path().join("secret.txt"), "no").unwrap();
    let sheet = root.path().join("paparan-sheet");
    std::fs::create_dir_all(&sheet).unwrap();
    std::fs::write(sheet.join("SKILL.md"), "# Sheet\n").unwrap();

    let skill = LocalSkill::open(&sheet).unwrap();
    assert_eq!(skill.name(), "paparan-sheet");
    // A sibling skill's file, as the skill text names it.
    assert_eq!(
        skill
            .read("~/.claude/skills/paparan-deck/references/charts.md")
            .unwrap(),
        SkillFile::Text("one\ntwo\nthree\n".into())
    );
    assert!(skill.read("../secret.txt").is_err());
    assert!(skill.read("/etc/passwd").is_err());

    let skill = LocalSkill::open(&deck).unwrap();
    assert_eq!(skill.name(), "paparan-deck");
    assert!(skill.read("references/charts.md").is_ok());
    assert!(skill.read("paparan-deck/references/charts.md").is_ok());
    assert!(matches!(
        skill.read("references/pic.png"),
        Ok(SkillFile::Image {
            mime: "image/png",
            ..
        })
    ));
    assert!(
        skill
            .files()
            .contains(&"references/charts.md (14 B)".to_owned())
    );

    let tools = Fake {
        dir: root.path().to_path_buf(),
    };
    let system = system_prompt(&tools, Some(&skill)).unwrap();
    assert!(system.contains("PRIMER: keys a, b"));
    assert!(system.contains("# Paparan deck\nBody text."));
    assert!(!system.contains("description: decks"), "no front matter");
    assert!(system.contains("references/charts.md (14 B)"));

    // read_skill_file through the loop, paged.
    let model = Scripted::new(vec![
        call(
            "read_skill_file",
            json!({"path": "references/charts.md", "offset": 2, "limit": 1}),
        ),
        call("read_skill_file", json!({"path": "references/pic.png"})),
        finish(&[]),
    ]);
    let mut tools = tools;
    let out = run(
        &model,
        &mut tools,
        Some(&skill),
        "x",
        &Limits::default(),
        &Cancel::default(),
        &mut |_| {},
    );
    assert!(matches!(out.ending, Ending::Finished { .. }));
    let s2 = serde_json::to_string(&model.sent(2)).unwrap();
    assert!(
        s2.contains("two\\n[lines 2-2 of 3; offset 3 reads on]"),
        "{s2}"
    );
    assert_eq!(images_in(&model.sent(3)), 1);
}

#[test]
fn announcing_work_is_recognised() {
    assert!(announces_more_work("Let me restructure the slides."));
    assert!(announces_more_work("Now I'll fix the chart"));
    assert!(announces_more_work("Saya akan memperbaiki slide 3"));
    assert!(announces_more_work("Next steps:"));
    assert!(!announces_more_work("The deck is finished and checked."));
    assert!(!announces_more_work(""));
}

#[test]
fn paging_and_truncating_text() {
    let t = "a\nb\nc\nd";
    assert_eq!(page_lines(t, None, None), t);
    assert_eq!(page_lines(t, Some(3), None), "c\nd");
    assert_eq!(
        page_lines(t, Some(1), Some(2)),
        "a\nb\n[lines 1-2 of 4; offset 3 reads on]"
    );
    assert!(page_lines(t, Some(9), None).contains("no line 9"));
    assert_eq!(truncate_result("short", 10, None), "short");
    let long = "é".repeat(30);
    let cut = truncate_result(&long, 10, None);
    assert!(cut.starts_with(&"é".repeat(10)));
    assert!(cut.contains("showed 10 of 30 characters"));
}

#[test]
fn steps_left_and_unchanged_pictures_are_pointed_out() {
    let look = || {
        call(
            "look",
            json!({"file": "d.pptx", "page": "1", "version": "v1"}),
        )
    };
    let limits = Limits {
        max_steps: 10,
        ..Limits::default()
    };
    let (_, model, _dir) = run_script(vec![look(), look(), look(), look()], limits);
    let s2 = serde_json::to_string(&model.sent(2)).unwrap();
    assert!(!s2.contains("Unchanged since"));
    assert!(!s2.contains("steps left"));
    let s3 = serde_json::to_string(&model.sent(3)).unwrap();
    assert!(
        s3.contains("Unchanged since you last saw them (no need to look again): d.pptx page 1")
    );
    // Steps 7 to 9 leave 3 to 1: the model is told.
    let mut many = vec![look()];
    many.extend((0..9).map(|_| call("big", json!({}))));
    let limits = Limits {
        max_steps: 10,
        ..Limits::default()
    };
    let (r, model, _dir) = run_script(many, limits);
    assert!(matches!(
        r.outcome.ending,
        Ending::Stopped {
            reason: StopReason::Steps,
            ..
        }
    ));
    let last = |n: usize| {
        model.sent(n).last().unwrap()["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(!last(7).contains("steps left"));
    assert!(last(8).contains("3 steps left of 10"), "{}", last(8));
    assert!(last(10).contains("One step left"), "{}", last(10));
}

#[test]
fn a_follow_up_goes_on_from_the_earlier_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = Fake {
        dir: dir.path().to_path_buf(),
    };
    let first = Scripted::new(vec![
        call(
            "look",
            json!({"file": "deck.pptx", "page": "1", "version": "a"}),
        ),
        call("write", json!({"path": "deck.pptx", "content": "v1"})),
        finish(&["deck.pptx"]),
    ]);
    let outcome = run(
        &first,
        &mut tools,
        None,
        "make a deck",
        &Limits::default(),
        &Cancel::default(),
        &mut |_| {},
    );
    assert!(matches!(outcome.ending, Ending::Finished { .. }));
    assert_eq!(images_in(&outcome.messages), 1);

    // A job stopped mid-step left a call without its result.
    let mut history = outcome.messages;
    history.push(json!({"role": "assistant", "content": null, "tool_calls": [
        {"id": "late", "type": "function", "function": {"name": "write", "arguments": "{}"}}
    ]}));

    let second = Scripted::new(vec![finish(&["deck.pptx"])]);
    let mut events = Vec::new();
    let outcome = resume(
        &second,
        &mut tools,
        None,
        history,
        "now shorter",
        &Limits::default(),
        &Cancel::default(),
        &mut |e| events.push(e.clone()),
    );
    assert!(matches!(outcome.ending, Ending::Finished { .. }));
    assert_eq!(outcome.steps, 1, "the step count starts again");
    let sent = second.sent(1);
    assert_eq!(sent[0]["content"], "make a deck");
    assert_eq!(images_in(&sent), 0, "old pictures become notes");
    let n = sent.len();
    assert_eq!(sent[n - 1]["content"], "now shorter");
    assert_eq!(sent[n - 2]["role"], "tool");
    assert_eq!(sent[n - 2]["tool_call_id"], "late");
    assert!(matches!(events.first(), Some(Event::StepStart { step: 1 })));
}
