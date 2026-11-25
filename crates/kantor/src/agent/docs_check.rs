//! `kantor docs check`: problems in a Word document, found on the pages as
//! Kantor lays them out (the same layout as the app, the PDF and the PNGs).
//! Each finding names the paragraph id (`kantor docs list --text`) and the
//! page. The checks are listed in docs/docs-json.md.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use docs_engine::object::Object;
use docs_engine::{Document, FloatKind, Paragraph, RunProps};
use serde_json::{Map, Value, json};

use super::docs_list::{Pages, direct, heading_level, shown, style_name};
pub use super::slides_check::Severity;
use super::slides_check::contrast;

/// Each check: name, severity, what it finds (for the help and docs).
pub const CHECKS: &[(&str, &str, &str)] = &[
    ("empty_heading", "error", "a heading with no text"),
    (
        "heading_skip",
        "warning",
        "a heading more than one level below the one before (Heading 1 then Heading 3)",
    ),
    (
        "fake_heading",
        "warning",
        "a short bold or large paragraph in a body style: a heading made by hand instead of with a Heading style",
    ),
    (
        "direct_formatting",
        "warning / info",
        "text formatted by hand (fonts, sizes, colours, bold) instead of with styles: warning when many paragraphs have it",
    ),
    (
        "broken_reference",
        "error",
        "a cross-reference (REF, PAGEREF, NOTEREF) to a bookmark that isn't there, or showing Word's \"Error! Reference source not found\"",
    ),
    (
        "toc_stale",
        "warning",
        "a table of contents that doesn't match the headings and pages (update fields), or has no entries",
    ),
    (
        "caption_numbers",
        "warning",
        "figure or table numbers (SEQ fields) that repeat, skip or are out of order",
    ),
    (
        "missing_caption",
        "info",
        "a picture or chart without a caption",
    ),
    (
        "missing_alt_text",
        "warning",
        "a picture or chart without alt text",
    ),
    (
        "table_header",
        "warning",
        "a table (of two rows or more) whose first row isn't a header row repeated on each page",
    ),
    (
        "cell_overflow",
        "error",
        "text wider than its table cell (a long word, number or link that can't wrap)",
    ),
    (
        "past_margin",
        "warning",
        "text, a table or a picture reaching past the right margin",
    ),
    (
        "heading_at_page_bottom",
        "warning / info",
        "a heading alone at the bottom of a page, its text on the next (info when the heading keeps with the next paragraph: Word moves it)",
    ),
    (
        "table_at_page_bottom",
        "warning / info",
        "a table whose header row sits alone at the bottom of a page, its body rows on the next (warning), or that starts with one body row before a page break (info)",
    ),
    (
        "mixed_fonts",
        "warning",
        "body text in more than one font family",
    ),
    (
        "placeholder_text",
        "error",
        "text left from a template: Lorem ipsum, TBD, TODO, XXX, [Client name], <insert …>, {{field}}",
    ),
    (
        "double_space",
        "info",
        "two spaces in a row inside a sentence",
    ),
    ("open_comments", "warning", "comments not marked resolved"),
    (
        "tracked_changes",
        "warning",
        "tracked changes not accepted or rejected",
    ),
    (
        "low_contrast",
        "warning / error",
        "text colour against its background (the page, or the cell's shading) under 4.5:1 (3:1 for large text); error under 2:1",
    ),
    (
        "font_missing",
        "warning / info",
        "a font not installed here: line breaks and pages can differ where it is (info when a metric-compatible substitute stands in)",
    ),
];

#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    pub check: &'static str,
    /// Paragraph id.
    pub para: Option<usize>,
    /// Page number from 1.
    pub page: Option<usize>,
    pub message: String,
    pub details: Map<String, Value>,
}

/// Everything found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub paragraphs: usize,
    pub pages: usize,
    pub findings: Vec<Finding>,
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl Report {
    pub fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }

    pub fn has_errors(&self) -> bool {
        self.count(Severity::Error) > 0
    }

    pub fn json(&self) -> Value {
        let findings: Vec<Value> = self
            .findings
            .iter()
            .map(|f| {
                let mut v = json!({
                    "severity": f.severity.name(), "check": f.check,
                    "paragraph": f.para, "page": f.page, "message": f.message,
                });
                if !f.details.is_empty() {
                    v["details"] = Value::Object(f.details.clone());
                }
                v
            })
            .collect();
        json!({
            "paragraphs": self.paragraphs,
            "pages": self.pages,
            "errors": self.count(Severity::Error),
            "warnings": self.count(Severity::Warning),
            "info": self.count(Severity::Info),
            "findings": findings,
        })
    }

    pub fn text(&self, file: &str) -> String {
        let (e, w, i) = (
            self.count(Severity::Error),
            self.count(Severity::Warning),
            self.count(Severity::Info),
        );
        let mut out = format!(
            "{file}: {} page{}, {} paragraph{}: {e} error{}, {w} warning{}, {i} info\n",
            self.pages,
            plural(self.pages),
            self.paragraphs,
            plural(self.paragraphs),
            plural(e),
            plural(w)
        );
        for f in &self.findings {
            let at = match (f.para, f.page) {
                (Some(p), Some(pg)) => format!("¶{p} p.{pg}: "),
                (Some(p), None) => format!("¶{p}: "),
                (None, Some(pg)) => format!("p.{pg}: "),
                (None, None) => String::new(),
            };
            out.push_str(&format!(
                "  {:<8}{:<24}{at}{}\n",
                f.severity.name(),
                f.check,
                f.message
            ));
        }
        out
    }
}

