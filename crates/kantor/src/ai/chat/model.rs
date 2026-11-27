//! The chat's view model: the agent's events reduced to the rows the panel
//! shows (no GPUI, tested without a window).
//!
//! A job's events arrive in order: `step_start`, streamed text, `usage`,
//! the whole text, then per tool call `tool_call` (pictures from the worker
//! come next), `tool_result`; nudges between steps; `done`, `stopped` or
//! `error` at the end.

use std::path::Path;

use kantor_ai::Usage;
use kantor_ai::agent::{Event, FINISH, StopReason};
use serde_json::Value;

/// What the user asks for; picks the skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Deck,
    Document,
    Workbook,
}

impl JobKind {
    pub const ALL: [JobKind; 3] = [JobKind::Deck, JobKind::Document, JobKind::Workbook];

    /// The skill folder's name.
    pub fn skill(self) -> &'static str {
        match self {
            JobKind::Deck => "paparan-deck",
            JobKind::Document => "paparan-doc",
            JobKind::Workbook => "paparan-sheet",
        }
    }

    /// The kind a request is about, from its words (English or Indonesian)
    /// and then the attached files; a deck when nothing tells.
    pub fn guess(prompt: &str, attachments: &[&Path]) -> JobKind {
        const WORDS: [(JobKind, &[&str]); 3] = [
            (
                JobKind::Deck,
                &[
                    "paparan",
                    "presentasi",
                    "presentation",
                    "slide",
                    "salindia",
                    "deck",
                    "decks",
                    "pitch",
                    "pptx",
                    "powerpoint",
                    "keynote",
                ],
            ),
            (
                JobKind::Document,
                &[
                    "dokumen",
                    "document",
                    "laporan",
                    "report",
                    "surat",
                    "letter",
                    "memo",
                    "proposal",
                    "notulen",
                    "minutes",
                    "sop",
                    "kebijakan",
                    "policy",
                    "docx",
                    "word",
                    "brief",
                    "artikel",
                    "article",
                ],
            ),
            (
                JobKind::Workbook,
                &[
                    "excel",
                    "spreadsheet",
                    "workbook",
                    "xlsx",
                    "csv",
                    "sheet",
                    "tabel",
                    "rekap",
                    "dashboard",
                    "anggaran",
                    "budget",
                    "pivot",
                    "formula",
                    "rumus",
                ],
            ),
        ];
        let lower = prompt.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        // Short words must match whole ("word" isn't "password"); longer
        // ones also start a word ("slides", "laporannya").
        let hits = |words: &[&str]| -> usize {
            tokens
                .iter()
                .filter(|t| {
                    words
                        .iter()
                        .any(|w| **t == *w || (w.len() >= 5 && t.starts_with(w)))
                })
                .count()
        };
        let mut score = [0usize; 3];
        for (i, (_, words)) in WORDS.iter().enumerate() {
            score[i] = hits(words) * 2;
        }
        for path in attachments {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let i = match ext.as_str() {
                "pptx" | "ppt" | "odp" => 0,
                "docx" | "doc" | "odt" | "rtf" => 1,
                "xlsx" | "xls" | "xlsm" | "ods" | "csv" | "tsv" => 2,
                _ => continue,
            };
            score[i] += 1;
        }
        // Ties go to the first: deck, then document.
        let best = (0..3).fold(0, |b, i| if score[i] > score[b] { i } else { b });
        JobKind::ALL[best]
    }
}

/// Why the loop nudged the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nudge {
    /// It said what it would do and stopped.
    Announced,
    /// A turn with nothing in it.
    Empty,
    /// Text without `finish`.
    NoFinish,
}

/// A tool call as a row: the tool, its main arguments, the result.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    pub id: String,
    pub step: u32,
    pub name: String,
    /// The arguments in short (`deck.json → deck.pptx`).
    pub args: String,
    /// `None` while it runs.
    pub result: Option<ToolResult>,
    /// Pictures it handed the model: ids into the panel's pictures, with
    /// their labels.
    pub pictures: Vec<(usize, String)>,
    pub expanded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub text: String,
}

