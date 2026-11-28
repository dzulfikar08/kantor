//! A folder of notes ("vault"): the link index behind wiki links,
//! backlinks, renames, search, `[[` completion and the graph.
//!
//! The index keeps each note's text and the links found in it by
//! [`crate::parse`] (wiki links `[[Note]]`, `[[Note|alias]]`,
//! `[[Note#Heading]]`, embeds `![[Note]]`, and Markdown links to `.md`
//! files). Names resolve as Obsidian does: by file name without the
//! extension, case-insensitively; a name with a `/` matches the end of a
//! path; among several notes of one name, the one in the linking note's
//! folder wins, then the shortest path.
//!
//! Renames are plans of byte splices ([`Splice`]) over each linking file:
//! only the bytes of the link's target change, everything else in the file
//! stays as it was.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::buffer::Buffer;
use crate::parse::{self, LinkKind};

/// The parts of a `[[wiki link]]`, as byte ranges in the note's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiParts {
    /// The note name (`Folder/Note` in `[[Folder/Note#H|alias]]`), trimmed.
    pub name: Range<usize>,
    /// After `#`.
    pub heading: Option<Range<usize>>,
    /// After `|`.
    pub alias: Option<Range<usize>>,
    /// `![[…]]`.
    pub embed: bool,
}

/// Split the wiki link whose source is `text[link]` (`[[…]]`, `![[…]]`).
pub fn wiki_parts(text: &str, link: Range<usize>) -> Option<WikiParts> {
    let s = text.get(link.clone())?;
    let open = s.find("[[")?;
    let close = s.rfind("]]")?;
    if close < open + 2 {
        return None;
    }
    let embed = s[..open].ends_with('!');
    let a = link.start + open + 2;
    let b = link.start + close;
    let inner = &text[a..b];
    let (target, alias) = match inner.find('|') {
        Some(i) => {
            // `\|` in a table cell: the backslash isn't part of the name.
            let end = if inner[..i].ends_with('\\') {
                a + i - 1
            } else {
                a + i
            };
            (a..end, Some(a + i + 1..b))
        }
        None => (a..b, None),
    };
    let t = &text[target.clone()];
    let (name, heading) = match t.find('#') {
        Some(i) => (
            target.start..target.start + i,
            Some(target.start + i + 1..target.end),
        ),
        None => (target, None),
    };
    Some(WikiParts {
        name: trim(text, name),
        heading,
        alias,
        embed,
    })
}

fn trim(text: &str, r: Range<usize>) -> Range<usize> {
    let s = &text[r.clone()];
    let lead = s.len() - s.trim_start().len();
    let tail = s.len() - s.trim_end().len();
    if lead + tail >= s.len() {
        return r.start..r.start;
    }
    r.start + lead..r.end - tail
}

/// How a name compares: lower case, `/` separators, no `.md`.
pub fn name_key(name: &str) -> String {
    let n = name.trim().replace('\\', "/");
    let n = n.trim_start_matches("./");
    let lower = n.to_lowercase();
    let stripped = lower
        .strip_suffix(".md")
        .or_else(|| lower.strip_suffix(".markdown"))
        .unwrap_or(&lower);
    stripped.to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutKind {
    Wiki,
    Embed,
    /// `[text](Other%20Note.md)`.
    Markdown,
}

/// A link from a note to (maybe) another note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutLink {
    pub kind: OutKind,
    /// The target as written (wiki: the name; Markdown: the decoded path,
    /// relative to the linking note's folder).
    pub target: String,
    pub heading: Option<String>,
    /// The whole link.
    pub range: Range<usize>,
    /// The bytes a rename rewrites (the name, or the Markdown link's path).
    pub name: Range<usize>,
    pub line: usize,
}

/// A heading of a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteHeading {
    pub level: u8,
    pub line: usize,
    pub text: String,
}