struct Cx<'a> {
    doc: &'a Document,
    pages: Pages,
    out: Vec<Finding>,
}

impl Cx<'_> {
    fn add(
        &mut self,
        severity: Severity,
        check: &'static str,
        para: Option<usize>,
        message: String,
        details: Value,
    ) {
        debug_assert!(CHECKS.iter().any(|c| c.0 == check), "{check} isn't listed");
        let page = para.and_then(|p| self.pages.page(p));
        self.out.push(Finding {
            severity,
            check,
            para,
            page,
            message,
            details: match details {
                Value::Object(m) => m,
                _ => Map::new(),
            },
        });
    }
}

fn short(s: &str) -> String {
    let t: String = s.trim().chars().take(60).collect();
    if s.trim().chars().count() > 60 {
        format!("{t}…")
    } else {
        t
    }
}

/// Whether the paragraph or its style chain keeps it with the next one.
fn keeps_with_next(doc: &Document, p: &Paragraph) -> bool {
    let has = |e: &docs_engine::model::Extras| -> Option<bool> {
        e.iter()
            .find(|x| docs_engine::model::element_name(x) == "keepNext")
            .map(|x| !(x.contains("w:val=\"0\"") || x.contains("w:val=\"false\"")))
    };
    if let Some(v) = has(&p.props.extras) {
        return v;
    }
    let mut id = p.props.style.clone();
    for _ in 0..12 {
        let Some(s) = id.as_deref().and_then(|i| doc.styles.get(i)) else {
            return false;
        };
        if let Some(v) = has(&s.para.extras) {
            return v;
        }
        id = s.based_on.clone();
    }
    false
}

/// Bookmark names in the document.
fn bookmarks(doc: &Document) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for p in doc.paragraphs() {
        for (_, o) in p.objects() {
            if let Object::Opaque(op) = o
                && docs_engine::model::element_name(&op.xml) == "bookmarkStart"
                && let Some(el) = docs_engine::xml::parse(&op.xml)
                && let Some(n) = el.attr("name")
            {
                out.insert(n.to_owned());
            }
        }
    }
    out
}

/// Check a document.
pub fn check(doc: &Document) -> Report {
    let pages = Pages::of(doc);
    let mut cx = Cx {
        doc,
        pages,
        out: Vec::new(),
    };
    check_headings(&mut cx);
    check_formatting(&mut cx);
    check_fields(&mut cx);
    check_objects(&mut cx);
    check_layout(&mut cx);
    check_text(&mut cx);
    check_review(&mut cx);
    check_fonts_and_colours(&mut cx);
    let mut findings = cx.out;
    findings.sort_by_key(|f| (f.para.unwrap_or(usize::MAX), f.severity));
    Report {
        paragraphs: doc.len(),
        pages: cx.pages.count,
        findings,
    }
}

fn body_size(doc: &Document) -> f32 {
    doc.resolve_run(&Paragraph::default(), &RunProps::default())
        .size_pt
}

fn check_headings(cx: &mut Cx) {
    let doc = cx.doc;
    let mut prev: Option<u8> = None;
    let normal = body_size(doc);
    for i in 0..doc.len() {
        let p = doc.paragraph(i);
        if p.cell.is_some() {
            continue;
        }
        let text = shown(doc, i);
        match heading_level(doc, p) {
            Some(level) => {
                if text.trim().is_empty() {
                    cx.add(
                        Severity::Error,
                        "empty_heading",
                        Some(i),
                        format!(
                            "an empty {} (delete it or give it text)",
                            style_name(doc, p)
                        ),
                        json!({"level": level}),
                    );
                }
                if let Some(pl) = prev
                    && level > pl + 1
                {
                    cx.add(
                        Severity::Warning,
                        "heading_skip",
                        Some(i),
                        format!("\"{}\" is a level {level} heading after a level {pl} one (use Heading {})", short(&text), pl + 1),
                        json!({"level": level, "previous": pl}),
                    );
                }
                prev = Some(level);
            }
            None => {
                let style = style_name(doc, p).to_ascii_lowercase();
                let body_style = matches!(
                    style.as_str(),
                    "normal" | "body text" | "no spacing" | "list paragraph"
                ) || style.starts_with("body");
                let t = text.trim();
                if !body_style
                    || t.is_empty()
                    || t.chars().count() > 90
                    || t.ends_with(['.', ':', ',', ';'])
                    || p.props.num.is_some()
                {
                    continue;
                }
                let runs: Vec<_> = p
                    .runs
                    .iter()
                    .filter(|r| r.object.is_none() && r.len > 0)
                    .map(|r| doc.resolve_run(p, &r.props))
                    .collect();
                if runs.is_empty() {
                    continue;
                }
                let bold = runs.iter().all(|r| r.bold);
                let size = runs.iter().map(|r| r.size_pt).fold(0.0f32, f32::max);
                if (bold && t.split_whitespace().count() <= 12) || size >= normal + 3.0 {
                    cx.add(
                        Severity::Warning,
                        "fake_heading",
                        Some(i),
                        format!(
                            "\"{}\" looks like a heading ({}{:.0} pt) but is in style {}: give it a Heading style so it numbers, shows in the contents and navigates",
                            short(t),
                            if bold { "bold, " } else { "" },
                            size,
                            style_name(doc, p)
                        ),
                        json!({"bold": bold, "size_pt": size}),
                    );
                }
            }
        }
    }
}