/// One row of the message list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// The user's request and the files attached to it.
    User {
        text: String,
        files: Vec<String>,
    },
    /// The model's text; `streaming` while it's still coming.
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool(ToolRow),
    Nudge(Nudge),
    /// The run's tokens (and cost when prices are known).
    Usage {
        total: Usage,
        steps: u32,
        cost: Option<f64>,
        /// Some steps' usage was estimated (the server sent none).
        estimated: bool,
    },
    /// `finish`: the summary and the files made (paths in the work folder).
    Finished {
        summary: String,
        outputs: Vec<String>,
    },
    Stopped {
        reason: StopReason,
        message: String,
    },
    Failed {
        message: String,
    },
}

/// The conversation as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chat {
    pub items: Vec<Item>,
    /// A run is going on.
    pub running: bool,
    /// The step the running job is on.
    pub step: u32,
    total: Usage,
    cost: Option<f64>,
    estimated: bool,
    next_picture: usize,
}

impl Chat {
    /// Whether a job has started in this chat (follow-ups continue it).
    pub fn started(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::User { .. }))
    }

    /// The user sent a request: a run starts.
    pub fn ask(&mut self, text: &str, files: Vec<String>) {
        self.items.push(Item::User {
            text: text.trim().to_owned(),
            files,
        });
        self.again();
    }

    /// A run starts again for the last request (Try again).
    pub fn again(&mut self) {
        self.running = true;
        self.step = 0;
        self.total = Usage::default();
        self.cost = None;
        self.estimated = false;
    }

    /// Fold one event in.
    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::StepStart { step } => {
                self.end_stream();
                self.step = *step;
            }
            Event::TextDelta { text } => match self.items.last_mut() {
                Some(Item::Assistant {
                    text: t,
                    streaming: true,
                }) => t.push_str(text),
                _ => self.items.push(Item::Assistant {
                    text: text.clone(),
                    streaming: true,
                }),
            },
            Event::Text { text, .. } => match self.items.last_mut() {
                Some(Item::Assistant {
                    text: t,
                    streaming: s @ true,
                }) => {
                    *t = text.clone();
                    *s = false;
                }
                _ => self.items.push(Item::Assistant {
                    text: text.clone(),
                    streaming: false,
                }),
            },
            // The model's thinking stays out of the list.
            Event::Reasoning { .. } | Event::ImagesDropped { .. } => {}
            Event::ToolCall {
                step,
                id,
                name,
                arguments,
            } => {
                self.end_stream();
                self.items.push(Item::Tool(ToolRow {
                    id: id.clone(),
                    step: *step,
                    name: name.clone(),
                    args: short_args(name, arguments),
                    result: None,
                    pictures: Vec::new(),
                    expanded: false,
                }));
            }
            Event::ToolResult {
                step,
                id,
                name,
                ok,
                text,
                ..
            } => {
                let at = self.items.iter().rposition(
                    |i| matches!(i, Item::Tool(r) if r.id == *id && r.step == *step && r.result.is_none()),
                );
                if let Some(at) = at {
                    // An accepted `finish` shows as the summary instead.
                    if name == FINISH && *ok {
                        self.items.remove(at);
                    } else if let Item::Tool(r) = &mut self.items[at] {
                        r.result = Some(ToolResult {
                            ok: *ok,
                            text: text.clone(),
                        });
                    }
                }
            }
            Event::Nudge { reason, .. } => {
                self.end_stream();
                let n = if reason.contains("announced") {
                    Nudge::Announced
                } else if reason.contains("empty") {
                    Nudge::Empty
                } else {
                    Nudge::NoFinish
                };
                self.items.push(Item::Nudge(n));
            }
            Event::Usage {
                step,
                total,
                cost,
                reported,
                ..
            } => {
                self.total = *total;
                self.cost = *cost;
                self.estimated |= !reported;
                self.step = *step;
            }
            Event::Done {
                summary, outputs, ..
            } => {
                self.end_stream();
                self.items.push(Item::Finished {
                    summary: summary.clone(),
                    outputs: outputs.clone(),
                });
            }
            Event::Stopped { reason, message } => {
                self.end_stream();
                self.items.push(Item::Stopped {
                    reason: *reason,
                    message: message.clone(),
                });
            }
            Event::Error { message } => {
                self.end_stream();
                self.items.push(Item::Failed {
                    message: message.clone(),
                });
            }
        }
    }

    /// Pictures the running tool handed the model: they show under its row.
    /// Returns their ids.
    pub fn add_pictures(&mut self, labels: Vec<String>) -> Vec<usize> {
        let ids: Vec<usize> = (0..labels.len()).map(|k| self.next_picture + k).collect();
        self.next_picture += labels.len();
        let row = self.items.iter_mut().rev().find_map(|i| match i {
            Item::Tool(r) if r.result.is_none() => Some(r),
            _ => None,
        });
        if let Some(r) = row {
            r.pictures.extend(ids.iter().copied().zip(labels));
        }
        ids
    }

    /// The run is over (whatever ended it): the usage line goes before the
    /// summary, which stays the last message.
    pub fn ended(&mut self) {
        self.end_stream();
        self.running = false;
        if self.step == 0 && self.total == Usage::default() {
            return;
        }
        let usage = Item::Usage {
            total: self.total,
            steps: self.step,
            cost: self.cost,
            estimated: self.estimated,
        };
        match self.items.last() {
            Some(Item::Finished { .. }) => {
                let at = self.items.len() - 1;
                self.items.insert(at, usage);
            }
            _ => self.items.push(usage),
        }
    }

    /// A failure the panel can say better (Paparan's credits, sign-in):
    /// replaces the last error's text, or adds one.
    pub fn set_failure(&mut self, message: String) {
        match self.items.last_mut() {
            Some(Item::Failed { message: m }) => *m = message,
            _ => self.items.push(Item::Failed { message }),
        }
    }

    /// Show or hide a tool row's result.
    pub fn toggle(&mut self, ix: usize) {
        if let Some(Item::Tool(r)) = self.items.get_mut(ix) {
            r.expanded = !r.expanded;
        }
    }

    fn end_stream(&mut self) {
        for i in self.items.iter_mut().rev() {
            if let Item::Assistant { streaming, .. } = i {
                *streaming = false;
                break;
            }
        }
    }
}