/// The links and headings in a note's text.
pub fn scan(text: &str) -> (Vec<OutLink>, Vec<NoteHeading>) {
    let buf = Buffer::new(text);
    let parsed = parse::parse(&buf);
    let mut links = Vec::new();
    for l in &parsed.links {
        match l.kind {
            LinkKind::Wiki => {
                if let Some(p) = wiki_parts(text, l.range.clone()) {
                    links.push(OutLink {
                        kind: if p.embed {
                            OutKind::Embed
                        } else {
                            OutKind::Wiki
                        },
                        target: text[p.name.clone()].to_owned(),
                        heading: p.heading.map(|h| text[h].trim().to_owned()),
                        range: l.range.clone(),
                        name: p.name,
                        line: buf.line_of(l.range.start),
                    });
                }
            }
            LinkKind::Url => {
                if let Some(o) = markdown_note_link(text, l.range.clone(), &l.dest) {
                    links.push(OutLink {
                        line: buf.line_of(l.range.start),
                        ..o
                    });
                }
            }
        }
    }
    // `![[Note]]` embeds come out of the parser as images.
    for im in &parsed.images {
        if text
            .get(im.range.clone())
            .is_some_and(|s| s.starts_with("![["))
            && let Some(p) = wiki_parts(text, im.range.clone())
        {
            links.push(OutLink {
                kind: OutKind::Embed,
                target: text[p.name.clone()].to_owned(),
                heading: p.heading.map(|h| text[h].trim().to_owned()),
                range: im.range.clone(),
                name: p.name,
                line: buf.line_of(im.range.start),
            });
        }
    }
    links.sort_by_key(|l| l.range.start);
    let headings = parsed
        .headings
        .iter()
        .map(|h| NoteHeading {
            level: h.level,
            line: h.line,
            text: h.text.trim().to_owned(),
        })
        .collect();
    (links, headings)
}

/// A Markdown link to a local `.md` file: where its path is in the source.
fn markdown_note_link(text: &str, range: Range<usize>, dest: &str) -> Option<OutLink> {
    let dest = dest.trim();
    if dest.is_empty() || dest.contains("://") || dest.starts_with("mailto:") {
        return None;
    }
    let path_part = dest.split('#').next().unwrap_or("");
    let lower = path_part.to_ascii_lowercase();
    if !(lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".md>")) {
        return None;
    }
    let src = text.get(range.clone())?;
    // The destination follows the last `](`.
    let open = src.rfind("](")? + 2;
    let rest = &src[open..];
    let (start, raw) = if let Some(r) = rest.strip_prefix('<') {
        (open + 1, &r[..r.find('>')?])
    } else {
        let end = rest
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(rest.len());
        (open, &rest[..end])
    };
    let raw_path = raw.split('#').next().unwrap_or("");
    if raw_path.is_empty() {
        return None;
    }
    let a = range.start + start;
    Some(OutLink {
        kind: OutKind::Markdown,
        target: percent_decode(raw_path),
        heading: dest.split_once('#').map(|(_, h)| h.to_owned()),
        range,
        name: a..a + raw_path.len(),
        line: 0,
    })
}

/// `%20` and friends.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// One note of a vault.
#[derive(Clone, Debug)]
pub struct Note {
    /// Path relative to the vault, `/`-separated, with its extension.
    pub rel: String,
    /// The text (no byte-order mark).
    pub text: String,
    /// The file starts with a byte-order mark (disk offsets are 3 more).
    pub bom: bool,
    pub links: Vec<OutLink>,
    pub headings: Vec<NoteHeading>,
}

impl Note {
    pub fn new(rel: &str, text: &str, bom: bool) -> Self {
        let (links, headings) = scan(text);
        Self {
            rel: rel.replace('\\', "/"),
            text: text.to_owned(),
            bom,
            links,
            headings,
        }
    }

    /// The file name without its extension (the note's name in links).
    pub fn stem(&self) -> &str {
        stem_of(&self.rel)
    }

    /// The folder part of `rel` (`""` at the root).
    pub fn folder(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(f, _)| f)
    }

    /// The line `line` of the text.
    pub fn line_text(&self, line: usize) -> &str {
        self.text
            .split('\n')
            .nth(line)
            .unwrap_or("")
            .trim_end_matches('\r')
    }
}

/// `a/b/Note.md` → `Note`.
pub fn stem_of(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// A link into a note, for the backlinks panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// Index of the note it is in.
    pub note: usize,
    pub line: usize,
    /// Byte range in that note's text.
    pub range: Range<usize>,
}

/// A search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub note: usize,
    pub line: usize,
    /// Byte range in the note's text.
    pub range: Range<usize>,
}

/// One `[[` completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// What the list shows.
    pub label: String,
    /// Where it lives (the note's path), for a second line.
    pub detail: String,
    /// What replaces the query (`]]` is added by the editor).
    pub insert: String,
}

