//! The agent loop ("harness"): a model and tools take turns until the
//! model calls `finish`, a budget runs out or the user cancels.
//!
//! No GPUI and no files here: the caller brings the [`Model`] (a [`Client`]
//! or a scripted one in tests), the [`Tools`] (Kantor's document tools,
//! confined to one folder) and the instructions ([`PromptSource`]), and
//! gets [`Event`]s to show while it runs. A chat panel renders those; the
//! `kantor agent run` command prints them.
//!
//! Rules learnt from running the Paparan skills with cheap models:
//! - a turn that ends with text and no tool call is not the end: the model
//!   is nudged to go on (it often says "Let me restructure…" and stops);
//!   only `finish` ends the job, and `finish` is refused while a declared
//!   output is missing or fails its check;
//! - every step resends the conversation, so it's kept small: one skill
//!   file up front, the rest on demand; old pictures dropped; big results
//!   and old file contents cut (see [`context`]).

pub mod context;
pub mod scripted;
pub mod skill;
#[cfg(test)]
mod tests;

use serde::Serialize;
use serde_json::{Value, json};

use crate::AiError;
use crate::client::{Cancel, ChatRequest, Client, Reply, Usage};
use crate::provider::Api;
pub use context::{Context, truncate_result};
pub use skill::{LocalSkill, PromptSource, ServedSkill, SkillFile};

/// The tool that ends the job.
pub const FINISH: &str = "finish";
/// The tool that reads the skill's other files.
pub const READ_SKILL_FILE: &str = "read_skill_file";

/// How one step may answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepOptions {
    /// The most tokens the reply may have, reasoning included (a budget
    /// that is nearly spent). `None`: the server's own limit.
    pub max_output: Option<u32>,
    /// `reasoning_effort` for models that think before they answer.
    pub reasoning: Option<String>,
}

/// What the loop asks a model: the next turn of the conversation.
pub trait Model {
    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError>;

    /// [`Model::complete`] with the step's options; models that can't
    /// take them answer as usual.
    fn complete_with(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        _opts: &StepOptions,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        self.complete(system, messages, tools, cancel, on_text)
    }
}

impl Model for Client {
    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        self.complete_with(
            system,
            messages,
            tools,
            &StepOptions::default(),
            cancel,
            on_text,
        )
    }

    fn complete_with(
        &self,
        system: &str,
        messages: &[Value],
        tools: &Value,
        opts: &StepOptions,
        cancel: &Cancel,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<Reply, AiError> {
        if self.config().api() != Api::OpenAi {
            return Err(AiError::BadReply(format!(
                "{} can't run the agent yet: it needs an OpenAI-compatible API with tool calls",
                self.config().provider.info().label
            )));
        }
        let mut req = ChatRequest::new(system, "")
            .task("agent")
            .tools(tools.clone())
            .messages(messages.to_vec());
        req.max_output = opts.max_output;
        req.reasoning_effort = opts.reasoning.clone();
        self.complete_reply(&req, cancel, on_text)
    }
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// The OpenAI `tools` entry.
    pub fn json(&self) -> Value {
        json!({"type": "function", "function": {
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        }})
    }
}

/// A picture a tool hands the model (a rendered slide, page or contact
/// sheet).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// The file it shows (`deck.pptx`).
    pub file: String,
    /// Which part of it (`3`, `sheet`); `None` for the whole file.
    pub page: Option<String>,
    /// Changes when the file changes (size and modified time): a picture
    /// of a newer version drops every older picture of the file.
    pub version: String,
    /// What it shows, for notes and events (`deck.pptx slide 3`).
    pub label: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// What a tool call gave back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<Image>,
    pub error: bool,
    /// How to see the rest when `text` is cut (`call list with range`).
    pub more: Option<String>,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
            ..Self::default()
        }
    }

    pub fn with_images(mut self, images: Vec<Image>) -> Self {
        self.images = images;
        self
    }

    pub fn more(mut self, hint: impl Into<String>) -> Self {
        self.more = Some(hint.into());
        self
    }
}

/// The tools the model works with (`finish` and `read_skill_file` are
/// the loop's own).
pub trait Tools {
    fn specs(&self) -> Vec<ToolSpec>;
    /// Text for the system prompt before the skill: how the tools map to
    /// what the skill says, keys to use (so the model needn't explore a
    /// schema step by step).
    fn primer(&self) -> String {
        String::new()
    }
    fn call(&mut self, name: &str, args: &Value, cancel: &Cancel) -> ToolOutput;
    /// Check the files `finish` declares: they exist, and documents have no
    /// error-level findings. `Err` is told to the model, which then has to
    /// fix and call `finish` again. `Ok` gives the files as shown to the
    /// user and the check's warnings (they don't block `finish`).
    fn check_outputs(&mut self, outputs: &[String]) -> Result<Checked, String>;
    /// The documents this job made so far that pass their check without
    /// errors: what a job that runs out of steps or budget hands over
    /// instead of nothing.
    fn made_outputs(&mut self) -> Checked {
        Checked::default()
    }
}

