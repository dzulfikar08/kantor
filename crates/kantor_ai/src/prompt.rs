//! Prompts for each feature and parsing of their JSON replies.
//!
//! Replies are asked for as one JSON object; parsing tolerates code fences
//! and text around the object, since not every provider has a JSON mode.

use serde::Deserialize;

use crate::AiError;
use crate::client::ChatRequest;
use crate::sample::SheetSample;

/// The language for answers that don't follow a user request (explanations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    English,
    Indonesian,
}

impl Lang {
    /// From the system locale (`LC_ALL`, `LC_MESSAGES`, `LANG`): `id_*` is
    /// Indonesian, anything else English.
    pub fn from_env() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find(|v| !v.is_empty())
            .unwrap_or_default();
        Self::from_locale(&locale)
    }

    pub fn from_locale(locale: &str) -> Self {
        let l = locale.to_ascii_lowercase();
        if l.starts_with("id") || l.starts_with("in_") {
            Self::Indonesian
        } else {
            Self::English
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Indonesian => "Bahasa Indonesia",
        }
    }
}

// ── Sheets: formula from a description ─────────────────────────────────────

const FORMULA_SYSTEM: &str = "You write spreadsheet formulas for Kantor Sheets, whose \
engine (IronCalc) is compatible with Excel.

Write ONE formula for the active cell that does what the user asks.
Rules:
- Use Excel function names in English and commas between arguments, whatever language the user writes in.
- Refer to cells in A1 notation with the column letters and row numbers given. Cover the whole data range (all data rows), not only the sample rows.
- Prefer widely supported functions: SUM, SUMIF(S), COUNTIF(S), AVERAGEIF(S), IF, IFERROR, AND, OR, VLOOKUP, XLOOKUP, INDEX, MATCH, ROUND, TEXT, LEFT, RIGHT, MID, LEN, UPPER, LOWER, TRIM, DATE, YEAR, MONTH, TODAY, MAX, MIN.
- Text criteria go in double quotes, e.g. \"Jakarta\".
- The active cell must not refer to itself.

Reply with only a JSON object, no other text:
{\"formula\": \"=...\", \"explanation\": \"one short sentence\"}
Write the explanation in the same language as the user's request (Bahasa Indonesia or English).
If a formula can't do it, reply {\"formula\": \"\", \"explanation\": \"why, in one sentence\"}.";

/// The formula request: the description plus the sample (headers and up to
/// [`crate::sample::MAX_SAMPLE_ROWS`] rows), nothing else.
pub fn formula_request(request: &str, sample: &SheetSample) -> ChatRequest {
    let mut user = format!("Request: {}\n\n", request.trim());
    user.push_str(&describe_sheet(sample, true));
    ChatRequest::new(FORMULA_SYSTEM, user)
        .json()
        .max_tokens(600)
}

fn describe_sheet(s: &SheetSample, with_rows: bool) -> String {
    let mut out = format!("Sheet: {}\nActive cell: {}\n", s.sheet, s.active);
    if s.used_range.is_empty() {
        out.push_str("The sheet is empty.\n");
        return out;
    }
    out.push_str(&format!("Used range: {}\n", s.used_range));
    if let Some(h) = s.header_row {
        out.push_str(&format!("Header row: {}\n", h + 1));
    }
    if let Some((a, b)) = s.data_rows {
        out.push_str(&format!("Data rows: {} to {}\n", a + 1, b + 1));
    }
    out.push_str("Columns:\n");
    for (letter, header) in &s.columns {
        if header.is_empty() {
            out.push_str(&format!("{letter}\n"));
        } else {
            out.push_str(&format!("{letter}: {header}\n"));
        }
    }
    if with_rows && !s.rows.is_empty() {
        out.push_str(&format!(
            "Sample rows ({} of {}, values as displayed):\n",
            s.rows.len(),
            s.data_rows.map_or(0, |(a, b)| b - a + 1)
        ));
        for (row, values) in &s.rows {
            let cells: Vec<String> = s
                .columns
                .iter()
                .zip(values)
                .filter(|(_, v)| !v.is_empty())
                .map(|((letter, _), v)| format!("{letter}={v}"))
                .collect();
            out.push_str(&format!("row {}: {}\n", row + 1, cells.join(" | ")));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaReply {
    /// Starts with `=`.
    pub formula: String,
    pub explanation: String,
}

pub fn parse_formula_reply(text: &str) -> Result<FormulaReply, AiError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        formula: String,
        #[serde(default)]
        explanation: String,
    }
    let raw: Raw = parse_json_object(text)?;
    let explanation = one_line(&raw.explanation);
    let formula = one_line(&raw.formula);
    if formula.is_empty() || formula == "=" {
        return Err(AiError::NoAnswer(if explanation.is_empty() {
            "The model couldn't find a formula for that.".into()
        } else {
            explanation
        }));
    }
    let formula = if formula.starts_with('=') {
        formula
    } else {
        format!("={formula}")
    };
    Ok(FormulaReply {
        formula,
        explanation,
    })
}

// ── Sheets: explain a formula ──────────────────────────────────────────────

/// Explain the active cell's formula. Sends the formula, the cell and the
/// column headers only (no rows).
pub fn explain_request(formula: &str, sample: &SheetSample, lang: Lang) -> ChatRequest {
    let system = format!(
        "You explain spreadsheet formulas (Excel-compatible) to non-experts. \
         Explain what the formula computes in at most three short sentences, naming \
         columns by their headers when known. Write in {}.\n\
         Reply with only a JSON object: {{\"explanation\": \"...\"}}",
        lang.name()
    );
    let mut user = format!("Formula: {}\n\n", formula.trim());
    user.push_str(&describe_sheet(sample, false));
    ChatRequest::new(system, user).json().max_tokens(500)
}

pub fn parse_explain_reply(text: &str) -> Result<String, AiError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        explanation: String,
    }
    // Some models ignore the JSON instruction here; plain text is fine.
    let explanation = match parse_json_object::<Raw>(text) {
        Ok(raw) => raw.explanation,
        Err(_) => text.trim().to_owned(),
    };
    let explanation = explanation.trim().to_owned();
    if explanation.is_empty() {
        return Err(AiError::BadReply("no explanation".into()));
    }
    Ok(explanation)
}