fn check_formatting(cx: &mut Cx) {
    let doc = cx.doc;
    let mut paras = Vec::new();
    let mut runs = 0;
    let mut body = 0;
    for i in 0..doc.len() {
        let p = doc.paragraph(i);
        if !p.text.trim().is_empty() {
            body += 1;
        }
        let d = p
            .runs
            .iter()
            .filter(|r| r.object.is_none() && r.len > 0 && direct(r))
            .count();
        if d > 0 {
            runs += d;
            paras.push(i);
        }
    }
    if paras.is_empty() {
        return;
    }
    let many = paras.len() > 12.max(body / 6);
    cx.add(
        if many { Severity::Warning } else { Severity::Info },
        "direct_formatting",
        paras.first().copied(),
        format!(
            "{} run{} in {} paragraph{} formatted by hand{}: prefer styles (restyle changes them all at once)",
            runs,
            plural(runs),
            paras.len(),
            plural(paras.len()),
            if many { " (a lot)" } else { "" }
        ),
        json!({"runs": runs, "paragraphs": paras.iter().take(30).collect::<Vec<_>>()}),
    );
}

fn check_fields(cx: &mut Cx) {
    let doc = cx.doc;
    let marks = bookmarks(doc);
    let fields = doc.fields();
    let mut seq: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for f in &fields {
        let k = f.kind();
        let i = docs_engine::Instr::parse(&f.instr);
        let (a, b) = f.result();
        let result = doc.text_in(a, b).replace('\u{FFFC}', "");
        match k.as_str() {
            "REF" | "PAGEREF" | "NOTEREF" => {
                let target = i.args.first().cloned().unwrap_or_default();
                let missing = !marks.iter().any(|m| m.eq_ignore_ascii_case(&target));
                if missing
                    || result.contains("Error!")
                    || result.contains(docs_engine::fields::NO_TOC_ENTRIES)
                {
                    cx.add(
                        Severity::Error,
                        "broken_reference",
                        Some(f.begin.para),
                        format!(
                            "{k} to \"{target}\"{}: shows \"{}\"",
                            if missing {
                                ", which isn't a bookmark here"
                            } else {
                                ""
                            },
                            short(&result)
                        ),
                        json!({"field": f.instr.trim(), "bookmark": target}),
                    );
                }
            }
            "SEQ" => {
                let id = i.args.first().cloned().unwrap_or_default();
                seq.entry(id)
                    .or_default()
                    .push((f.begin.para, result.trim().to_owned()));
            }
            _ => {}
        }
    }
    for (id, list) in seq {
        for (expect, (para, shown_n)) in (1u32..).zip(list.iter()) {
            let n = shown_n.parse::<u32>().ok();
            if n != Some(expect) {
                cx.add(
                    Severity::Warning,
                    "caption_numbers",
                    Some(*para),
                    format!("{id} number shows \"{shown_n}\" where {expect} comes next: update fields (update_fields)"),
                    json!({"sequence": id, "shown": shown_n, "expected": expect}),
                );
                break;
            }
        }
    }
    // Tables of contents: rebuilt, do they read the same?
    let tocs: Vec<_> = fields
        .iter()
        .filter(|f| f.kind() == "TOC")
        .cloned()
        .collect();
    if tocs.is_empty() {
        return;
    }
    let mut copy = doc.clone();
    let mut engine = docs_layout::Engine::new();
    let ctx = docs_engine::FieldContext {
        now: chrono::Local::now().naive_local(),
        offset: chrono::Offset::fix(&chrono::Utc),
    };
    let sel = docs_engine::Selection::caret(docs_engine::Position::new(doc.len() - 1, 0));
    copy.update_tables_of_contents(sel, &ctx, &mut |d| docs_pdf::field_pages(d, &mut engine));
    let lines = |d: &Document, f: &docs_engine::FieldSpan| -> Vec<String> {
        (f.begin.para..=f.end.para.min(d.len() - 1))
            .map(|k| shown(d, k).trim().to_owned())
            .collect()
    };
    let after: Vec<_> = copy
        .fields()
        .into_iter()
        .filter(|f| f.kind() == "TOC")
        .collect();
    for (old, new) in tocs.iter().zip(&after) {
        let (a, b) = (lines(doc, old), lines(&copy, new));
        let (ra, rb) = old.result();
        let res = doc.text_in(ra, rb);
        if res.contains(docs_engine::fields::NO_TOC_ENTRIES) || res.trim().is_empty() {
            cx.add(
                Severity::Warning,
                "toc_stale",
                Some(old.begin.para),
                "the table of contents has no entries (no headings, or not updated)".into(),
                json!({}),
            );
        } else if a != b {
            let changed =
                a.iter().zip(&b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len());
            cx.add(
                Severity::Warning,
                "toc_stale",
                Some(old.begin.para),
                format!(
                    "the table of contents is out of date ({changed} line{} differ): update fields",
                    plural(changed)
                ),
                json!({"lines": a.len(), "expected": b.len()}),
            );
        }
    }
}