/// Outputs that passed their check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Checked {
    /// The files, as shown to the user.
    pub files: Vec<String>,
    /// The check's warnings left in them (`file: finding`).
    pub warnings: Vec<String>,
}

impl Checked {
    pub fn files(files: Vec<String>) -> Self {
        Self {
            files,
            warnings: Vec::new(),
        }
    }
}

/// Prices in USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Prices {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Prices {
    pub fn cost(&self, u: Usage) -> f64 {
        let fresh = u.input.saturating_sub(u.cached);
        (fresh as f64 * self.input + u.cached as f64 * self.cached + u.output as f64 * self.output)
            / 1_000_000.0
    }
}

/// When to stop, and how lean to keep the conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Model turns at most.
    pub max_steps: u32,
    /// Text-only turns answered with a nudge before giving up.
    pub max_nudges: u32,
    /// Input plus output tokens for the whole job.
    pub max_tokens: Option<u64>,
    /// USD for the whole job (needs `prices`).
    pub max_cost: Option<f64>,
    pub prices: Option<Prices>,
    /// Pictures kept in the conversation.
    pub max_images: usize,
    /// A tool result longer than this (characters) is cut.
    pub max_result_chars: usize,
    /// Strings longer than this in tool-call arguments are left out of
    /// later steps.
    pub compact_args_over: usize,
    /// Tool results this many steps old are cut to about half of
    /// `old_result_chars`.
    pub old_result_steps: u32,
    pub old_result_chars: usize,
    /// Tries again after a network error, time-out or server error.
    pub retries: u32,
    /// `reasoning_effort` sent with every step (`low` keeps a thinking
    /// model's hidden reasoning, paid as output, short).
    pub reasoning: Option<String>,
}

/// Steps a job gets by default. The jobs that finish well take 10 to 20;
/// past that a cheap model is mostly polishing.
pub const DEFAULT_MAX_STEPS: u32 = 24;

/// A step that can't afford this many output tokens isn't started: the
/// reply would be cut before it does anything.
const MIN_STEP_OUTPUT: u64 = 1_500;

/// Output tokens a step is asked to stay under when the budget allows
/// more (the server has its own limit too).
const MAX_STEP_OUTPUT: u64 = 16_384;

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_steps: DEFAULT_MAX_STEPS,
            max_nudges: 3,
            max_tokens: None,
            max_cost: None,
            prices: None,
            max_images: 4,
            max_result_chars: 12_000,
            compact_args_over: 1_500,
            old_result_steps: 4,
            old_result_chars: 3_000,
            retries: 2,
            reasoning: None,
        }
    }
}

/// Why the job ended without `finish`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// `max_steps` reached.
    Steps,
    /// The token or cost budget would be exceeded by another step.
    Budget,
    Cancelled,
    /// The model kept ending its turn without `finish`.
    NoFinish,
}

/// What happens while the agent runs, in order. Serialized (JSON lines)
/// with a `type` field.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    StepStart {
        step: u32,
    },
    /// A piece of streamed text (for a live view; `Text` repeats it whole).
    TextDelta {
        text: String,
    },
    Text {
        step: u32,
        text: String,
    },
    Reasoning {
        step: u32,
        text: String,
    },
    ToolCall {
        step: u32,
        id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        step: u32,
        id: String,
        name: String,
        ok: bool,
        /// What the model was given (cut as it was).
        text: String,
        /// Pictures attached (labels).
        images: Vec<String>,
    },
    /// Pictures dropped from the conversation (labels).
    ImagesDropped {
        step: u32,
        labels: Vec<String>,
    },
    Nudge {
        step: u32,
        count: u32,
        reason: String,
    },
    Usage {
        step: u32,
        usage: Usage,
        total: Usage,
        cost: Option<f64>,
        /// False when the server sent no usage and it was estimated.
        reported: bool,
    },
    Done {
        summary: String,
        outputs: Vec<String>,
        /// Set when the job ran into this limit before the model called
        /// `finish`, and ended with the documents it had made (they pass
        /// their check).
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<StopReason>,
    },
    Stopped {
        reason: StopReason,
        message: String,
    },
    Error {
        message: String,
    },
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ending {
    Finished {
        summary: String,
        outputs: Vec<String>,
    },
    Stopped {
        reason: StopReason,
        message: String,
    },
    Failed(AiError),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub ending: Ending,
    pub usage: Usage,
    pub cost: Option<f64>,
    pub steps: u32,
    /// Whether every step's usage came from the server.
    pub usage_reported: bool,
    /// The conversation as last sent (after the system prompt).
    pub messages: Vec<Value>,
    /// `Finished` at this limit with the documents made so far (see
    /// [`Event::Done`]).
    pub limit: Option<StopReason>,
}

/// The harness's own rules, before the tools' primer and the skill.
const RULES: &str = "\
You are Paparan AI, working inside Kantor (an office suite) on the user's files. \
You work alone: nobody answers questions mid-job, so make reasonable assumptions and \
state them in the summary.

