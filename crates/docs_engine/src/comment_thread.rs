//! Threaded comments as Word 2013 and later save them: replies and the
//! resolved ("done") flag live in `commentsExtended.xml` (`w15:commentEx`,
//! keyed by the `w14:paraId` of each comment's last paragraph, a reply
//! pointing at its parent's with `w15:paraIdParent`); Word 365 adds
//! `commentsIds.xml` (paraId → durable id), `commentsExtensible.xml`
//! (durable id → UTC date) and `people.xml` (the authors).
//!
//! Reading maps all of that onto [`Comment::parent`] and
//! [`Comment::done`]. Writing regenerates the three comment parts from the
//! model whenever the comments changed (keeping each extensible entry and
//! person as read), and creates them, with their relationships and content
//! types, the first time a document needs them.

use std::collections::{HashMap, HashSet};

use crate::document::Document;
use crate::model::{OBJECT, Object, Opaque, Paragraph, Position, RunProps, Selection};
use crate::object::Level;
use crate::review::{Comment, comment_id};
use crate::xml;

pub(crate) const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
pub(crate) const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
pub(crate) const W15_NS: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
pub(crate) const W16CID_NS: &str = "http://schemas.microsoft.com/office/word/2016/wordml/cid";
pub(crate) const W16CEX_NS: &str = "http://schemas.microsoft.com/office/word/2018/wordml/cex";

/// One of the extra comment parts: relationship type suffix (as found),
/// full relationship type and content type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Extra {
    Extended,
    Ids,
    Extensible,
    People,
}

impl Extra {
    pub const ALL: [Extra; 4] = [Self::Extended, Self::Ids, Self::Extensible, Self::People];

    /// The end of the relationship type, to find the part.
    pub fn rel_suffix(self) -> &'static str {
        match self {
            Self::Extended => "/commentsExtended",
            Self::Ids => "/commentsIds",
            Self::Extensible => "/commentsExtensible",
            Self::People => "/people",
        }
    }

    pub fn rel_type(self) -> &'static str {
        match self {
            Self::Extended => {
                "http://schemas.microsoft.com/office/2011/relationships/commentsExtended"
            }
            Self::Ids => "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds",
            Self::Extensible => {
                "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible"
            }
            Self::People => "http://schemas.microsoft.com/office/2011/relationships/people",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Extended => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml"
            }
            Self::Ids => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsIds+xml"
            }
            Self::Extensible => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtensible+xml"
            }
            Self::People => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.people+xml"
            }
        }
    }

    /// File name of a new part.
    pub fn file(self) -> &'static str {
        match self {
            Self::Extended => "commentsExtended.xml",
            Self::Ids => "commentsIds.xml",
            Self::Extensible => "commentsExtensible.xml",
            Self::People => "people.xml",
        }
    }

    /// Root start tag of a new part.
    fn new_root(self) -> String {
        match self {
            Self::Extended => format!("<w15:commentsEx xmlns:w15=\"{W15_NS}\">"),
            Self::Ids => format!("<w16cid:commentsIds xmlns:w16cid=\"{W16CID_NS}\">"),
            Self::Extensible => {
                format!("<w16cex:commentsExtensible xmlns:w16cex=\"{W16CEX_NS}\">")
            }
            Self::People => format!("<w15:people xmlns:w15=\"{W15_NS}\">"),
        }
    }
}

/// An extra comment part as read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExtraPart {
    pub kind: Extra,
    pub name: String,
    /// The root start tag, kept when the part is written again.
    pub root_tag: String,
    /// The part's text (people.xml keeps its persons as they were).
    pub src: String,
}

/// The extra comment parts of a package.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ThreadParts {
    pub parts: Vec<ExtraPart>,
    /// `w16cex:commentExtensible` elements by durable id, as read.
    pub extensible: HashMap<String, String>,
}

impl ThreadParts {
    pub fn get(&self, kind: Extra) -> Option<&ExtraPart> {
        self.parts.iter().find(|p| p.kind == kind)
    }
}

fn root_tag(src: &str) -> Option<String> {
    let mut at = 0;
    while let Some(i) = src[at..].find('<') {
        let s = at + i;
        let rest = &src[s + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            at = s + 1;
            continue;
        }
        let e = src[s..].find('>')?;
        return Some(src[s..=s + e].to_owned());
    }
    None
}