fn check_objects(cx: &mut Cx) {
    let doc = cx.doc;
    let n = doc.len();
    let is_caption =
        |k: usize| k < n && style_name(doc, doc.paragraph(k)).eq_ignore_ascii_case("caption");
    let mut seen_tables = BTreeSet::new();
    for i in 0..n {
        let p = doc.paragraph(i);
        for (_, o) in p.objects() {
            let (what, alt) = match o {
                Object::Image(img) => ("picture", img.descr.clone()),
                Object::Float(f) => match &f.kind {
                    FloatKind::Picture { .. } => ("picture", f.descr.clone()),
                    FloatKind::Chart { .. } => ("chart", f.descr.clone()),
                    _ => continue,
                },
                _ => continue,
            };
            if alt.trim().is_empty() {
                cx.add(
                    Severity::Warning,
                    "missing_alt_text",
                    Some(i),
                    format!("a {what} without alt text (screen readers say nothing)"),
                    json!({}),
                );
            }
            if p.cell.is_none()
                && !is_caption(i + 1)
                && !(i > 0 && is_caption(i - 1))
                && !matches!(o, Object::Float(f) if !f.inline)
            {
                cx.add(
                    Severity::Info,
                    "missing_caption",
                    Some(i),
                    format!("a {what} without a caption"),
                    json!({}),
                );
            }
        }
        if let Some(c) = p.cell
            && seen_tables.insert(c.table)
            && let Some(t) = doc.table(c.table)
            && t.rows.len() >= 2
            && !t.rows[0].header
        {
            cx.add(
                Severity::Warning,
                "table_header",
                Some(i),
                format!("a {}-row table without a header row (mark its first row to repeat as a header row)", t.rows.len()),
                json!({"rows": t.rows.len()}),
            );
        }
    }
}