How this job runs:
- Every step is a tool call. A reply without a tool call does NOT end the job and \
does not reach the user; never end a turn with a plan or \"let me…\": do it.
- The job ends only when you call `finish` with `summary` (your answer or report for the \
user, complete: what you made, assumptions, what to confirm, warnings you left) and \
`outputs` (the files you made or changed). `finish` is refused while an output is missing or \
its check reports errors. Warnings never block `finish`.
- Write everything the user reads (your short notes between tool calls and the summary) in \
the language of the user's request, even though the skill below is in English.
- Fewer, bigger steps cost less: each step resends the whole conversation, and long \
deliberation is paid too. Batch work, write whole files at once, read only the skill files \
the job needs, and keep notes between tool calls to a sentence.
- Pictures and long results from earlier steps may be replaced by a note to save space; \
look again if you need them.
- Done means: the outputs are built, their check has 0 errors, and you have looked at the \
pages once after the last change. Fix warnings that change what the reader sees in one \
pass; leave the rest and name them in the summary. Don't chase every warning and don't \
polish beyond what the skill's review asks: steps and credits are limited, and a job that \
runs out before `finish` hands over less.";

/// The language of the user's request (Indonesian or English), told to
/// the model outright: cheap models narrate in the skill's English, or
/// follow the data's language, otherwise.
pub fn request_language(task: &str) -> Option<&'static str> {
    const ID: &[&str] = &[
        "yang",
        "dan",
        "dari",
        "untuk",
        "buat",
        "buatkan",
        "dengan",
        "ini",
        "itu",
        "tolong",
        "saya",
        "kami",
        "jadi",
        "ubah",
        "surat",
        "laporan",
        "rekap",
        "anggaran",
        "penjualan",
        "dalam",
        "akan",
        "atau",
        "tahun",
        "grafik",
        "setiap",
        "tiap",
        "sudah",
        "bisa",
        "harus",
        "kota",
        "undangan",
        "rapat",
        "bulanan",
        "per",
        "kepada",
        "perusahaan",
        "pak",
        "bu",
        "tanggal",
        "jumlah",
        "berisi",
        "agar",
        "pada",
        "tentang",
        "juga",
        "slide",
        "paparan",
        "presentasi",
        "ringkasan",
        "kantor",
        "cabang",
        "hasil",
        "lalu",
        "kemudian",
        "di",
        "ke",
    ];
    let words: Vec<String> = task
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return None;
    }
    let hits = words.iter().filter(|w| ID.contains(&w.as_str())).count();
    if hits >= 3 || (hits >= 2 && hits * 6 >= words.len()) {
        return Some("Indonesian");
    }
    // English needs saying too: with Indonesian data and examples, the
    // model otherwise answers an English request in Indonesian.
    const EN: &[&str] = &[
        "the", "a", "an", "and", "of", "for", "from", "on", "with", "to", "in", "by", "make",
        "create", "write", "build", "please", "our", "my", "deck", "report", "letter", "workbook",
        "sheet", "summary", "each", "per", "into", "about", "this", "that", "is", "are",
    ];
    let en = words.iter().filter(|w| EN.contains(&w.as_str())).count();
    (en >= 3 || (en >= 2 && en * 4 >= words.len())).then_some("English")
}

/// The system prompt: the rules, the tools' primer, the skill.
pub fn system_prompt(
    tools: &dyn Tools,
    skill: Option<&dyn PromptSource>,
) -> Result<String, String> {
    let mut out = String::from(RULES);
    let primer = tools.primer();
    if !primer.trim().is_empty() {
        out.push_str("\n\n");
        out.push_str(primer.trim());
    }
    if let Some(skill) = skill {
        let body = skill.system()?;
        out.push_str(&format!(
            "\n\n# Skill: {}\n\nThe skill was written for a terminal; use the tools as mapped above.\n\n{}",
            skill.name(),
            body.trim()
        ));
        let files = skill.files();
        if !files.is_empty() {
            out.push_str(&format!(
                "\n\n# The skill's other files (read with `{READ_SKILL_FILE}` only when needed)\n\n{}",
                files.join("\n")
            ));
        }
    }
    Ok(out)
}

/// The note on the language to answer in, after the system prompt's
/// skill (the last thing the model reads there).
fn language_note(lang: &str) -> String {
    format!(
        "\n\n# Language\n\nThe user wrote in {lang}. Write every note between tool calls and the \
         `finish` summary in {lang}. The documents follow the request (its language, unless it \
         asks for another)."
    )
}

fn finish_spec() -> ToolSpec {
    ToolSpec::new(
        FINISH,
        "End the job. Call it once the work is done and checked (or can't be done). \
         Refused while an output is missing or a document's check reports errors.",
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "The answer or report for the user, in their language: what you made or found, assumptions, what to confirm, warnings you accepted."},
                "outputs": {"type": "array", "items": {"type": "string"}, "description": "Files you made or changed (paths in the work folder). Empty for an answer-only job."},
            },
            "required": ["summary", "outputs"],
        }),
    )
}