/// Every element named `local` (any prefix) in `src`, whole (self-closing
/// or with its end tag).
fn elements<'a>(src: &'a str, local: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = src[at..].find('<') {
        let s = at + i;
        let name: String = src[s + 1..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
            .collect();
        let bare = name.rsplit(':').next().unwrap_or("");
        if bare != local || name.starts_with('/') {
            at = s + 1;
            continue;
        }
        let Some(gt) = src[s..].find('>') else { break };
        let tag_end = s + gt + 1;
        if src[..tag_end].ends_with("/>") {
            out.push(&src[s..tag_end]);
            at = tag_end;
            continue;
        }
        let close = format!("</{name}>");
        let Some(c) = src[tag_end..].find(&close) else {
            break;
        };
        let end = tag_end + c + close.len();
        out.push(&src[s..end]);
        at = end;
    }
    out
}

/// The `w14:paraId` of a comment element's last paragraph.
pub(crate) fn last_para_id(comment_xml: &str) -> Option<String> {
    let at = last_p_tag(comment_xml)?;
    let e = comment_xml[at..].find('>')? + at;
    let tag = comment_xml[at..e].trim_end_matches('/');
    xml::parse(&format!("{tag}/>"))?
        .attr("paraId")
        .map(str::to_owned)
}

/// Byte offset of the last `<w:p>` / `<w:p …>` start tag.
fn last_p_tag(s: &str) -> Option<usize> {
    let a = s.rfind("<w:p>");
    let b = s.rfind("<w:p ");
    a.max(b)
}

/// `comment_xml` with `w14:paraId="id"` on its last paragraph (replacing
/// one that's there).
pub(crate) fn with_para_id(comment_xml: &str, id: &str) -> String {
    let Some(at) = last_p_tag(comment_xml) else {
        return comment_xml.to_owned();
    };
    let Some(e) = comment_xml[at..].find('>').map(|e| e + at) else {
        return comment_xml.to_owned();
    };
    let tag = &comment_xml[at..e];
    let new_tag = if let Some(i) = tag.find("w14:paraId=\"") {
        let v = i + "w14:paraId=\"".len();
        let close = tag[v..].find('"').map_or(tag.len(), |c| v + c);
        format!("{}{id}{}", &tag[..v], &tag[close..])
    } else {
        format!("<w:p w14:paraId=\"{id}\"{}", &tag["<w:p".len()..])
    };
    format!("{}{new_tag}{}", &comment_xml[..at], &comment_xml[e..])
}

/// A hex id below 0x80000000 (as Word requires for paraIds and durable
/// ids), derived from `seed` and not in `taken`.
pub(crate) fn hex_id(seed: &str, taken: &HashSet<String>) -> String {
    // FNV-1a.
    let mut h: u32 = 0x811c_9dc5;
    for b in seed.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    loop {
        let v = (h & 0x7FFF_FFFF).max(1);
        let s = format!("{v:08X}");
        if !taken.contains(&s) {
            return s;
        }
        h = h.wrapping_mul(0x0100_0193).wrapping_add(1);
    }
}

/// Fill in threads from the extra parts and give every comment a paraId
/// and durable id (derived from its id when the file has none, so saves
/// stay the same).
pub(crate) fn read(comments: &mut [Comment], srcs: Vec<(Extra, String, String)>) -> ThreadParts {
    let mut parts = ThreadParts::default();
    let mut ex: HashMap<String, (Option<String>, bool)> = HashMap::new();
    let mut durable: HashMap<String, String> = HashMap::new();
    for (kind, name, src) in srcs {
        match kind {
            Extra::Extended => {
                for e in elements(&src, "commentEx") {
                    if let Some(el) = xml::parse(e)
                        && let Some(p) = el.attr("paraId")
                    {
                        ex.insert(
                            p.to_owned(),
                            (
                                el.attr("paraIdParent").map(str::to_owned),
                                matches!(el.attr("done"), Some("1" | "true")),
                            ),
                        );
                    }
                }
            }
            Extra::Ids => {
                for e in elements(&src, "commentId") {
                    if let Some(el) = xml::parse(e)
                        && let (Some(p), Some(d)) = (el.attr("paraId"), el.attr("durableId"))
                    {
                        durable.insert(p.to_owned(), d.to_owned());
                    }
                }
            }
            Extra::Extensible => {
                for e in elements(&src, "commentExtensible") {
                    if let Some(d) =
                        xml::parse(e).and_then(|el| el.attr("durableId").map(str::to_owned))
                    {
                        parts.extensible.insert(d, e.to_owned());
                    }
                }
            }
            Extra::People => {}
        }
        parts.parts.push(ExtraPart {
            kind,
            name,
            root_tag: root_tag(&src).unwrap_or_else(|| kind.new_root()),
            src,
        });
    }
    let mut taken: HashSet<String> = HashSet::new();
    for c in comments.iter_mut() {
        c.para_id = c.xml.as_deref().and_then(last_para_id);
        if let Some(p) = &c.para_id {
            taken.insert(p.clone());
        }
    }
    for c in comments.iter_mut() {
        if c.para_id.is_none() {
            let p = hex_id(&format!("comment-para-{}", c.id), &taken);
            taken.insert(p.clone());
            c.para_id = Some(p);
        }
    }
    let by_para: HashMap<String, String> = comments
        .iter()
        .filter_map(|c| Some((c.para_id.clone()?, c.id.clone())))
        .collect();
    let mut durables: HashSet<String> = durable.values().cloned().collect();
    for c in comments.iter_mut() {
        let p = c.para_id.clone().unwrap_or_default();
        if let Some((parent, done)) = ex.get(&p) {
            c.done = *done;
            c.parent = parent
                .as_ref()
                .and_then(|pp| by_para.get(pp))
                .filter(|id| **id != c.id)
                .cloned();
        }
        c.durable_id = Some(match durable.get(&p) {
            Some(d) => d.clone(),
            None => {
                let d = hex_id(&format!("comment-durable-{}", c.id), &durables);
                durables.insert(d.clone());
                d
            }
        });
    }
    parts
}