fn check_layout(cx: &mut Cx) {
    let doc = cx.doc;
    let paged = &cx.pages.laid.paged;
    let mut found: Vec<(Severity, &'static str, usize, String, Value)> = Vec::new();
    let mut seen: BTreeSet<(usize, &'static str)> = BTreeSet::new();
    for (pi, page) in paged.pages.iter().enumerate() {
        let content_right = page.setup.width - page.setup.margin_right;
        // The column (or cell) right edge of each paragraph's lines.
        let mut right: HashMap<usize, f32> = HashMap::new();
        for l in &page.lines {
            let r = right.entry(l.para).or_insert(0.0);
            *r = r.max(l.x + l.width);
        }
        let mut ink: HashMap<usize, f32> = HashMap::new();
        for run in paged.glyph_runs(pi) {
            if !matches!(run.source, docs_layout::RunSource::Text) {
                continue;
            }
            let text = &doc.paragraph(run.para).text;
            let r = run
                .glyphs
                .iter()
                .filter(|g| {
                    !text
                        .get(g.cluster.clone())
                        .is_some_and(|s| s.trim().is_empty())
                })
                .map(|g| g.x + g.advance)
                .fold(0.0f32, f32::max);
            let e = ink.entry(run.para).or_insert(0.0);
            *e = e.max(r);
        }
        for (para, r) in ink {
            let Some(col) = right.get(&para).copied() else {
                continue;
            };
            let in_cell = doc.paragraph(para).cell.is_some();
            if r > col + 1.5
                && seen.insert((
                    para,
                    if in_cell {
                        "cell_overflow"
                    } else {
                        "past_margin"
                    },
                ))
            {
                let over = (r - col) * 0.75;
                if in_cell {
                    found.push((Severity::Error, "cell_overflow", para, format!("text runs {over:.0} pt past its table cell (a word or number too long to wrap: widen the column or shorten it)"), json!({"over_pt": over})));
                } else {
                    found.push((
                        Severity::Warning,
                        "past_margin",
                        para,
                        format!("text runs {over:.0} pt past the margin"),
                        json!({"over_pt": over}),
                    ));
                }
            } else if in_cell && col > content_right + 1.5 && seen.insert((para, "past_margin")) {
                let over = (col - content_right) * 0.75;
                found.push((
                    Severity::Warning,
                    "past_margin",
                    para,
                    format!("the table reaches {over:.0} pt past the right margin"),
                    json!({"over_pt": over}),
                ));
            }
        }
        for o in paged.objects(pi) {
            let [x, _, w, _] = o.rect;
            if x + w > content_right + 1.5 && seen.insert((o.para, "past_margin")) {
                let over = (x + w - content_right) * 0.75;
                found.push((
                    Severity::Warning,
                    "past_margin",
                    o.para,
                    format!("a picture reaches {over:.0} pt past the right margin"),
                    json!({"over_pt": over}),
                ));
            }
        }
    }
    // Headings alone at the bottom of a page.
    for i in 0..doc.len().saturating_sub(1) {
        let p = doc.paragraph(i);
        if p.cell.is_some() || heading_level(doc, p).is_none() || shown(doc, i).trim().is_empty() {
            continue;
        }
        let (Some(a), Some(b)) = (cx.pages.page(i), cx.pages.page(i + 1)) else {
            continue;
        };
        if b > a {
            let keeps = keeps_with_next(doc, p);
            found.push((
                if keeps { Severity::Info } else { Severity::Warning },
                "heading_at_page_bottom",
                i,
                if keeps {
                    format!("\"{}\" ends page {a} here; Word keeps it with the next paragraph and moves it to page {b}, so Word's later pages can differ", short(&shown(doc, i)))
                } else {
                    format!("\"{}\" is alone at the bottom of page {a} (turn on keep with next, or add a page break before it)", short(&shown(doc, i)))
                },
                json!({}),
            ));
        }
    }
    // Tables that start at the bottom of a page: the page each row starts
    // on (its first paragraph's), per table.
    let mut rows: BTreeMap<usize, BTreeMap<usize, (usize, usize)>> = BTreeMap::new();
    for i in 0..doc.len() {
        let Some(c) = doc.paragraph(i).cell else {
            continue;
        };
        let Some(page) = cx.pages.page(i) else {
            continue;
        };
        let e = rows
            .entry(c.table as usize)
            .or_default()
            .entry(c.row as usize)
            .or_insert((page, i));
        e.0 = e.0.min(page);
    }
    for (table, pages) in rows {
        let Some(t) = doc.table(table as _) else {
            continue;
        };
        let heads = t.rows.iter().take_while(|r| r.header).count();
        let page_of = |r: usize| pages.get(&r).map(|x| x.0);
        let Some((start, first_para)) = pages.values().next().copied() else {
            continue;
        };
        let body: Vec<usize> = (heads..t.rows.len()).filter_map(page_of).collect();
        let Some(&last) = body.last() else { continue };
        if last <= start {
            continue;
        }
        let on_first = body.iter().filter(|&&p| p == start).count();
        if heads > 0 && on_first == 0 {
            found.push((
                Severity::Warning,
                "table_at_page_bottom",
                first_para,
                format!(
                    "the table's header row is alone at the bottom of page {start}, its rows start on page {} (keep the header row with the next row: turn on keep with next in it, or start the table on the next page)",
                    body[0]
                ),
                json!({"body_rows_on_first_page": 0}),
            ));
        } else if on_first == 1 && body.len() > 2 {
            found.push((
                Severity::Info,
                "table_at_page_bottom",
                first_para,
                format!("the table starts at the bottom of page {start} with one row, the rest on the next page"),
                json!({"body_rows_on_first_page": 1}),
            ));
        }
    }
    for (s, c, p, m, d) in found {
        cx.add(s, c, Some(p), m, d);
    }
}

/// Placeholder words and patterns left from templates.
fn placeholder(t: &str) -> Option<String> {
    let lower = t.to_lowercase();
    if lower.contains("lorem ipsum") {
        return Some("Lorem ipsum".into());
    }
    for w in ["TBD", "TBC", "TODO", "FIXME", "XXX", "XX"] {
        let hit = t.match_indices(w).any(|(i, _)| {
            let before = t[..i].chars().next_back();
            let after = t[i + w.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric())
                && !after.is_some_and(|c| c.is_alphanumeric())
        });
        if hit {
            return Some(w.into());
        }
    }
    for (open, close) in [("[", "]"), ("<", ">"), ("{{", "}}")] {
        let mut rest = t;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + open.len()..];
            let Some(j) = after.find(close) else { break };
            let inner = &after[..j];
            let il = inner.to_lowercase();
            // Whole words only ("[update]" isn't "[date]").
            let words: Vec<&str> = il
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            // Code written as text: `[section]`, `<tag>`.
            let in_code = rest[..i].ends_with('`') && after[j + close.len()..].starts_with('`');
            const WORDS: &[&str] = &[
                "name",
                "insert",
                "date",
                "client",
                "company",
                "title",
                "address",
                "amount",
                "number",
                "nama",
                "tanggal",
                "alamat",
                "perusahaan",
                "placeholder",
                "your",
                "add",
                "tbd",
                "xx",
            ];
            if !in_code
                && (open == "{{"
                    || (inner.chars().count() <= 40
                        && !inner.is_empty()
                        && WORDS.iter().any(|w| words.contains(w))
                        && !il.contains("://")))
            {
                return Some(format!("{open}{inner}{close}"));
            }
            rest = &after[j..];
        }
    }
    None
}