fn read_skill_spec() -> ToolSpec {
    ToolSpec::new(
        READ_SKILL_FILE,
        "Read one of the skill's files (references/…, examples/…), as listed in the system prompt. Pictures come back as images. Long files: page with offset/limit (lines).",
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path in the skill, e.g. references/charts.md"},
                "offset": {"type": "integer", "description": "First line (from 1)"},
                "limit": {"type": "integer", "description": "Lines to read"},
            },
            "required": ["path"],
        }),
    )
}

/// Lines `offset..offset+limit` (from 1) of `text`, with a note when more
/// follow.
pub fn page_lines(text: &str, offset: Option<u64>, limit: Option<u64>) -> String {
    if offset.is_none() && limit.is_none() {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).max(1) as usize - 1;
    let end = limit.map_or(lines.len(), |n| start.saturating_add(n as usize));
    let end = end.min(lines.len());
    if start >= lines.len() {
        return format!(
            "[no line {}: the text has {} lines]",
            start + 1,
            lines.len()
        );
    }
    let mut out = lines[start..end].join("\n");
    if end < lines.len() {
        out.push_str(&format!(
            "\n[lines {}-{} of {}; offset {} reads on]",
            start + 1,
            end,
            lines.len(),
            end + 1
        ));
    }
    out
}

/// Whether a text-only turn says more work is coming ("Let me…").
pub fn announces_more_work(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "let me",
        "let's",
        "i'll",
        "i will",
        "i'm going to",
        "i am going to",
        "now i",
        "next, i",
        "next i",
        "now let",
        "i need to",
        "saya akan",
        "mari ",
        "selanjutnya",
        "sekarang saya",
        "berikutnya",
        "kita akan",
    ];
    PHRASES.iter().any(|p| t.contains(p))
        || t.ends_with(':')
        || t.ends_with("...")
        || t.ends_with('…')
}

/// Rough tokens for text when the server reports none (4 characters each).
fn estimate(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

fn retryable(e: &AiError) -> bool {
    match e {
        AiError::Network(_) | AiError::Timeout => true,
        AiError::Http { status, .. } => *status >= 500 || *status == 429,
        AiError::RateLimited { retry_after, .. } => retry_after.is_none_or(|s| s <= 30),
        _ => false,
    }
}

/// Run one job: `task` is the user's request.
pub fn run(
    model: &dyn Model,
    tools: &mut dyn Tools,
    skill: Option<&dyn PromptSource>,
    task: &str,
    limits: &Limits,
    cancel: &Cancel,
    on_event: &mut dyn FnMut(&Event),
) -> Outcome {
    run_in(
        Context::new(task),
        model,
        tools,
        skill,
        limits,
        cancel,
        on_event,
    )
}

/// Go on with a job: `history` is the conversation an earlier run ended
/// with ([`Outcome::messages`]), `task` the user's follow-up ("make slide 3
/// shorter"). The model sees what it did before; the budgets start again.
#[allow(clippy::too_many_arguments)]
pub fn resume(
    model: &dyn Model,
    tools: &mut dyn Tools,
    skill: Option<&dyn PromptSource>,
    history: Vec<Value>,
    task: &str,
    limits: &Limits,
    cancel: &Cancel,
    on_event: &mut dyn FnMut(&Event),
) -> Outcome {
    run_in(
        Context::resume(history, task),
        model,
        tools,
        skill,
        limits,
        cancel,
        on_event,
    )
}

/// How much budget is left before a step, and what the step may spend.
struct Room {
    /// Output tokens the next step can afford after its input; `None`
    /// without a budget.
    output: Option<u64>,
    /// The share of the tightest budget already spent (0 to 1).
    used: f64,
}

/// The room left for the next step: its input is the last step's grown by
/// what was added since (`growth` tokens), priced as if the last step's
/// cached prefix stays cached.
fn room(limits: &Limits, total: Usage, last: Usage, next_input: u64) -> Room {
    let mut out: Option<u64> = None;
    let mut used: f64 = 0.0;
    let mut tighten = |n: u64| out = Some(out.map_or(n, |o| o.min(n)));
    if let Some(max) = limits.max_tokens {
        let spent = total.input + total.output;
        used = used.max(spent as f64 / max.max(1) as f64);
        tighten(max.saturating_sub(spent).saturating_sub(next_input));
    }
    if let (Some(max), Some(p)) = (limits.max_cost, limits.prices) {
        let spent = p.cost(total);
        used = used.max(if max > 0.0 { spent / max } else { 1.0 });
        let input = p.cost(Usage {
            input: next_input,
            cached: last.cached.min(next_input),
            output: 0,
        });
        let per_token = p.output / 1_000_000.0;
        let left = max - spent - input;
        tighten(if left <= 0.0 {
            0
        } else if per_token <= 0.0 {
            u64::MAX
        } else {
            (left / per_token) as u64
        });
    }
    Room { output: out, used }
}

/// Characters of the conversation as the model gets it, a picture
/// counted as about 1,000 tokens.
fn conversation_chars(messages: &[Value]) -> usize {
    fn go(v: &Value) -> usize {
        match v {
            Value::String(s) => s.len(),
            Value::Array(a) => a.iter().map(go).sum(),
            Value::Object(m) => {
                if m.get("type").and_then(Value::as_str) == Some("image_url") {
                    return 4_000;
                }
                m.iter().map(|(k, v)| k.len() + go(v)).sum()
            }
            _ => 4,
        }
    }
    messages.iter().map(go).sum()
}

/// What the job hands over when it ends at `reason` without `finish`: the
/// documents it made that pass their check, with a summary saying so (in
/// the user's language as far as the harness knows it).
fn fallback(
    reason: StopReason,
    tools: &mut dyn Tools,
    lang: Option<&str>,
    last_text: &str,
) -> Option<(String, Vec<String>)> {
    if reason == StopReason::Cancelled {
        return None;
    }
    let made = tools.made_outputs();
    if made.files.is_empty() {
        return None;
    }
    let id = lang == Some("Indonesian");
    let limit = match (reason, id) {
        (StopReason::Steps, false) => "its step limit",
        (StopReason::Steps, true) => "batas langkahnya",
        (StopReason::Budget, false) => "its budget",
        (StopReason::Budget, true) => "batas biayanya",
        (_, false) => "its limit",
        (_, true) => "batasnya",
    };
    let mut summary = if id {
        format!(
            "Pekerjaan mencapai {limit} sebelum model menutupnya, jadi Paparan AI menyerahkan \
             file yang sudah dibuat. Semuanya lolos pemeriksaan tanpa galat: {}.",
            made.files.join(", ")
        )
    } else {
        format!(
            "The job reached {limit} before the model wrapped it up, so Paparan AI hands over \
             the files it made. They pass their check with no errors: {}.",
            made.files.join(", ")
        )
    };
    let last = last_text.trim();
    if !last.is_empty() {
        summary.push_str(if id {
            "\n\nCatatan terakhir model:\n\n"
        } else {
            "\n\nThe model's last note:\n\n"
        });
        summary.push_str(context::cut(last, 1_500));
    }
    summary.push_str(&warnings_note(&made.warnings, lang));
    Some((summary, made.files))
}

/// The check's warnings left in the outputs, for the end of a summary.
fn warnings_note(warnings: &[String], lang: Option<&str>) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    const SHOWN: usize = 8;
    let mut out = if lang == Some("Indonesian") {
        format!(
            "\n\n**Peringatan pemeriksaan yang tersisa ({}):**\n",
            warnings.len()
        )
    } else {
        format!("\n\n**Check warnings left ({}):**\n", warnings.len())
    };
    for w in warnings.iter().take(SHOWN) {
        out.push_str(&format!("- {}\n", context::cut(w, 240).trim()));
    }
    if warnings.len() > SHOWN {
        out.push_str(&format!("- … {}\n", warnings.len() - SHOWN));
    }
    out.trim_end().to_owned()
}