/// `root` with namespace `prefix` declared and, if `ignorable`, listed in
/// `mc:Ignorable` (declaring `mc` too).
pub(crate) fn ensure_ns(root: &str, prefix: &str, uri: &str, ignorable: bool) -> String {
    let mut out = root.to_owned();
    let insert = |out: &mut String, attr: &str| {
        let at = if out.ends_with("/>") {
            out.len() - 2
        } else {
            out.len() - 1
        };
        out.insert_str(at, attr);
    };
    if !out.contains(&format!("xmlns:{prefix}=")) {
        insert(&mut out, &format!(" xmlns:{prefix}=\"{uri}\""));
    }
    if ignorable {
        if !out.contains("xmlns:mc=") {
            insert(&mut out, &format!(" xmlns:mc=\"{MC_NS}\""));
        }
        match out.find("mc:Ignorable=\"") {
            Some(i) => {
                let v = i + "mc:Ignorable=\"".len();
                let close = out[v..].find('"').map_or(out.len(), |c| v + c);
                if !out[v..close].split_whitespace().any(|p| p == prefix) {
                    let sep = if close > v { " " } else { "" };
                    out.insert_str(close, &format!("{sep}{prefix}"));
                }
            }
            None => insert(&mut out, &format!(" mc:Ignorable=\"{prefix}\"")),
        }
    }
    out
}

/// What saving the comments writes besides `comments.xml`.
pub(crate) struct Written {
    /// Comments with their paraIds on the last paragraph.
    pub comments: Vec<Comment>,
    /// The parts: kind, existing part (if any), bytes.
    pub parts: Vec<(Extra, Option<String>, Vec<u8>)>,
}