/// A rename's change to one file: byte splices in its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// Apply `splices` (not overlapping, any order) to `text`.
pub fn apply_splices(text: &str, splices: &[Splice]) -> String {
    let mut sorted: Vec<&Splice> = splices.iter().collect();
    sorted.sort_by_key(|s| s.range.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for s in sorted {
        out.push_str(&text[at..s.range.start]);
        out.push_str(&s.text);
        at = s.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The splices that undo `splices` once applied (ranges in the new text).
pub fn inverse_splices(text: &str, splices: &[Splice]) -> Vec<Splice> {
    let mut sorted: Vec<&Splice> = splices.iter().collect();
    sorted.sort_by_key(|s| s.range.start);
    let mut shift: isize = 0;
    let mut out = Vec::with_capacity(sorted.len());
    for s in sorted {
        let start = (s.range.start as isize + shift) as usize;
        out.push(Splice {
            range: start..start + s.text.len(),
            text: text[s.range.clone()].to_owned(),
        });
        shift += s.text.len() as isize - s.range.len() as isize;
    }
    out
}

/// Where an offset in the old text lands after `splices` (an offset inside
/// a replaced range goes to the end of its replacement).
pub fn map_offset(offset: usize, splices: &[Splice]) -> usize {
    let mut shift: isize = 0;
    for s in splices {
        if s.range.end <= offset {
            shift += s.text.len() as isize - s.range.len() as isize;
        } else if s.range.start < offset {
            return (s.range.start as isize + shift) as usize + s.text.len();
        }
    }
    (offset as isize + shift).max(0) as usize
}

/// The folders a vault skips: hidden ones (`.obsidian`, `.git`, `.trash`)
/// and dependency folders.
fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules" || name == "target"
}

/// Notes bigger than this aren't indexed (a log file named `.md`).
pub const MAX_NOTE_BYTES: u64 = 8 << 20;

/// The notes of a folder.
#[derive(Clone, Debug, Default)]
pub struct Vault {
    pub root: PathBuf,
    pub notes: Vec<Note>,
    /// `name_key(stem)` → notes with that name.
    by_stem: HashMap<String, Vec<usize>>,
}

impl Vault {
    /// An index of `notes` (`(rel, text)`), for tests and in-memory vaults.
    pub fn from_notes(root: &Path, notes: Vec<(String, String)>) -> Self {
        let mut v = Self {
            root: root.to_path_buf(),
            notes: notes
                .into_iter()
                .map(|(rel, text)| Note::new(&rel, &text, false))
                .collect(),
            by_stem: HashMap::new(),
        };
        v.sort_and_reindex();
        v
    }

    /// Read every note under `root` (hidden folders skipped).
    pub fn scan(root: &Path) -> Self {
        let mut notes = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !skipped_dir(&name) {
                        stack.push(p);
                    }
                    continue;
                }
                if !crate::is_note_path(&p) || e.metadata().is_ok_and(|m| m.len() > MAX_NOTE_BYTES)
                {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else {
                    continue;
                };
                let Ok(buf) = Buffer::from_bytes(&bytes) else {
                    continue;
                };
                let Some(rel) = rel_path(root, &p) else {
                    continue;
                };
                notes.push(Note::new(&rel, buf.text(), buf.has_bom()));
            }
        }
        let mut v = Self {
            root: root.to_path_buf(),
            notes,
            by_stem: HashMap::new(),
        };
        v.sort_and_reindex();
        v
    }

    fn sort_and_reindex(&mut self) {
        self.notes.sort_by_key(|a| a.rel.to_lowercase());
        self.by_stem.clear();
        for (i, n) in self.notes.iter().enumerate() {
            self.by_stem.entry(name_key(n.stem())).or_default().push(i);
        }
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// The note at `rel`.
    pub fn find(&self, rel: &str) -> Option<usize> {
        let rel = rel.replace('\\', "/");
        self.notes.iter().position(|n| n.rel == rel)
    }

    /// The note at absolute `path` (inside the vault).
    pub fn find_path(&self, path: &Path) -> Option<usize> {
        self.find(&rel_path(&self.root, path)?)
    }

    pub fn path_of(&self, note: usize) -> PathBuf {
        self.root.join(&self.notes[note].rel)
    }

    /// Add or replace the note at `rel` with `text`.
    pub fn upsert(&mut self, rel: &str, text: &str, bom: bool) -> usize {
        let rel = rel.replace('\\', "/");
        match self.find(&rel) {
            Some(i) => {
                self.notes[i] = Note::new(&rel, text, bom);
                i
            }
            None => {
                self.notes.push(Note::new(&rel, text, bom));
                self.sort_and_reindex();
                self.find(&rel).unwrap_or(0)
            }
        }
    }

    pub fn remove(&mut self, rel: &str) {
        if let Some(i) = self.find(rel) {
            self.notes.remove(i);
            self.sort_and_reindex();
        }
    }

    /// Move the note at `from` to `to` (its text unchanged).
    pub fn move_note(&mut self, from: &str, to: &str) {
        if let Some(i) = self.find(from) {
            self.notes[i].rel = to.replace('\\', "/");
            self.sort_and_reindex();
        }
    }

    /// The note a link to `target` (written in the note at `from_rel`)
    /// goes to.
    pub fn resolve(&self, from_rel: Option<&str>, target: &str) -> Option<usize> {
        let key = name_key(target);
        if key.is_empty() {
            return from_rel.and_then(|r| self.find(r));
        }
        let from_folder = from_rel
            .map(|r| r.rsplit_once('/').map_or("", |(f, _)| f).to_lowercase())
            .unwrap_or_default();
        let pick = |cands: &mut dyn Iterator<Item = usize>| -> Option<usize> {
            cands.min_by_key(|&i| {
                let n = &self.notes[i];
                let same = n.folder().to_lowercase() == from_folder;
                (!same, n.rel.matches('/').count(), n.rel.len())
            })
        };
        if key.contains('/') {
            // A path: from the vault's root, or the end of a path.
            let exact = self.notes.iter().position(|n| name_key(&n.rel) == key);
            if exact.is_some() {
                return exact;
            }
            let suffix = format!("/{key}");
            return pick(
                &mut (0..self.notes.len())
                    .filter(|&i| name_key(&self.notes[i].rel).ends_with(&suffix)),
            );
        }
        pick(&mut self.by_stem.get(&key)?.iter().copied())
    }

    /// Where a Markdown link's path (relative to the note at `from_rel`)
    /// points, as a vault path.
    pub fn markdown_target(from_rel: &str, target: &str) -> String {
        let folder = from_rel.rsplit_once('/').map_or("", |(f, _)| f);
        let mut parts: Vec<&str> = if target.starts_with('/') {
            Vec::new()
        } else {
            folder.split('/').filter(|s| !s.is_empty()).collect()
        };
        for seg in target.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        parts.join("/")
    }

    /// The note link `l` of note `from` goes to.
    pub fn target_of(&self, from: usize, l: &OutLink) -> Option<usize> {
        let rel = &self.notes[from].rel;
        match l.kind {
            OutKind::Markdown => self.find(&Self::markdown_target(rel, &l.target)),
            _ => self.resolve(Some(rel), &l.target),
        }
    }

    /// Links into note `to` from every note (itself included), in vault
    /// order.
    pub fn backlinks(&self, to: usize) -> Vec<Mention> {
        let mut out = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            for l in &n.links {
                if self.target_of(i, l) == Some(to) && !(i == to && l.target.is_empty()) {
                    out.push(Mention {
                        note: i,
                        line: l.line,
                        range: l.range.clone(),
                    });
                }
            }
        }
        out
    }

    /// Where other notes name note `to` without linking it: the name as a
    /// whole word (any case), outside links and code.
    pub fn unlinked_mentions(&self, to: usize, limit: usize) -> Vec<Mention> {
        let name = self.notes[to].stem().to_lowercase();
        if name.chars().count() < 3 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            if i == to {
                continue;
            }
            let lower = n.text.to_lowercase();
            // Lower-casing can change byte lengths; only trust offsets
            // where it didn't.
            if lower.len() != n.text.len() {
                continue;
            }
            let mut skip: Vec<Range<usize>> = n.links.iter().map(|l| l.range.clone()).collect();
            skip.extend(code_ranges(&n.text));
            for (at, m) in lower.match_indices(&name) {
                let r = at..at + m.len();
                if !whole_word(&lower, &r)
                    || skip.iter().any(|s| s.start < r.end && r.start < s.end)
                {
                    continue;
                }
                out.push(Mention {
                    note: i,
                    line: n.text[..at].matches('\n').count(),
                    range: r,
                });
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    /// Lines of any note holding every word of `query` (any case); the
    /// first word's place is the hit. File names match too (line 0).
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            let mut start = 0;
            for (line, raw) in n.text.split('\n').enumerate() {
                let lower = raw.to_lowercase();
                if lower.len() == raw.len() && words.iter().all(|w| lower.contains(w.as_str())) {
                    let at = lower.find(words[0].as_str()).unwrap_or(0);
                    out.push(Hit {
                        note: i,
                        line,
                        range: start + at..start + at + words[0].len(),
                    });
                    if out.len() >= limit {
                        return out;
                    }
                }
                start += raw.len() + 1;
            }
        }
        out
    }

    /// Notes whose path holds every word of `query` (for the file list's
    /// filter and the search's "files" part).
    pub fn search_names(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        (0..self.notes.len())
            .filter(|&i| {
                let rel = self.notes[i].rel.to_lowercase();
                !words.is_empty() && words.iter().all(|w| rel.contains(w.as_str()))
            })
            .collect()
    }

    /// Completions for `[[query` typed in the note at `from_rel`: notes by
    /// name (best first), or `Note#heading` once there is a `#`.
    pub fn complete(&self, from_rel: Option<&str>, query: &str, limit: usize) -> Vec<Completion> {
        if let Some((note, h)) = query.split_once('#') {
            let target = if note.trim().is_empty() {
                from_rel.and_then(|r| self.find(r))
            } else {
                self.resolve(from_rel, note)
            };
            let Some(t) = target else {
                return Vec::new();
            };
            let h = h.to_lowercase();
            let mut out: Vec<(u32, Completion)> = self.notes[t]
                .headings
                .iter()
                .filter_map(|x| {
                    let s = score(&x.text.to_lowercase(), &h)?;
                    Some((
                        s,
                        Completion {
                            label: x.text.clone(),
                            detail: format!(
                                "{} {}",
                                "#".repeat(x.level.into()),
                                self.notes[t].stem()
                            ),
                            insert: format!("{note}#{}", x.text),
                        },
                    ))
                })
                .collect();
            out.sort_by_key(|(s, _)| *s);
            return out.into_iter().take(limit).map(|(_, c)| c).collect();
        }
        let q = query.trim().to_lowercase();
        let mut out: Vec<(u32, usize, Completion)> = Vec::new();
        for n in &self.notes {
            if from_rel == Some(n.rel.as_str()) {
                continue;
            }
            let stem = n.stem();
            let Some(s) = score(&stem.to_lowercase(), &q)
                .or_else(|| score(&n.rel.to_lowercase(), &q).map(|s| s + 1000))
            else {
                continue;
            };
            // Two notes of one name: write the path so the link is exact.
            let insert = if self
                .by_stem
                .get(&name_key(stem))
                .is_some_and(|v| v.len() > 1)
            {
                n.rel
                    .strip_suffix(".md")
                    .or_else(|| n.rel.strip_suffix(".markdown"))
                    .unwrap_or(&n.rel)
                    .to_owned()
            } else {
                stem.to_owned()
            };
            out.push((
                s,
                n.rel.len(),
                Completion {
                    label: stem.to_owned(),
                    detail: n.rel.clone(),
                    insert,
                },
            ));
        }
        out.sort_by_key(|a| (a.0, a.1));
        out.into_iter().take(limit).map(|(_, _, c)| c).collect()
    }

    /// What renaming note `note` to `new_rel` changes in note `in_note`'s
    /// `text` (its current text: an open note's unsaved edits count). The
    /// renamed note itself keeps its text; only links change.
    pub fn rename_splices(
        &self,
        note: usize,
        new_rel: &str,
        in_note: &str,
        text: &str,
    ) -> Vec<Splice> {
        let (links, _) = scan(text);
        let old_stem = self.notes[note].stem().to_owned();
        let new_stem = stem_of(new_rel).to_owned();
        let mut out = Vec::new();
        for l in &links {
            let hit = match l.kind {
                OutKind::Markdown => {
                    Self::markdown_target(in_note, &l.target) == self.notes[note].rel
                }
                _ => self.resolve(Some(in_note), &l.target) == Some(note),
            };
            if !hit {
                continue;
            }
            let written = &text[l.name.clone()];
            let new_text = match l.kind {
                OutKind::Markdown => rewrite_markdown_path(written, &old_stem, new_rel),
                _ => Some(rewrite_wiki_name(written, &new_stem, new_rel)),
            };
            if let Some(t) = new_text
                && t != written
            {
                out.push(Splice {
                    range: l.name.clone(),
                    text: t,
                });
            }
        }
        out
    }

    /// The notes linked from and to `center` within `depth` steps
    /// (`center` included), for the local graph.
    pub fn neighbourhood(&self, center: usize, depth: usize) -> Vec<usize> {
        let edges = self.edges();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); self.notes.len()];
        for &(a, b) in &edges {
            adj[a].push(b);
            adj[b].push(a);
        }
        let mut seen = vec![false; self.notes.len()];
        let mut frontier = vec![center];
        seen[center] = true;
        let mut out = vec![center];
        for _ in 0..depth {
            let mut next = Vec::new();
            for &n in &frontier {
                for &m in &adj[n] {
                    if !seen[m] {
                        seen[m] = true;
                        out.push(m);
                        next.push(m);
                    }
                }
            }
            frontier = next;
        }
        out
    }

    /// Links between notes, each pair once (`a < b`), self-links left out.
    pub fn edges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            for l in &n.links {
                if let Some(j) = self.target_of(i, l)
                    && j != i
                {
                    out.push((i.min(j), i.max(j)));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// `root`-relative path of `p`, `/`-separated.
pub fn rel_path(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let s: Vec<String> = r
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!s.is_empty()).then(|| s.join("/"))
}

/// A wiki name written `written` pointing at a note now at `new_rel`: the
/// same style (bare name, path, `.md`), with the new name.
fn rewrite_wiki_name(written: &str, new_stem: &str, new_rel: &str) -> String {
    let w = written.replace('\\', "/");
    let ext = [".md", ".markdown"]
        .into_iter()
        .find(|e| w.to_lowercase().ends_with(e))
        .map(|e| &written[written.len() - e.len()..]);
    if w.contains('/') {
        // A path: the new path, in the same form.
        let new_path = new_rel
            .strip_suffix(".md")
            .or_else(|| new_rel.strip_suffix(".markdown"))
            .unwrap_or(new_rel);
        // Written as the tail of a path: keep as many parts as before.
        let parts = w.trim_end_matches(ext.unwrap_or("")).split('/').count();
        let all: Vec<&str> = new_path.split('/').collect();
        let tail = all[all.len().saturating_sub(parts)..].join("/");
        return format!("{tail}{}", ext.unwrap_or(""));
    }
    format!("{new_stem}{}", ext.unwrap_or(""))
}

/// A Markdown link's path (as written) after the file it names becomes
/// `new_rel`'s file name; spaces stay encoded the way they were.
fn rewrite_markdown_path(written: &str, old_stem: &str, new_rel: &str) -> Option<String> {
    let (dir, file) = match written.rfind('/') {
        Some(i) => (&written[..=i], &written[i + 1..]),
        None => ("", written),
    };
    let decoded = percent_decode(file);
    let ext = decoded.rsplit_once('.').map_or("md", |(_, e)| e).to_owned();
    if stem_of(&decoded) != old_stem && !stem_of(&decoded).eq_ignore_ascii_case(old_stem) {
        return None;
    }
    let new_name = format!("{}.{ext}", stem_of(new_rel));
    let encoded = if file.contains("%20") || (!file.contains(' ') && new_name.contains(' ')) {
        new_name.replace(' ', "%20")
    } else {
        new_name
    };
    Some(format!("{dir}{encoded}"))
}

/// Inline and fenced code in `text` (skipped by unlinked mentions).
fn code_ranges(text: &str) -> Vec<Range<usize>> {
    let buf = Buffer::new(text);
    let parsed = parse::parse(&buf);
    let mut out: Vec<Range<usize>> = Vec::new();
    for (l, info) in parsed.lines.iter().enumerate() {
        match info.kind {
            parse::LineKind::Code { .. } | parse::LineKind::CodeFence { .. } => {
                out.push(buf.line_range(l))
            }
            _ => out.extend(
                info.spans
                    .iter()
                    .filter(|s| matches!(s.kind, parse::SpanKind::Code))
                    .map(|s| s.range.clone()),
            ),
        }
    }
    out
}

fn whole_word(text: &str, r: &Range<usize>) -> bool {
    let before = text[..r.start].chars().next_back();
    let after = text[r.end..].chars().next();
    !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
}

/// How well `name` matches `q` (lower is better): prefix, word start,
/// substring, then letters in order; `None` if it doesn't.
fn score(name: &str, q: &str) -> Option<u32> {
    if q.is_empty() {
        return Some(500);
    }
    if name == q {
        return Some(0);
    }
    if name.starts_with(q) {
        return Some(10 + (name.len() - q.len()).min(80) as u32);
    }
    if let Some(i) = name.find(q) {
        let word = name[..i].ends_with([' ', '-', '_', '/', '.']);
        return Some(if word { 100 } else { 200 } + i.min(90) as u32);
    }
    // Letters in order.
    let mut it = name.chars();
    let mut gaps = 0u32;
    for c in q.chars() {
        loop {
            match it.next() {
                Some(x) if x == c => break,
                Some(_) => gaps += 1,
                None => return None,
            }
        }
    }
    Some(300 + gaps.min(190))
}

/// The `[[` query the caret is in: `line_before` is the line up to the
/// caret. Returns where the query starts (after `[[`) and the query.
pub fn completion_query(line_before: &str) -> Option<(usize, &str)> {
    let open = line_before.rfind("[[")?;
    let q = &line_before[open + 2..];
    if q.contains("]]") || q.contains('|') || q.chars().count() > 80 {
        return None;
    }
    Some((open + 2, q))
}

/// A file name for a new note called `name` (characters that can't be in
/// file names on Windows or in links taken out).
pub fn note_file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_matches('.').to_owned();
    if clean.is_empty() {
        "Untitled.md".to_owned()
    } else if crate::is_note_path(Path::new(&clean)) {
        clean
    } else {
        format!("{clean}.md")
    }
}