/// Ends the job at a limit: with the documents made so far when they pass
/// their check, else stopped.
fn at_limit(
    lang: Option<&str>,
    reason: StopReason,
    message: String,
    tools: &mut dyn Tools,
    last_text: &str,
    on_event: &mut dyn FnMut(&Event),
) -> (Ending, Option<StopReason>) {
    match fallback(reason, tools, lang, last_text) {
        Some((summary, outputs)) => {
            log::info!("agent: {message}; finishing with {}", outputs.join(", "));
            on_event(&Event::Done {
                summary: summary.clone(),
                outputs: outputs.clone(),
                limit: Some(reason),
            });
            (Ending::Finished { summary, outputs }, Some(reason))
        }
        None => (stop(reason, message, on_event), None),
    }
}

fn run_in(
    mut ctx: Context,
    model: &dyn Model,
    tools: &mut dyn Tools,
    skill: Option<&dyn PromptSource>,
    limits: &Limits,
    cancel: &Cancel,
    on_event: &mut dyn FnMut(&Event),
) -> Outcome {
    let mut total = Usage::default();
    let mut reported = true;
    let mut last_step = Usage::default();
    let mut outputs_seen: Vec<u64> = Vec::new();
    let mut nudges = 0;
    let mut seen = std::collections::HashSet::new();
    let mut step = 0;
    let mut last_text = String::new();
    let mut warned_budget = false;
    // Characters of the conversation when the last step was sent.
    let mut sent_chars = 0usize;
    let cost = |u: Usage| limits.prices.map(|p| p.cost(u));
    let end = |ending: Ending,
               ctx: Context,
               total: Usage,
               step: u32,
               reported: bool,
               limit: Option<StopReason>| Outcome {
        ending,
        usage: total,
        cost: limits.prices.map(|p| p.cost(total)),
        steps: step,
        usage_reported: reported,
        messages: ctx.into_messages(),
        limit,
    };
    let lang = ctx
        .messages()
        .iter()
        .rev()
        .find(|m| m["role"] == "user" && m["content"].is_string())
        .and_then(|m| m["content"].as_str())
        .and_then(request_language);
    let system = match system_prompt(tools, skill) {
        Ok(mut s) => {
            if let Some(l) = lang {
                s.push_str(&language_note(l));
            }
            s
        }
        Err(e) => {
            on_event(&Event::Error { message: e.clone() });
            return end(
                Ending::Failed(AiError::BadReply(e)),
                ctx,
                total,
                0,
                true,
                None,
            );
        }
    };
    let mut specs = tools.specs();
    if skill.is_some() {
        specs.push(read_skill_spec());
    }
    specs.push(finish_spec());
    let tools_json = Value::Array(specs.iter().map(ToolSpec::json).collect());
    let finish_only = Value::Array(vec![finish_spec().json()]);

    loop {
        if cancel.is_cancelled() {
            let e = stop(StopReason::Cancelled, "cancelled".into(), on_event);
            return end(e, ctx, total, step, reported, None);
        }
        if step >= limits.max_steps {
            let (e, l) = at_limit(
                lang,
                StopReason::Steps,
                format!("stopped after {step} steps without `finish`"),
                tools,
                &last_text,
                on_event,
            );
            return end(e, ctx, total, step, reported, l);
        }
        // The budget: the next step's input is about the last one's plus
        // what was added since; its output gets what is left after that
        // (the server cuts the reply there), and a step that couldn't do
        // anything useful with it isn't started.
        let chars_now = conversation_chars(ctx.messages());
        let next_input = if step == 0 {
            estimate(chars_now + system.len() + tools_json.to_string().len())
        } else {
            last_step.input + estimate(chars_now.saturating_sub(sent_chars))
        };
        let room = room(limits, total, last_step, next_input);
        let mut opts = StepOptions {
            max_output: None,
            reasoning: limits.reasoning.clone(),
        };
        if let Some(out) = room.output {
            if out < MIN_STEP_OUTPUT {
                let message = match (limits.max_cost, limits.prices) {
                    (Some(max), Some(p)) => format!(
                        "cost budget: {:.4} of {max:.4} spent, too little left for another step",
                        p.cost(total)
                    ),
                    _ => format!(
                        "token budget: {} of {} used, too little left for another step",
                        total.input + total.output,
                        limits.max_tokens.unwrap_or_default()
                    ),
                };
                let (e, l) = at_limit(
                    lang,
                    StopReason::Budget,
                    message,
                    tools,
                    &last_text,
                    on_event,
                );
                return end(e, ctx, total, step, reported, l);
            }
            if out < MAX_STEP_OUTPUT {
                opts.max_output = Some(out as u32);
            }
            // A typical step's output: the largest of the last few.
            let typical = outputs_seen
                .iter()
                .rev()
                .take(3)
                .copied()
                .max()
                .unwrap_or(0)
                .max(MIN_STEP_OUTPUT);
            if !warned_budget && step > 0 && (room.used >= 0.7 || out < typical * 3) {
                warned_budget = true;
                ctx.push(
                    step,
                    json!({"role": "user", "content": format!(
                        "The budget for this job is nearly spent ({:.0}% used). Wrap up now: if \
                         the outputs aren't built yet, build them as they are; then call `finish` \
                         with what exists and list what you would still improve in the summary. \
                         Don't start new improvements, and keep each step short.",
                        room.used * 100.0
                    )}),
                );
            }
        }

        step += 1;
        // Old results are cut every few steps, not every step: each cut
        // ends the provider's prompt cache there, and cached input costs a
        // fraction of fresh input.
        let results_before = if limits.old_result_steps > 0 && step % limits.old_result_steps == 0 {
            (step + 1).saturating_sub(limits.old_result_steps)
        } else {
            0
        };
        ctx.compact(
            step,
            limits.compact_args_over,
            results_before,
            limits.old_result_chars,
        );
        // The last step can only call `finish`.
        let last = step >= limits.max_steps;
        let step_tools = if last { &finish_only } else { &tools_json };
        on_event(&Event::StepStart { step });
        sent_chars = conversation_chars(ctx.messages());
        let mut tries = 0;
        let reply = loop {
            let r = model.complete_with(
                &system,
                ctx.messages(),
                step_tools,
                &opts,
                cancel,
                &mut |t| on_event(&Event::TextDelta { text: t.to_owned() }),
            );
            match r {
                Err(e) if tries < limits.retries && retryable(&e) && !cancel.is_cancelled() => {
                    tries += 1;
                    let secs = match &e {
                        AiError::RateLimited {
                            retry_after: Some(s),
                            ..
                        } => *s,
                        _ => 2 * u64::from(tries),
                    };
                    log::warn!("agent step {step}: {e}; trying again in {secs} s");
                    if crate::client::wait(std::time::Duration::from_secs(secs), cancel).is_err() {
                        break Err(AiError::Cancelled);
                    }
                }
                // A reply with nothing in it is a turn without `finish`.
                Err(AiError::BadReply(m)) if m.contains("empty answer") => {
                    break Ok(Reply::default());
                }
                other => break other,
            }
        };
        let reply = match reply {
            Ok(r) => r,
            Err(AiError::Cancelled) => {
                let e = stop(StopReason::Cancelled, "cancelled".into(), on_event);
                return end(e, ctx, total, step, reported, None);
            }
            Err(e) => {
                on_event(&Event::Error {
                    message: e.to_string(),
                });
                return end(Ending::Failed(e), ctx, total, step, reported, None);
            }
        };

        let usage = match reply.usage {
            Some(u) => u,
            None => {
                reported = false;
                let sent: usize = system.len()
                    + step_tools.to_string().len()
                    + ctx
                        .messages()
                        .iter()
                        .map(|m| m.to_string().len())
                        .sum::<usize>();
                let said = reply.text.len()
                    + reply.reasoning.len()
                    + reply
                        .tool_calls
                        .iter()
                        .map(|c| c.name.len() + c.arguments.len())
                        .sum::<usize>();
                Usage {
                    input: estimate(sent),
                    cached: 0,
                    output: estimate(said),
                }
            }
        };
        total.add(usage);
        last_step = usage;
        outputs_seen.push(usage.output);
        on_event(&Event::Usage {
            step,
            usage,
            total,
            cost: cost(total),
            reported: reply.usage.is_some(),
        });
        if !reply.reasoning.trim().is_empty() {
            on_event(&Event::Reasoning {
                step,
                text: reply.reasoning.clone(),
            });
        }
        if !reply.text.trim().is_empty() {
            last_text = reply.text.clone();
            on_event(&Event::Text {
                step,
                text: reply.text.clone(),
            });
        }
        if reply.finish_reason.as_deref() == Some("length") {
            log::info!("agent step {step}: the reply reached its output limit");
        }

        // The assistant turn, as the API expects it back.
        let mut calls = reply.tool_calls.clone();
        for (i, c) in calls.iter_mut().enumerate() {
            if c.id.is_empty() {
                c.id = format!("call_{step}_{i}");
            }
        }
        let mut turn = json!({"role": "assistant", "content": reply.text});
        if !calls.is_empty() {
            turn["tool_calls"] = Value::Array(
                calls
                    .iter()
                    .map(|c| {
                        // Arguments that aren't JSON go back as `{}`: some
                        // servers reject the conversation otherwise.
                        let args = if serde_json::from_str::<Value>(&c.arguments).is_ok() {
                            c.arguments.as_str()
                        } else {
                            "{}"
                        };
                        json!({"id": c.id, "type": "function", "function": {
                            "name": c.name,
                            "arguments": args,
                        }})
                    })
                    .collect(),
            );
            if reply.text.is_empty() {
                turn["content"] = Value::Null;
            }
        }
        ctx.push(step, turn);

        if calls.is_empty() {
            nudges += 1;
            if nudges > limits.max_nudges {
                let (e, l) = at_limit(
                    lang,
                    StopReason::NoFinish,
                    format!(
                        "the model ended {nudges} turns without calling `finish`; its last words: {}",
                        context::cut(reply.text.trim(), 300)
                    ),
                    tools,
                    &last_text,
                    on_event,
                );
                return end(e, ctx, total, step, reported, l);
            }
            let (reason, nudge) = if announces_more_work(&reply.text) {
                (
                    "announced work without doing it",
                    "You said what you would do next but ended your turn without a tool call, \
                     so nothing happened. Do it now with tools. The job ends only when you call \
                     `finish` with the summary for the user and the output files.",
                )
            } else if reply.text.trim().is_empty() {
                (
                    "empty turn",
                    "Your turn had no text and no tool call. Continue the job with tools, \
                     and call `finish` when it is done.",
                )
            } else {
                (
                    "ended without finish",
                    "That text did not reach the user: a turn without a tool call is not the \
                     end of the job. If the work is done and checked, call `finish` with the full \
                     answer as `summary` and the output files; otherwise continue with tools.",
                )
            };
            on_event(&Event::Nudge {
                step,
                count: nudges,
                reason: reason.into(),
            });
            ctx.push(step, json!({"role": "user", "content": nudge}));
            continue;
        }

        // Run the calls; `finish` last, after the others in the same turn.
        let mut order: Vec<usize> = (0..calls.len()).collect();
        order.sort_by_key(|&i| calls[i].name == FINISH);
        let mut results: Vec<(usize, Value)> = Vec::new();
        let mut images = Vec::new();
        let mut finished = None;
        for i in order {
            let c = &calls[i];
            let args: Result<Value, String> = if c.arguments.trim().is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_str(&c.arguments).map_err(|e| {
                    if reply.finish_reason.as_deref() == Some("length") {
                        "the reply was cut at its output limit before the arguments were complete; \
                         send less in one call"
                            .to_owned()
                    } else {
                        format!("the arguments are not valid JSON ({e}); send them again")
                    }
                })
            };
            on_event(&Event::ToolCall {
                step,
                id: c.id.clone(),
                name: c.name.clone(),
                arguments: args
                    .clone()
                    .unwrap_or_else(|_| Value::String(c.arguments.clone())),
            });
            let out = match args {
                Err(e) => ToolOutput::error(e),
                Ok(args) if !args.is_object() => {
                    ToolOutput::error("the arguments must be a JSON object")
                }
                Ok(args) => match c.name.as_str() {
                    FINISH => {
                        let summary = args
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim()
                            .to_owned();
                        let outputs: Vec<String> = args
                            .get("outputs")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        if summary.is_empty() {
                            ToolOutput::error(
                                "finish refused: `summary` is empty. Give the user's answer or report.",
                            )
                        } else {
                            match tools.check_outputs(&outputs) {
                                Ok(checked) => {
                                    let summary = format!(
                                        "{summary}{}",
                                        warnings_note(&checked.warnings, lang)
                                    );
                                    finished = Some((summary, checked.files));
                                    ToolOutput::ok("finished")
                                }
                                Err(why) => ToolOutput::error(format!(
                                    "finish refused: {why}\nFix the errors (warnings may stay), then call finish again."
                                )),
                            }
                        }
                    }
                    READ_SKILL_FILE => match skill {
                        None => ToolOutput::error("there is no skill in this job"),
                        Some(s) => read_skill(s, &args),
                    },
                    name if specs.iter().any(|s| s.name == name) && !last => {
                        tools.call(name, &args, cancel)
                    }
                    name if last => ToolOutput::error(format!(
                        "{name} not run: this was the last step, only `finish` could run"
                    )),
                    name => ToolOutput::error(format!(
                        "there is no tool {name}; the tools are {}",
                        specs
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                },
            };
            let text = truncate_result(&out.text, limits.max_result_chars, out.more.as_deref());
            let labels: Vec<String> = out.images.iter().map(|i| i.label.clone()).collect();
            on_event(&Event::ToolResult {
                step,
                id: c.id.clone(),
                name: c.name.clone(),
                ok: !out.error,
                text: text.clone(),
                images: labels.clone(),
            });
            let content = if labels.is_empty() {
                text
            } else {
                format!("{text}\n(pictures follow: {})", labels.join(", "))
            };
            results.push((
                i,
                json!({"role": "tool", "tool_call_id": c.id, "content": content}),
            ));
            images.extend(out.images);
            if cancel.is_cancelled() {
                break;
            }
        }
        // Tool messages in the order of the calls.
        results.sort_by_key(|(i, _)| *i);
        for (_, m) in results {
            ctx.push(step, m);
        }
        if let Some((summary, outputs)) = finished {
            on_event(&Event::Done {
                summary: summary.clone(),
                outputs: outputs.clone(),
                limit: None,
            });
            return end(
                Ending::Finished { summary, outputs },
                ctx,
                total,
                step,
                reported,
                None,
            );
        }
        if !images.is_empty() {
            // Looking again at a page that didn't change is a wasted step:
            // say so.
            let unchanged: Vec<String> = images
                .iter()
                .filter(|i| !seen.insert((i.file.clone(), i.page.clone(), i.version.clone())))
                .map(|i| i.label.clone())
                .collect();
            let intro = if unchanged.is_empty() {
                "The pictures you asked for:".to_owned()
            } else {
                format!(
                    "The pictures you asked for. Unchanged since you last saw them (no need to look again): {}.",
                    unchanged.join(", ")
                )
            };
            let dropped = ctx.push_images(step, &intro, images, limits.max_images);
            if !dropped.is_empty() {
                on_event(&Event::ImagesDropped {
                    step,
                    labels: dropped,
                });
            }
        }
        // Near the step limit, say how many are left, firmly (models
        // polish forever otherwise). The last step offers only `finish`.
        let left = limits.max_steps.saturating_sub(step);
        if left <= (limits.max_steps / 4).max(3) {
            let note = if left <= 1 {
                "One step left, and it can only call `finish`: call it now with the summary and \
                 the outputs that exist (0 errors needed; warnings may stay, name them). List what \
                 you would still improve in the summary."
                    .to_owned()
            } else {
                format!(
                    "{left} steps left of {}. Wrap up: if the outputs aren't built, build them now; \
                     fix only check errors; then call `finish` with what exists. Put further \
                     polish in the summary as suggestions instead of doing it.",
                    limits.max_steps
                )
            };
            ctx.push(step, json!({"role": "user", "content": note}));
        }
    }
}

fn stop(reason: StopReason, message: String, on_event: &mut dyn FnMut(&Event)) -> Ending {
    on_event(&Event::Stopped {
        reason,
        message: message.clone(),
    });
    Ending::Stopped { reason, message }
}

fn read_skill(skill: &dyn PromptSource, args: &Value) -> ToolOutput {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return ToolOutput::error("`path` is required");
    };
    match skill.read(path) {
        Err(e) => ToolOutput::error(e),
        Ok(SkillFile::Text(t)) => {
            let n = |k: &str| args.get(k).and_then(Value::as_u64);
            ToolOutput::ok(page_lines(&t, n("offset"), n("limit"))).more(format!(
                "read on with {READ_SKILL_FILE} offset/limit (lines)"
            ))
        }
        Ok(SkillFile::Image { mime, bytes }) => ToolOutput::ok(format!("{path}: a picture"))
            .with_images(vec![Image {
                file: format!("skill:{path}"),
                page: None,
                version: "skill".into(),
                label: format!("skill file {path}"),
                mime,
                bytes,
            }]),
    }
}