// ── Docs: rewrite / translate selection ────────────────────────────────────

/// Most characters of selected text sent in one request.
pub const MAX_REWRITE_CHARS: usize = 12_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteMode {
    ToEnglish,
    ToIndonesian,
    Formal,
    Shorter,
    FixGrammar,
}

impl RewriteMode {
    pub const ALL: [Self; 5] = [
        Self::FixGrammar,
        Self::Formal,
        Self::Shorter,
        Self::ToEnglish,
        Self::ToIndonesian,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ToEnglish => "Translate to English",
            Self::ToIndonesian => "Translate to Indonesian",
            Self::Formal => "More formal",
            Self::Shorter => "Shorter",
            Self::FixGrammar => "Fix grammar",
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::ToEnglish => "Translate each paragraph into natural, fluent English.",
            Self::ToIndonesian => {
                "Translate each paragraph into natural, fluent Bahasa Indonesia (standard spelling, EYD)."
            }
            Self::Formal => {
                "Rewrite each paragraph in a more formal, professional tone. Keep its language (Bahasa Indonesia stays Bahasa Indonesia)."
            }
            Self::Shorter => {
                "Make each paragraph shorter and clearer while keeping its meaning. Keep its language."
            }
            Self::FixGrammar => {
                "Fix spelling, grammar and punctuation only. Keep the wording, tone and language otherwise."
            }
        }
    }
}