/// `dir/name`, made unique with ` 2`, ` 3`… before the extension.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let p = dir.join(file_name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (file_name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(notes: &[(&str, &str)]) -> Vault {
        Vault::from_notes(
            Path::new("/v"),
            notes
                .iter()
                .map(|(r, t)| (r.to_string(), t.to_string()))
                .collect(),
        )
    }

    #[test]
    fn wiki_link_parts() {
        let t = "a [[Note]] b [[Folder/Other#Part two|shown]] ![[Pic]] [[ Spaced ]]";
        let (links, _) = scan(t);
        assert_eq!(links.len(), 4);
        assert_eq!(links[0].target, "Note");
        assert_eq!(&t[links[0].name.clone()], "Note");
        assert_eq!(links[1].target, "Folder/Other");
        assert_eq!(links[1].heading.as_deref(), Some("Part two"));
        assert_eq!(links[2].kind, OutKind::Embed);
        assert_eq!(links[2].target, "Pic");
        assert_eq!(links[3].target, "Spaced");
        let p = wiki_parts(t, links[1].range.clone()).unwrap();
        assert_eq!(&t[p.alias.unwrap()], "shown");
    }

    #[test]
    fn escaped_pipe_in_a_table() {
        let t = "| [[Note\\|alias]] |";
        let p = wiki_parts(t, 2..17).unwrap();
        assert_eq!(&t[p.name], "Note");
        assert_eq!(&t[p.alias.unwrap()], "alias");
    }

    #[test]
    fn markdown_links_to_notes() {
        let t = "See [x](Other%20Note.md#Top) and [y](<sub/A b.md>) and [w](https://a.b/c.md)";
        let (links, _) = scan(t);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].kind, OutKind::Markdown);
        assert_eq!(links[0].target, "Other Note.md");
        assert_eq!(&t[links[0].name.clone()], "Other%20Note.md");
        assert_eq!(links[1].target, "sub/A b.md");
        assert_eq!(&t[links[1].name.clone()], "sub/A b.md");
    }

    #[test]
    fn names_resolve_like_obsidian() {
        let v = vault(&[
            ("Index.md", ""),
            ("a/Topic.md", ""),
            ("b/Topic.md", ""),
            ("b/deep/Only.md", ""),
        ]);
        let topic_a = v.find("a/Topic.md");
        let topic_b = v.find("b/Topic.md");
        assert_eq!(
            v.resolve(Some("b/x.md"), "topic"),
            topic_b,
            "same folder first"
        );
        assert_eq!(
            v.resolve(Some("Index.md"), "Topic"),
            topic_a,
            "then shortest"
        );
        assert_eq!(v.resolve(None, "b/Topic"), topic_b);
        assert_eq!(v.resolve(None, "deep/Only.md"), v.find("b/deep/Only.md"));
        assert_eq!(v.resolve(None, "Missing"), None);
    }

    #[test]
    fn backlinks_and_unlinked_mentions() {
        let v = vault(&[
            (
                "Alpha.md",
                "Links to [[Beta]] and [[Beta#Intro|the intro]].",
            ),
            ("Beta.md", "# Intro\nBeta itself."),
            (
                "Gamma.md",
                "Talks about beta here, `beta` in code, [[Beta]] linked, Betamax no.",
            ),
            ("Delta.md", "[b](Beta.md)"),
        ]);
        let beta = v.find("Beta.md").unwrap();
        let back = v.backlinks(beta);
        let from: Vec<&str> = back.iter().map(|m| v.notes[m.note].rel.as_str()).collect();
        assert_eq!(from, ["Alpha.md", "Alpha.md", "Delta.md", "Gamma.md"]);
        let un = v.unlinked_mentions(beta, 10);
        assert_eq!(un.len(), 1);
        let g = &v.notes[un[0].note];
        assert_eq!(&g.text[un[0].range.clone()], "beta");
        assert_eq!(v.edges().len(), 3);
    }

    #[test]
    fn rename_splices_touch_only_link_names() {
        let v = vault(&[
            ("Old.md", "Self [[Old]]"),
            (
                "a/Other.md",
                "x [[Old]] y [[old#H|alias]] z [[Old.md]] [[Elsewhere]]\r\n[m](../Old.md) ![[Old]]",
            ),
        ]);
        let old = v.find("Old.md").unwrap();
        let text = v.notes[v.find("a/Other.md").unwrap()].text.clone();
        let sp = v.rename_splices(old, "New Name.md", "a/Other.md", &text);
        let out = apply_splices(&text, &sp);
        assert_eq!(
            out,
            "x [[New Name]] y [[New Name#H|alias]] z [[New Name.md]] [[Elsewhere]]\r\n[m](../New%20Name.md) ![[New Name]]"
        );
        // Undo restores the bytes.
        let inv = inverse_splices(&text, &sp);
        assert_eq!(apply_splices(&out, &inv), text);
        // The renamed note's own self-link.
        let own = v.rename_splices(old, "New Name.md", "Old.md", "Self [[Old]]");
        assert_eq!(apply_splices("Self [[Old]]", &own), "Self [[New Name]]");
    }

    #[test]
    fn rename_keeps_path_style() {
        let v = vault(&[("f/Old.md", ""), ("x.md", "[[f/Old]] [[Old]]")]);
        let old = v.find("f/Old.md").unwrap();
        let sp = v.rename_splices(old, "g/New.md", "x.md", "[[f/Old]] [[Old]]");
        assert_eq!(apply_splices("[[f/Old]] [[Old]]", &sp), "[[g/New]] [[New]]");
    }

    #[test]
    fn offsets_follow_splices() {
        let sp = vec![Splice {
            range: 2..5,
            text: "abcdef".into(),
        }];
        assert_eq!(map_offset(1, &sp), 1);
        assert_eq!(map_offset(3, &sp), 8);
        assert_eq!(map_offset(6, &sp), 9);
    }

    #[test]
    fn search_and_names() {
        let v = vault(&[
            ("One.md", "alpha beta\ngamma\nBeta Alpha"),
            ("Two.md", "nothing"),
        ]);
        let hits = v.search("beta ALPHA", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].line, 2);
        assert_eq!(&v.notes[0].text[hits[1].range.clone()], "Beta");
        assert_eq!(v.search_names("two"), vec![1]);
    }

    #[test]
    fn completion() {
        let v = vault(&[
            ("Project Plan.md", "# Goals\n## Risks\n"),
            ("Plans/Old plan.md", ""),
            ("Zebra.md", ""),
        ]);
        assert_eq!(completion_query("see [[Proj"), Some((6, "Proj")));
        assert_eq!(completion_query("see [[Proj]] x"), None);
        let c = v.complete(None, "plan", 5);
        assert_eq!(c[0].label, "Old plan");
        assert!(c.iter().any(|x| x.label == "Project Plan"));
        assert!(!c.iter().any(|x| x.label == "Zebra"));
        let h = v.complete(None, "Project Plan#ri", 5);
        assert_eq!(h[0].insert, "Project Plan#Risks");
        let fuzzy = v.complete(None, "zbr", 5);
        assert_eq!(fuzzy[0].label, "Zebra");
    }

    #[test]
    fn neighbourhood_depth() {
        let v = vault(&[
            ("A.md", "[[B]]"),
            ("B.md", "[[C]]"),
            ("C.md", "[[D]]"),
            ("D.md", ""),
        ]);
        let a = v.find("A.md").unwrap();
        assert_eq!(v.neighbourhood(a, 1).len(), 2);
        assert_eq!(v.neighbourhood(a, 2).len(), 3);
    }

    #[test]
    fn file_names_for_new_notes() {
        assert_eq!(note_file_name("A: b/c?"), "A b c.md");
        assert_eq!(note_file_name("  "), "Untitled.md");
        assert_eq!(note_file_name("x.md"), "x.md");
    }

    #[test]
    fn scanning_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        std::fs::write(dir.path().join("A.md"), "\u{feff}[[B]]").unwrap();
        std::fs::write(dir.path().join("sub/B.md"), "b").unwrap();
        std::fs::write(dir.path().join(".obsidian/x.md"), "hidden").unwrap();
        std::fs::write(dir.path().join("c.txt"), "not a note").unwrap();
        let v = Vault::scan(dir.path());
        let rels: Vec<&str> = v.notes.iter().map(|n| n.rel.as_str()).collect();
        assert_eq!(rels, ["A.md", "sub/B.md"]);
        assert!(v.notes[0].bom);
        assert_eq!(v.edges(), vec![(0, 1)]);
        assert_eq!(v.find_path(&dir.path().join("sub/B.md")), Some(1));
    }
}