/// The extra comment parts for `comments`, from `read` (the package's).
pub(crate) fn write(comments: &[Comment], read: &ThreadParts) -> Written {
    const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
    let comments: Vec<Comment> = comments
        .iter()
        .map(|c| {
            let mut c = c.clone();
            if let (Some(x), Some(p)) = (&c.xml, &c.para_id)
                && last_para_id(x).as_deref() != Some(p.as_str())
            {
                c.xml = Some(with_para_id(x, p));
            }
            c
        })
        .collect();
    let para = |id: &str| {
        comments
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.para_id.clone())
    };
    let root_of = |k: Extra| {
        read.get(k)
            .map(|p| p.root_tag.clone())
            .unwrap_or_else(|| k.new_root())
    };
    let close_of = |root: &str| {
        let name: String = root[1..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
            .collect();
        format!("</{name}>")
    };
    let open = |root: &str| root.trim_end_matches("/>").trim_end_matches('>').to_owned() + ">";
    let mut parts = Vec::new();

    let root = root_of(Extra::Extended);
    let mut x = format!("{DECL}{}", open(&root));
    for c in &comments {
        let Some(p) = &c.para_id else { continue };
        x.push_str(&format!("<w15:commentEx w15:paraId=\"{p}\""));
        if let Some(pp) = c.parent.as_deref().and_then(para) {
            x.push_str(&format!(" w15:paraIdParent=\"{pp}\""));
        }
        x.push_str(&format!(" w15:done=\"{}\"/>", u8::from(c.done)));
    }
    x.push_str(&close_of(&root));
    parts.push((
        Extra::Extended,
        read.get(Extra::Extended).map(|p| p.name.clone()),
        x.into_bytes(),
    ));

    let root = root_of(Extra::Ids);
    let mut x = format!("{DECL}{}", open(&root));
    for c in &comments {
        if let (Some(p), Some(d)) = (&c.para_id, &c.durable_id) {
            x.push_str(&format!(
                "<w16cid:commentId w16cid:paraId=\"{p}\" w16cid:durableId=\"{d}\"/>"
            ));
        }
    }
    x.push_str(&close_of(&root));
    parts.push((
        Extra::Ids,
        read.get(Extra::Ids).map(|p| p.name.clone()),
        x.into_bytes(),
    ));

    let root = root_of(Extra::Extensible);
    let mut x = format!("{DECL}{}", open(&root));
    for c in &comments {
        let Some(d) = &c.durable_id else { continue };
        match read.extensible.get(d) {
            Some(e) => x.push_str(e),
            None => {
                x.push_str(&format!(
                    "<w16cex:commentExtensible w16cex:durableId=\"{d}\""
                ));
                if !c.date.is_empty() {
                    let utc = if c.date.ends_with('Z') {
                        c.date.clone()
                    } else {
                        format!("{}Z", c.date.trim_end_matches('Z'))
                    };
                    x.push_str(&format!(" w16cex:dateUtc=\"{}\"", xml::escape(&utc)));
                }
                x.push_str("/>");
            }
        }
    }
    x.push_str(&close_of(&root));
    parts.push((
        Extra::Extensible,
        read.get(Extra::Extensible).map(|p| p.name.clone()),
        x.into_bytes(),
    ));

    // People: the persons as read, plus authors not listed yet.
    let people = read.get(Extra::People);
    let src = people.map_or_else(
        || {
            let r = Extra::People.new_root();
            format!("{DECL}{r}{}", close_of(&r))
        },
        |p| p.src.clone(),
    );
    let known: HashSet<String> = elements(&src, "person")
        .into_iter()
        .filter_map(|e| xml::parse(e)?.attr("author").map(str::to_owned))
        .collect();
    let mut add = String::new();
    let mut seen = HashSet::new();
    for c in &comments {
        if c.author.is_empty() || known.contains(&c.author) || !seen.insert(c.author.clone()) {
            continue;
        }
        let a = xml::escape(&c.author);
        add.push_str(&format!(
            "<w15:person w15:author=\"{a}\"><w15:presenceInfo w15:providerId=\"None\" w15:userId=\"{a}\"/></w15:person>"
        ));
    }
    if people.is_none() || !add.is_empty() {
        let src = if add.is_empty() {
            src
        } else if let Some(i) = src.rfind("</") {
            format!("{}{add}{}", &src[..i], &src[i..])
        } else {
            src
        };
        parts.push((
            Extra::People,
            people.map(|p| p.name.clone()),
            src.into_bytes(),
        ));
    }
    Written { comments, parts }
}

impl Document {
    /// Reply to comment `parent` (a reply to a reply joins the same
    /// thread): one undo step. Returns the reply's id.
    pub fn reply_comment(&mut self, parent: &str, text: &str, author: &str) -> Option<String> {
        let root = self.thread_root(parent)?;
        // Where the root's markers are: the reply's go just inside them,
        // as Word places them.
        let mut start: Option<Position> = None;
        let mut reference: Option<Position> = None;
        let mut end: Option<Position> = None;
        for (pi, p) in self.paragraphs.iter().enumerate() {
            for (off, o) in p.objects() {
                match comment_id(o) {
                    Some(("start", id)) if id == root && start.is_none() => {
                        start = Some(Position::new(pi, off + OBJECT.len_utf8()));
                    }
                    Some(("ref", id)) if id == root && reference.is_none() => {
                        reference = Some(Position::new(pi, off + OBJECT.len_utf8()));
                    }
                    Some(("end", id)) if id == root && end.is_none() => {
                        end = Some(Position::new(pi, off + OBJECT.len_utf8()));
                    }
                    _ => {}
                }
            }
        }
        let b = reference.or(end)?;
        let a = start.unwrap_or(b);
        let before = self.review_state();
        let id = self.fresh_comment_id();
        let mut c = self.new_comment(&id, text, author);
        c.parent = Some(root);
        self.comments.push(c);
        self.comments_modified = true;
        self.insert_comment_marks(&id, a, b, a == b, Selection::caret(a));
        let after = self.review_state();
        self.set_last_side(before, after);
        Some(id)
    }