/// A tool call's arguments in a few words: the files it works on
/// (`deck.json → deck.pptx`), never a whole file's content.
pub fn short_args(name: &str, args: &Value) -> String {
    let s = |k: &str| -> Option<String> {
        match args.get(k)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            Value::Array(a) if !a.is_empty() => Some(
                a.iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            _ => None,
        }
    };
    let arrow = |from: Option<String>, to: Option<String>| match (from, to) {
        (Some(a), Some(b)) => format!("{a} → {b}"),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => String::new(),
    };
    let text = match name {
        "build" => arrow(s("source"), s("output")),
        "convert" => arrow(s("input"), s("output")),
        "edit" => {
            let ops = args
                .get("ops")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let files = arrow(s("file"), s("output"));
            if ops > 0 {
                format!("{files} ({ops} ops)")
            } else {
                files
            }
        }
        "look" => match (s("file"), s("pages")) {
            (Some(f), Some(p)) => format!("{f} · {p}"),
            (f, _) => f.unwrap_or_default(),
        },
        "check" | "list" | "macros" => s("file").unwrap_or_default(),
        "schema" => arrow(s("kind"), s("defs")),
        FINISH => String::new(),
        _ => match s("path").or_else(|| s("file")) {
            Some(p) => p,
            None => match args {
                Value::Object(m) if m.is_empty() => String::new(),
                Value::Null => String::new(),
                other => other.to_string(),
            },
        },
    };
    let text = text.replace('\n', " ");
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(79).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            cached: 0,
            output,
        }
    }

    /// The first step of a job: streamed text, then a build.
    fn job() -> Vec<Event> {
        vec![
            Event::StepStart { step: 1 },
            Event::TextDelta {
                text: "Membuat ".into(),
            },
            Event::TextDelta {
                text: "deck…".into(),
            },
            Event::Usage {
                step: 1,
                usage: usage(1000, 50),
                total: usage(1000, 50),
                cost: None,
                reported: true,
            },
            Event::Text {
                step: 1,
                text: "Membuat deck…".into(),
            },
            Event::ToolCall {
                step: 1,
                id: "c0".into(),
                name: "build".into(),
                arguments: json!({"source": "deck.json", "output": "deck.pptx"}),
            },
        ]
    }

    #[std::prelude::v1::test]
    fn events_become_rows() {
        let mut chat = Chat::default();
        chat.ask("  buat paparan penjualan ", vec!["data.xlsx".into()]);
        assert!(chat.running && chat.started());
        for e in job() {
            chat.apply(&e);
        }
        // Streamed pieces and the whole text make one message.
        assert_eq!(
            chat.items[1],
            Item::Assistant {
                text: "Membuat deck…".into(),
                streaming: false
            }
        );
        // The pictures arrive while the build runs.
        assert_eq!(
            chat.add_pictures(vec!["deck.pptx contact sheet".into()]),
            vec![0]
        );
        chat.apply(&Event::ToolResult {
            step: 1,
            id: "c0".into(),
            name: "build".into(),
            ok: true,
            text: "built deck.pptx".into(),
            images: vec!["deck.pptx contact sheet".into()],
        });
        let Item::Tool(row) = &chat.items[2] else {
            panic!("{:?}", chat.items[2]);
        };
        assert_eq!(row.args, "deck.json → deck.pptx");
        assert_eq!(row.pictures, vec![(0, "deck.pptx contact sheet".into())]);
        assert_eq!(
            row.result,
            Some(ToolResult {
                ok: true,
                text: "built deck.pptx".into()
            })
        );

        // A refused finish stays as a failed row; a nudge shows quietly.
        chat.apply(&Event::StepStart { step: 2 });
        chat.apply(&Event::ToolCall {
            step: 2,
            id: "c1".into(),
            name: FINISH.into(),
            arguments: json!({"summary": "x", "outputs": ["deck.pptx"]}),
        });
        chat.apply(&Event::ToolResult {
            step: 2,
            id: "c1".into(),
            name: FINISH.into(),
            ok: false,
            text: "finish refused".into(),
            images: vec![],
        });
        chat.apply(&Event::StepStart { step: 3 });
        chat.apply(&Event::Text {
            step: 3,
            text: "Let me fix it".into(),
        });
        chat.apply(&Event::Nudge {
            step: 3,
            count: 1,
            reason: "announced work without doing it".into(),
        });
        // The accepted finish: its row goes, the summary comes.
        chat.apply(&Event::StepStart { step: 4 });
        chat.apply(&Event::Usage {
            step: 4,
            usage: usage(2000, 80),
            total: usage(6000, 300),
            cost: Some(0.01),
            reported: false,
        });
        chat.apply(&Event::ToolCall {
            step: 4,
            id: "c0".into(),
            name: FINISH.into(),
            arguments: json!({"summary": "Selesai", "outputs": ["deck.pptx"]}),
        });
        chat.apply(&Event::ToolResult {
            step: 4,
            id: "c0".into(),
            name: FINISH.into(),
            ok: true,
            text: "finished".into(),
            images: vec![],
        });
        chat.apply(&Event::Done {
            summary: "Selesai".into(),
            outputs: vec!["deck.pptx".into()],
            limit: None,
        });
        chat.ended();
        assert!(!chat.running);

        let kinds: Vec<&str> = chat
            .items
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Assistant { .. } => "text",
                Item::Tool(_) => "tool",
                Item::Nudge(_) => "nudge",
                Item::Usage { .. } => "usage",
                Item::Finished { .. } => "finished",
                Item::Stopped { .. } => "stopped",
                Item::Failed { .. } => "failed",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user", "text", "tool", "tool", "text", "nudge", "usage", "finished"
            ]
        );
        assert!(
            matches!(&chat.items[3], Item::Tool(r) if r.result.as_ref().is_some_and(|x| !x.ok))
        );
        assert_eq!(chat.items[5], Item::Nudge(Nudge::Announced));
        assert_eq!(
            chat.items[6],
            Item::Usage {
                total: usage(6000, 300),
                steps: 4,
                cost: Some(0.01),
                estimated: true
            }
        );
        assert_eq!(
            chat.items[7],
            Item::Finished {
                summary: "Selesai".into(),
                outputs: vec!["deck.pptx".into()]
            }
        );

        // A follow-up starts a new run in the same chat.
        chat.ask("lebih singkat", vec![]);
        assert!(chat.running);
        chat.apply(&Event::StepStart { step: 1 });
        chat.apply(&Event::Stopped {
            reason: StopReason::Cancelled,
            message: "cancelled".into(),
        });
        chat.ended();
        assert!(matches!(
            chat.items.last(),
            Some(Item::Usage { steps: 1, .. })
        ));
        assert!(matches!(
            chat.items[chat.items.len() - 2],
            Item::Stopped {
                reason: StopReason::Cancelled,
                ..
            }
        ));
    }

    #[std::prelude::v1::test]
    fn streaming_text_is_closed_by_the_next_step_and_errors_can_be_reworded() {
        let mut chat = Chat::default();
        chat.ask("x", vec![]);
        chat.apply(&Event::StepStart { step: 1 });
        chat.apply(&Event::TextDelta { text: "a".into() });
        assert!(matches!(
            chat.items.last(),
            Some(Item::Assistant {
                streaming: true,
                ..
            })
        ));
        chat.apply(&Event::Error {
            message: "HTTP 402".into(),
        });
        assert!(matches!(
            chat.items[1],
            Item::Assistant {
                streaming: false,
                ..
            }
        ));
        chat.set_failure("Not enough credits".into());
        assert_eq!(
            chat.items.last(),
            Some(&Item::Failed {
                message: "Not enough credits".into()
            })
        );
        // A run that never reached the model has no usage line.
        let mut chat = Chat::default();
        chat.ask("x", vec![]);
        chat.apply(&Event::Error {
            message: "no skill".into(),
        });
        chat.ended();
        assert!(matches!(chat.items.last(), Some(Item::Failed { .. })));
        chat.again();
        assert!(chat.running && chat.step == 0);
        assert_eq!(
            chat.items
                .iter()
                .filter(|i| matches!(i, Item::User { .. }))
                .count(),
            1,
            "trying again doesn't repeat the request"
        );
        // Rows open and close.
        let mut chat = Chat::default();
        chat.apply(&Event::ToolCall {
            step: 1,
            id: "c".into(),
            name: "check".into(),
            arguments: json!({"file": "a.docx"}),
        });
        chat.toggle(0);
        assert!(matches!(&chat.items[0], Item::Tool(r) if r.expanded && r.args == "a.docx"));
    }

    #[std::prelude::v1::test]
    fn short_arguments() {
        assert_eq!(
            short_args(
                "write_file",
                &json!({"path": "deck.json", "content": "{…long…}"})
            ),
            "deck.json"
        );
        assert_eq!(
            short_args(
                "edit",
                &json!({"file": "a.docx", "output": "b.docx", "ops": [{}, {}]})
            ),
            "a.docx → b.docx (2 ops)"
        );
        assert_eq!(
            short_args("look", &json!({"file": "deck.pptx", "pages": "2-3"})),
            "deck.pptx · 2-3"
        );
        assert_eq!(
            short_args(
                "schema",
                &json!({"kind": "slides", "defs": ["chart", "table"]})
            ),
            "slides → chart, table"
        );
        assert_eq!(short_args("list_files", &json!({})), "");
        assert_eq!(short_args(FINISH, &json!({"summary": "x"})), "");
        let long = short_args("other", &json!({"q": "x".repeat(200)}));
        assert_eq!(long.chars().count(), 80);
        assert!(long.ends_with('…'));
    }

    #[std::prelude::v1::test]
    fn job_kind_from_the_words_then_the_files() {
        let none: [&Path; 0] = [];
        let g = |p: &str| JobKind::guess(p, &none);
        assert_eq!(g("Buat paparan 8 slide tentang kinerja Q3"), JobKind::Deck);
        assert_eq!(g("Make a pitch deck for investors"), JobKind::Deck);
        assert_eq!(g("Tolong buatkan surat undangan rapat"), JobKind::Document);
        assert_eq!(g("Write a two-page report on the audit"), JobKind::Document);
        assert_eq!(
            g("Rekap anggaran per divisi dengan rumus"),
            JobKind::Workbook
        );
        assert_eq!(g("Clean up this spreadsheet"), JobKind::Workbook);
        // "word" is a whole word, not part of "password".
        assert_eq!(g("Reset the password list"), JobKind::Deck);
        // Nothing in the words: the attachments tell.
        let xlsx = [Path::new("/x/penjualan.xlsx")];
        assert_eq!(JobKind::guess("tolong rapikan", &xlsx), JobKind::Workbook);
        // The words weigh more than a data file attached as the source.
        assert_eq!(
            JobKind::guess("buat paparan dari data ini", &xlsx),
            JobKind::Deck
        );
        assert_eq!(JobKind::Workbook.skill(), "paparan-sheet");
    }
}