const REWRITE_SYSTEM: &str = "You edit text for a word processor.
Apply the instruction to the paragraphs given as a JSON array.
Return exactly as many paragraphs as you were given, in the same order. Keep names, numbers, \
dates and links unchanged. A \"\\n\" inside a paragraph is a line break; keep it where it makes \
sense. Don't add notes or explanations.
Reply with only a JSON object: {\"paragraphs\": [\"...\", ...]}";

/// The rewrite request for the selected paragraphs' text (nothing else).
pub fn rewrite_request(mode: RewriteMode, paragraphs: &[String]) -> Result<ChatRequest, AiError> {
    let total: usize = paragraphs.iter().map(|p| p.chars().count()).sum();
    if total == 0 {
        return Err(AiError::NoAnswer("Select some text first.".into()));
    }
    if total > MAX_REWRITE_CHARS {
        return Err(AiError::TooLarge(format!(
            "The selection is too long ({total} characters); select at most {MAX_REWRITE_CHARS}."
        )));
    }
    let list = serde_json::to_string(paragraphs).expect("strings serialize");
    let user = format!("Instruction: {}\n\nParagraphs:\n{list}", mode.instruction());
    // Room for the rewritten text (translations grow) plus JSON overhead.
    let max_tokens = ((total / 2) as u32 + 512).clamp(1024, 8192);
    Ok(ChatRequest::new(REWRITE_SYSTEM, user)
        .json()
        .max_tokens(max_tokens))
}

pub fn parse_rewrite_reply(text: &str) -> Result<Vec<String>, AiError> {
    #[derive(Deserialize)]
    struct Raw {
        paragraphs: Vec<String>,
    }
    let raw: Raw = parse_json_object(text)?;
    let paragraphs: Vec<String> = raw
        .paragraphs
        .into_iter()
        .map(|p| p.replace("\r\n", "\n").trim_end().to_owned())
        .collect();
    if paragraphs.iter().all(|p| p.trim().is_empty()) {
        return Err(AiError::BadReply("the rewrite is empty".into()));
    }
    Ok(paragraphs)
}

// ── helpers ────────────────────────────────────────────────────────────────

/// The first JSON object in `text` (after stripping ``` fences and any
/// leading or trailing prose).
pub fn parse_json_object<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, AiError> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str(t) {
        return Ok(v);
    }
    let start = t.find('{');
    let end = t.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if s < e => serde_json::from_str(&t[s..=e])
            .map_err(|err| AiError::BadReply(format!("invalid JSON ({err})"))),
        _ => Err(AiError::BadReply("expected a JSON object".into())),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::sample_sheet;

    fn col(c: u32) -> String {
        ((b'A' + c as u8) as char).to_string()
    }

    fn sales(r: u32, c: u32) -> String {
        match (r, c) {
            (0, 0) => "Region".into(),
            (0, 1) => "Sales".into(),
            (_, 0) => ["Jakarta", "Bandung", "Surabaya"][(r % 3) as usize].into(),
            (_, 1) => format!("{}", r * 100),
            _ => String::new(),
        }
    }

    #[test]
    fn formula_prompt_holds_only_the_sample() {
        let sample = sample_sheet("Sales", (200, 2), (0, 3), col, sales);
        let req = formula_request("jumlah penjualan untuk region Jakarta", &sample);
        assert!(req.json);
        assert!(
            req.user
                .contains("Request: jumlah penjualan untuk region Jakarta")
        );
        assert!(req.user.contains("Active cell: D1"));
        assert!(req.user.contains("Data rows: 2 to 200"));
        assert!(req.user.contains("A: Region\nB: Sales\n"));
        assert!(req.user.contains("row 2: A=Bandung | B=100"));
        let rows = req.user.lines().filter(|l| l.starts_with("row ")).count();
        assert_eq!(rows, crate::sample::MAX_SAMPLE_ROWS);
        assert!(!req.user.contains("row 30:"), "only the sample rows");
        assert!(req.system.contains("same language as the user's request"));
    }

    #[test]
    fn explain_prompt_has_headers_but_no_rows() {
        let sample = sample_sheet("Sales", (200, 2), (5, 3), col, sales);
        let req = explain_request(
            "=SUMIF(A2:A200,\"Jakarta\",B2:B200)",
            &sample,
            Lang::Indonesian,
        );
        assert!(req.system.contains("Bahasa Indonesia"));
        assert!(req.user.contains("A: Region"));
        assert!(!req.user.contains("row "));
        assert!(!req.user.contains("Jakarta |"));
    }

    #[test]
    fn parses_formula_replies() {
        let r = parse_formula_reply(
            "```json\n{\"formula\": \"SUMIF(A2:A200, \\\"Jakarta\\\", B2:B200)\", \"explanation\": \"Menjumlahkan penjualan\\nJakarta.\"}\n```",
        )
        .unwrap();
        assert_eq!(r.formula, "=SUMIF(A2:A200, \"Jakarta\", B2:B200)");
        assert_eq!(r.explanation, "Menjumlahkan penjualan Jakarta.");

        let r =
            parse_formula_reply("Sure! {\"formula\":\"=A1\",\"explanation\":\"x\"} Hope it helps")
                .unwrap();
        assert_eq!(r.formula, "=A1");

        assert_eq!(
            parse_formula_reply(
                "{\"formula\":\"\",\"explanation\":\"Needs a chart, not a formula.\"}"
            ),
            Err(AiError::NoAnswer("Needs a chart, not a formula.".into()))
        );
        assert!(matches!(
            parse_formula_reply("I can't do that"),
            Err(AiError::BadReply(_))
        ));
    }

    #[test]
    fn parses_explanations() {
        assert_eq!(
            parse_explain_reply("{\"explanation\":\"Adds B.\"}").unwrap(),
            "Adds B."
        );
        assert_eq!(
            parse_explain_reply("Adds column B.").unwrap(),
            "Adds column B."
        );
    }

    #[test]
    fn rewrite_prompt_and_caps() {
        let paras = vec!["Saya mau pergi.".to_owned(), "Line\nbreak".to_owned()];
        let req = rewrite_request(RewriteMode::ToEnglish, &paras).unwrap();
        assert!(req.user.contains("into natural, fluent English"));
        assert!(req.user.contains(r#"["Saya mau pergi.","Line\nbreak"]"#));

        let long = vec!["x".repeat(MAX_REWRITE_CHARS + 1)];
        assert!(matches!(
            rewrite_request(RewriteMode::Shorter, &long),
            Err(AiError::TooLarge(_))
        ));
        assert!(rewrite_request(RewriteMode::Shorter, &[String::new()]).is_err());
    }

    #[test]
    fn parses_rewrite_replies() {
        let p = parse_rewrite_reply("{\"paragraphs\":[\"I want to go.\",\"Line\\r\\nbreak \"]}")
            .unwrap();
        assert_eq!(p, vec!["I want to go.", "Line\nbreak"]);
        assert!(parse_rewrite_reply("{\"paragraphs\":[\"\"]}").is_err());
        assert!(parse_rewrite_reply("{\"text\":\"x\"}").is_err());
    }

    #[test]
    fn locale_language() {
        assert_eq!(Lang::from_locale("id_ID.UTF-8"), Lang::Indonesian);
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Lang::English);
        assert_eq!(Lang::from_locale(""), Lang::English);
    }
}