    /// The comment a thread starts with (`id` itself when it's no reply).
    pub fn thread_root(&self, id: &str) -> Option<String> {
        let mut cur = self.comments.iter().find(|c| c.id == id)?;
        for _ in 0..self.comments.len() {
            match cur
                .parent
                .as_deref()
                .and_then(|p| self.comments.iter().find(|c| c.id == p))
            {
                Some(p) => cur = p,
                None => break,
            }
        }
        Some(cur.id.clone())
    }

    /// The replies in `root`'s thread, in file order.
    pub fn thread_replies(&self, root: &str) -> Vec<&Comment> {
        // Parents by id, so finding each comment's root is quick (a
        // reviewed document can have thousands of comments).
        let mut parent: std::collections::HashMap<&str, Option<&str>> =
            std::collections::HashMap::with_capacity(self.comments.len());
        for c in &self.comments {
            parent.entry(c.id.as_str()).or_insert(c.parent.as_deref());
        }
        let root_of = |id: &str| -> &str {
            let mut cur = id;
            for _ in 0..self.comments.len() {
                match parent.get(cur).copied().flatten() {
                    Some(p) if parent.contains_key(p) => cur = p,
                    _ => break,
                }
            }
            cur
        };
        self.comments
            .iter()
            .filter(|c| c.id != root && root_of(&c.id) == root)
            .collect()
    }

    /// Resolve (or reopen) the thread `id` is in: one undo step.
    pub fn set_comment_done(&mut self, id: &str, done: bool, sel: Selection) -> bool {
        let Some(root) = self.thread_root(id) else {
            return false;
        };
        let before = self.review_state();
        for c in &mut self.comments {
            if c.id == root {
                c.done = done;
            }
        }
        if before.comments == self.comments {
            return true;
        }
        self.comments_modified = true;
        let after = self.review_state();
        if before != after {
            self.push_command(
                0,
                Vec::new(),
                Vec::new(),
                sel,
                Some(Box::new((before, after))),
            );
        }
        true
    }

    pub(crate) fn fresh_comment_id(&self) -> String {
        (0..)
            .map(|n: u32| n.to_string())
            .find(|id| !self.comments.iter().any(|c| c.id == *id) && !self.has_comment_marker(id))
            .expect("infinite ids")
    }

    /// A new comment by `author`, dated now, with fresh paraId and
    /// durable id.
    pub(crate) fn new_comment(&self, id: &str, text: &str, author: &str) -> Comment {
        let initials: String = author
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .flat_map(char::to_uppercase)
            .take(3)
            .collect();
        let date = crate::review::now_utc();
        let paras: HashSet<String> = self
            .comments
            .iter()
            .filter_map(|c| c.para_id.clone())
            .collect();
        let durables: HashSet<String> = self
            .comments
            .iter()
            .filter_map(|c| c.durable_id.clone())
            .collect();
        let seed = format!("{id}|{author}|{date}|{}", self.comments.len());
        Comment {
            id: id.to_owned(),
            author: author.to_owned(),
            initials,
            date,
            text: text.to_owned(),
            xml: None,
            para_id: Some(hex_id(&format!("p{seed}"), &paras)),
            durable_id: Some(hex_id(&format!("d{seed}"), &durables)),
            parent: None,
            done: false,
        }
    }