/// Whether a run is code: the Code character style (or one named like
/// it), a code paragraph style, or a monospaced font.
fn is_code(doc: &Document, p: &Paragraph, props: &RunProps) -> bool {
    let named = |id: Option<&str>| {
        id.is_some_and(|id| {
            let name = doc
                .styles
                .get(id)
                .map_or(id.to_owned(), |s| s.name.clone())
                .to_ascii_lowercase();
            name.contains("code") || name.contains("verbatim") || id.eq_ignore_ascii_case("code")
        })
    };
    if named(props.style.as_deref()) || named(p.props.style.as_deref()) {
        return true;
    }
    let font = doc.resolve_run(p, props).font.to_ascii_lowercase();
    [
        "mono", "consolas", "courier", "menlo", "monaco", "cascadia", "code",
    ]
    .iter()
    .any(|m| font.contains(m))
}

/// The text of paragraph `i` as shown, code runs blanked out (template
/// placeholders aren't looked for in code).
fn prose(doc: &Document, i: usize) -> String {
    let p = doc.paragraph(i);
    let mut out = String::with_capacity(p.text.len());
    for (range, run) in p.run_ranges() {
        if run
            .rev
            .as_ref()
            .is_some_and(|r| r.kind == docs_engine::RevKind::Delete)
        {
            continue;
        }
        let code = is_code(doc, p, &run.props);
        for c in p.text[range].chars() {
            out.push(if code || c == docs_engine::OBJECT {
                ' '
            } else if c == docs_engine::LINE_BREAK {
                '\n'
            } else {
                c
            });
        }
    }
    out
}

fn check_text(cx: &mut Cx) {
    let doc = cx.doc;
    let mut doubles = Vec::new();
    for i in 0..doc.len() {
        let t = shown(doc, i);
        if let Some(p) = placeholder(&prose(doc, i)) {
            cx.add(
                Severity::Error,
                "placeholder_text",
                Some(i),
                format!("placeholder text left in: \"{p}\""),
                json!({"text": p}),
            );
        }
        let inner = t.trim();
        if inner.contains("  ") && !inner.contains('\t') {
            doubles.push(i);
        }
    }
    if let Some(&first) = doubles.first() {
        cx.add(
            Severity::Info,
            "double_space",
            Some(first),
            format!(
                "two spaces in a row in {} paragraph{} (find_replace \"  \" with \" \")",
                doubles.len(),
                plural(doubles.len())
            ),
            json!({"paragraphs": doubles.iter().take(30).collect::<Vec<_>>()}),
        );
    }
}

fn check_review(cx: &mut Cx) {
    let doc = cx.doc;
    let anchors = doc.comment_anchors();
    for c in doc
        .comments()
        .iter()
        .filter(|c| !c.done && c.parent.is_none())
    {
        let at = anchors
            .iter()
            .find(|(id, _)| *id == c.id)
            .map(|(_, a)| a.start.para);
        cx.add(
            Severity::Warning,
            "open_comments",
            at,
            format!("an open comment by {}: \"{}\"", c.author, short(&c.text)),
            json!({"id": c.id}),
        );
    }
    let changes = doc.changes();
    if let Some(first) = changes.first() {
        let authors: BTreeSet<&str> = changes.iter().map(|c| c.author.as_str()).collect();
        cx.add(
            Severity::Warning,
            "tracked_changes",
            Some(first.start.para),
            format!(
                "{} tracked change{} by {} not accepted or rejected",
                changes.len(),
                plural(changes.len()),
                authors.into_iter().collect::<Vec<_>>().join(", ")
            ),
            json!({"changes": changes.len()}),
        );
    }
}

/// Whether a font family is installed (or names a style of one).
fn installed(engine: &mut docs_layout::Engine, family: &str) -> bool {
    engine.fonts.collection.family_id(family).is_some()
        || engine.style_family(&format!("\"{family}\"")).is_some()
}