    /// Put comment `id`'s range markers at `a` and `b` and its reference
    /// after the end marker (one undo step, no side state).
    pub(crate) fn insert_comment_marks(
        &mut self,
        id: &str,
        a: Position,
        b: Position,
        no_range: bool,
        sel: Selection,
    ) {
        let marker = |name: &str| {
            Object::Opaque(std::sync::Arc::new(Opaque {
                level: Level::Marker,
                xml: format!("<w:{name} w:id=\"{id}\"/>"),
                label: String::new(),
            }))
        };
        let reference = Object::Opaque(std::sync::Arc::new(Opaque {
            level: Level::Run,
            xml: format!("<w:commentReference w:id=\"{id}\"/>"),
            label: String::new(),
        }));
        let ref_props = RunProps {
            style: self
                .styles
                .get("CommentReference")
                .map(|_| "CommentReference".to_owned()),
            ..RunProps::default()
        };
        let first = a.para;
        let last = b.para;
        let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
        let n = new.len();
        // End first so the start offset stays valid.
        {
            let p = &mut new[n - 1];
            p.insert_object(b.offset, reference, ref_props);
            if !no_range {
                p.insert_object(b.offset, marker("commentRangeEnd"), RunProps::default());
            }
        }
        if !no_range {
            new[0].insert_object(a.offset, marker("commentRangeStart"), RunProps::default());
        }
        self.replace(
            first,
            n,
            new,
            (sel, sel),
            crate::document::Kind::Other,
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn para_ids_are_found_and_set() {
        let x = r#"<w:comment w:id="0"><w:p w14:paraId="1A2B3C4D"><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r></w:p></w:comment>"#;
        assert_eq!(last_para_id(x), None);
        let y = with_para_id(x, "00000011");
        assert_eq!(last_para_id(&y).as_deref(), Some("00000011"));
        let z = with_para_id(&y, "00000022");
        assert_eq!(last_para_id(&z).as_deref(), Some("00000022"));
        assert!(z.contains("1A2B3C4D"));
    }

    #[test]
    fn hex_ids_stay_below_the_sign_bit() {
        let mut taken = HashSet::new();
        for i in 0..200 {
            let id = hex_id(&i.to_string(), &taken);
            assert!(u32::from_str_radix(&id, 16).unwrap() < 0x8000_0000);
            assert!(taken.insert(id));
        }
        // Collisions move on.
        let first = hex_id("x", &HashSet::new());
        let taken: HashSet<String> = [first.clone()].into();
        assert_ne!(hex_id("x", &taken), first);
    }

    #[test]
    fn namespaces_are_added_once() {
        let r = "<w:comments xmlns:w=\"w\">";
        let r = ensure_ns(r, "w14", W14_NS, true);
        assert!(r.contains("xmlns:w14="));
        assert!(r.contains("mc:Ignorable=\"w14\""));
        let again = ensure_ns(&r, "w14", W14_NS, true);
        assert_eq!(again, r);
        let r2 = ensure_ns(
            "<w:comments xmlns:mc=\"m\" mc:Ignorable=\"w15\">",
            "w14",
            W14_NS,
            true,
        );
        assert!(r2.contains("mc:Ignorable=\"w15 w14\""));
    }

    #[test]
    fn threads_are_read_from_comments_extended() {
        let (mut cs, _) = crate::review::parse_comments(
            r#"<w:comments><w:comment w:id="0" w:author="Ani"><w:p w14:paraId="00000001"><w:r><w:t>Q</w:t></w:r></w:p></w:comment><w:comment w:id="1" w:author="Budi"><w:p w14:paraId="00000002"><w:r><w:t>A</w:t></w:r></w:p></w:comment></w:comments>"#,
        );
        let parts = read(
            &mut cs,
            vec![
                (
                    Extra::Extended,
                    "word/commentsExtended.xml".into(),
                    r#"<w15:commentsEx><w15:commentEx w15:paraId="00000001" w15:done="1"/><w15:commentEx w15:paraId="00000002" w15:paraIdParent="00000001" w15:done="0"/></w15:commentsEx>"#.into(),
                ),
                (
                    Extra::Ids,
                    "word/commentsIds.xml".into(),
                    r#"<w16cid:commentsIds><w16cid:commentId w16cid:paraId="00000001" w16cid:durableId="11111111"/></w16cid:commentsIds>"#.into(),
                ),
            ],
        );
        assert!(cs[0].done);
        assert_eq!(cs[0].parent, None);
        assert_eq!(cs[1].parent.as_deref(), Some("0"));
        assert_eq!(cs[0].durable_id.as_deref(), Some("11111111"));
        assert!(cs[1].durable_id.is_some());
        let w = write(&cs, &parts);
        let ex = String::from_utf8(w.parts[0].2.clone()).unwrap();
        assert!(ex.contains(
            r#"<w15:commentEx w15:paraId="00000002" w15:paraIdParent="00000001" w15:done="0"/>"#
        ));
        assert!(ex.contains(r#"w15:paraId="00000001" w15:done="1""#));
        // Two authors become people.
        let people = w.parts.iter().find(|p| p.0 == Extra::People).unwrap();
        let people = String::from_utf8(people.2.clone()).unwrap();
        assert!(people.contains("w15:author=\"Ani\"") && people.contains("w15:author=\"Budi\""));
    }
}