fn check_fonts_and_colours(cx: &mut Cx) {
    let doc = cx.doc;
    let mut used: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut body_fonts: BTreeMap<String, usize> = BTreeMap::new();
    let mut contrast_hits = 0;
    for i in 0..doc.len() {
        let p = doc.paragraph(i);
        let heading = heading_level(doc, p).is_some();
        let style = style_name(doc, p).to_ascii_lowercase();
        let body = !heading
            && !matches!(style.as_str(), "title" | "subtitle")
            && !style.starts_with("toc")
            && !style.contains("code");
        let bg = p
            .cell
            .and_then(|c| {
                let t = doc.table(c.table)?;
                let cell = t.rows.get(c.row as usize)?.cells.get(c.cell as usize)?;
                docs_pdf::build::cell_shading(cell.tc_pr.as_deref())
            })
            .map(|c| [c.0[0], c.0[1], c.0[2]])
            .unwrap_or([255, 255, 255]);
        let mut at = 0;
        for r in &p.runs {
            let range = at..at + r.len;
            at = range.end;
            if r.object.is_some() || r.len == 0 || p.text[range.clone()].trim().is_empty() {
                continue;
            }
            let rr = doc.resolve_run(p, &r.props);
            let e = used.entry(rr.font.clone()).or_insert((i, 0));
            e.1 += range.len();
            if body && p.cell.is_none() {
                *body_fonts.entry(rr.font.clone()).or_default() += range.len();
            }
            let c = contrast(rr.color.0, bg);
            let large = rr.size_pt >= 18.0 || (rr.bold && rr.size_pt >= 14.0);
            let need = if large { 3.0 } else { 4.5 };
            if c < need && contrast_hits < 20 {
                contrast_hits += 1;
                cx.add(
                    if c < 2.0 { Severity::Error } else { Severity::Warning },
                    "low_contrast",
                    Some(i),
                    format!(
                        "#{} text on #{:02X}{:02X}{:02X} reads at {c:.1}:1 (needs {need}:1): \"{}\"",
                        rr.color.hex(),
                        bg[0],
                        bg[1],
                        bg[2],
                        short(&p.text[range.clone()])
                    ),
                    json!({"ratio": (c * 100.0).round() / 100.0, "color": rr.color.hex()}),
                );
            }
        }
    }
    let total: usize = body_fonts.values().sum();
    let main = body_fonts.values().copied().max().unwrap_or(0);
    if body_fonts.len() > 1 && total > 0 && (main as f64) < total as f64 * 0.97 {
        let list: Vec<String> = body_fonts
            .iter()
            .map(|(f, n)| format!("{f} {}%", (*n as f64 * 100.0 / total as f64).round()))
            .collect();
        cx.add(
            Severity::Warning,
            "mixed_fonts",
            None,
            format!(
                "body text is in {} fonts ({}): set the font in the styles instead",
                body_fonts.len(),
                list.join(", ")
            ),
            json!({"fonts": body_fonts}),
        );
    }
    let mut engine = docs_layout::Engine::new();
    for (font, (first, _)) in used {
        if font.is_empty() || installed(&mut engine, &font) {
            continue;
        }
        let sub =
            kantor_core::fonts::metric_substitute(&font).filter(|s| installed(&mut engine, s));
        match sub {
            Some(s) => cx.add(
                Severity::Info,
                "font_missing",
                Some(first),
                format!("{font} isn't installed here: drawn with {s}, which has the same metrics"),
                json!({"font": font, "substitute": s}),
            ),
            None => cx.add(
                Severity::Warning,
                "font_missing",
                Some(first),
                format!("{font} isn't installed here: Kantor measures it with a fallback, so line breaks and page breaks can differ where it is installed"),
                json!({"font": font}),
            ),
        }
    }
}

/// The checks as a markdown table (docs/docs-json.md keeps a copy).
#[cfg(test)]
pub fn checks_table() -> String {
    let mut s = String::from("| check | severity | what |\n|---|---|---|\n");
    for (c, sev, what) in CHECKS {
        s.push_str(&format!("| `{c}` | {sev} | {what} |\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc_of(v: Value) -> Document {
        let b = crate::agent::docs_author::build(&v, None).unwrap();
        Document::from_read(docs_engine::docx::read(b.bytes).unwrap(), None)
    }

    fn checks(r: &Report) -> BTreeSet<&'static str> {
        r.findings.iter().map(|f| f.check).collect()
    }

    #[std::prelude::v1::test]
    fn a_clean_build_has_no_errors() {
        let d = doc_of(json!({
            "properties": {"title": "Clean"},
            "toc": true,
            "blocks": [{"h1": "One", "id": "one"}, "Text about {ref:one}.", {"h2": "Sub"}, "More text.",
                       {"type": "table", "header": ["A", "B"], "rows": [["1", "2"]], "caption": "T"}]
        }));
        let r = check(&d);
        assert!(!r.has_errors(), "{}", r.text("clean"));
        let c = checks(&r);
        assert!(!c.contains("toc_stale"), "{}", r.text("clean"));
        assert!(!c.contains("table_header"));
        assert!(!c.contains("heading_skip"));
    }

    #[std::prelude::v1::test]
    fn finds_problems() {
        let d = doc_of(json!({
            "markdown": false,
            "blocks": [
                {"h1": "Start"}, {"h3": "Too deep"}, {"type": "heading", "level": 2, "text": ""},
                {"text": "Hand made", "bold": true, "size": 16},
                "Dear [Client Name], this is TBD.  Two spaces.",
                {"text": "Pale", "color": "#EEEEEE"},
                {"type": "table", "rows": [["a", "b"], ["c", "d"]]},
                {"type": "table", "header": ["x"], "rows": [["Supercalifragilisticexpialidocious_and_more_and_more_and_more_and_more_unbreakable"]], "columns": ["1cm"], "width": "1cm"},
                {"text": "Odd font", "font": "Kantor Missing Font"}
            ]
        }));
        let r = check(&d);
        let c = checks(&r);
        for want in [
            "heading_skip",
            "empty_heading",
            "fake_heading",
            "placeholder_text",
            "double_space",
            "low_contrast",
            "table_header",
            "direct_formatting",
            "font_missing",
            "cell_overflow",
        ] {
            assert!(c.contains(want), "{want} missing:\n{}", r.text("x"));
        }
        assert!(r.has_errors());
        let j = r.json();
        assert!(j["errors"].as_u64().unwrap() >= 3);
        assert!(j["findings"][0]["paragraph"].is_number());
        assert!(checks_table().contains("`cell_overflow`"));
    }

    #[std::prelude::v1::test]
    fn placeholders() {
        assert_eq!(
            placeholder("Lorem ipsum dolor").as_deref(),
            Some("Lorem ipsum")
        );
        assert_eq!(placeholder("Due date TBD.").as_deref(), Some("TBD"));
        assert_eq!(placeholder("Dear [Name],").as_deref(), Some("[Name]"));
        assert_eq!(placeholder("Hi {{First}}").as_deref(), Some("{{First}}"));
        assert!(placeholder("XXL shirts and OXXO stores, see [1] and [Appendix A]").is_none());
        // Whole words: a TOML section isn't a [date] placeholder.
        assert!(placeholder("the [update] section").is_none());
        assert!(placeholder("set `[name]` in the file").is_none());
        assert_eq!(placeholder("Kepada [nama], ").as_deref(), Some("[nama]"));
        assert_eq!(
            placeholder("For [Client name] only").as_deref(),
            Some("[Client name]")
        );
        assert_eq!(placeholder("[Due date]").as_deref(), Some("[Due date]"));
        assert_eq!(placeholder("ref XXX here").as_deref(), Some("XXX"));
        assert_eq!(
            placeholder("[company_name]").as_deref(),
            Some("[company_name]")
        );
    }

    #[std::prelude::v1::test]
    fn tables_starting_at_a_page_bottom() {
        // Filler lines move the table down a line at a time: for some
        // count its header row would end page 1 alone. Built, the header
        // row keeps with the first row; without that, check finds it.
        // Small filler lines (a step shorter than a table row) so one
        // count lands the table in the gap.
        let build = |n: usize| {
            let mut blocks: Vec<Value> = (0..n).map(|k| json!(format!("Line {k}."))).collect();
            blocks.push(json!({"type": "table", "header": ["A", "B"],
                               "rows": [["1", "2"], ["3", "4"], ["5", "6"]]}));
            let doc = json!({
                "styles": {"Normal": {"size": 6, "space_after": 0, "line_spacing": 1.0}},
                "blocks": blocks
            });
            crate::agent::docs_author::build(&doc, None).unwrap()
        };
        // The table reaches page 2 somewhere after `lo` lines.
        let mut lo = 40;
        while build(lo + 20).pages < 2 {
            lo += 20;
            assert!(lo < 400);
        }
        let mut found = false;
        for n in lo..lo + 40 {
            let b = build(n);
            let warned = |bytes: Vec<u8>| {
                let d = Document::from_read(docs_engine::docx::read(bytes).unwrap(), None);
                check(&d)
                    .findings
                    .iter()
                    .any(|f| f.check == "table_at_page_bottom" && f.severity == Severity::Warning)
            };
            assert!(
                !warned(b.bytes.clone()),
                "{n} lines: the header row moves on"
            );
            let mut pkg = crate::agent::docs_pkg::Pkg::read(&b.bytes).unwrap();
            let x = pkg.text("word/document.xml").unwrap();
            assert!(x.contains("<w:keepNext/>"));
            pkg.set_text("word/document.xml", x.replace("<w:keepNext/>", ""));
            if warned(pkg.to_bytes().unwrap()) {
                found = true;
                break;
            }
        }
        assert!(found, "some line count leaves the header row alone");
    }

    #[std::prelude::v1::test]
    fn no_placeholders_in_code() {
        let r = check(&doc_of(json!({"blocks": [
            {"h1": "Setup"},
            "Edit the `[update]` section and the `[sheets]` one, then `TODO.md` and `{{ name }}`.",
            {"text": [{"text": "Run "}, {"text": "[name] = TBD", "code": true}, {"text": " once."}]},
            {"type": "table", "header": ["Key", "Value"], "rows": [["`[date]`", "the build date"]]}
        ]})));
        assert!(
            !checks(&r).contains("placeholder_text"),
            "{}",
            r.text("code")
        );
        for text in ["Kepada [nama],", "Due TBD.", "Hi {{field}}", "Ref XXX."] {
            let r = check(&doc_of(json!({"blocks": [{"h1": "A"}, text]})));
            assert!(checks(&r).contains("placeholder_text"), "{text}");
        }
    }
}
