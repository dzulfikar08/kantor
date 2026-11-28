//! Office Math Markup (OMML, ECMA-376 Part 1 §22.1) to and from the model.
//!
//! Reading is lenient: unknown elements give their content, tracked
//! deletions are left out, insertions kept. Writing produces what Word's
//! equation editor writes, with children in schema order, so Word and
//! PowerPoint open the result as editable equations.

use crate::ast::*;
use roxmltree::Node as X;

pub const NS_M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
pub const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// Prefixes an OMML fragment cut out of a part may use without declaring.
const WRAP_NS: &[(&str, &str)] = &[
    ("m", NS_M),
    ("w", NS_W),
    ("a", NS_A),
    (
        "r",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    (
        "a14",
        "http://schemas.microsoft.com/office/drawing/2010/main",
    ),
    (
        "mc",
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
    ),
    (
        "w14",
        "http://schemas.microsoft.com/office/word/2010/wordml",
    ),
    (
        "w15",
        "http://schemas.microsoft.com/office/word/2012/wordml",
    ),
    (
        "wp",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    ),
    ("v", "urn:schemas-microsoft-com:vml"),
    ("o", "urn:schemas-microsoft-com:office:office"),
];

/// Reads the first `m:oMathPara` or `m:oMath` in `xml` (a fragment of a
/// document part, its prefixes declared or not).
pub fn parse(xml: &str) -> Result<Equation, String> {
    let mut wrapped = String::with_capacity(xml.len() + 600);
    wrapped.push_str("<kantor-wrap");
    for (p, ns) in WRAP_NS {
        wrapped.push_str(&format!(" xmlns:{p}=\"{ns}\""));
    }
    wrapped.push('>');
    wrapped.push_str(xml);
    wrapped.push_str("</kantor-wrap>");
    let doc = roxmltree::Document::parse(&wrapped).map_err(|e| e.to_string())?;
    let root = doc
        .descendants()
        .find(|n| is_m(*n, "oMathPara") || is_m(*n, "oMath"))
        .ok_or_else(|| "no m:oMath element".to_owned())?;
    Ok(from_node(root))
}

/// The equation an `m:oMathPara` or `m:oMath` element holds.
pub fn from_node(root: X) -> Equation {
    if is_m(root, "oMathPara") {
        let justify = kid(root, "oMathParaPr")
            .and_then(|p| val(Some(p), "jc"))
            .map(|v| Justify::from_omml(&v))
            .unwrap_or_default();
        let mut lines: Vec<Row> = root
            .descendants()
            .filter(|n| is_m(*n, "oMath"))
            .map(row)
            .collect();
        if lines.is_empty() {
            lines.push(Row::new());
        }
        Equation {
            display: true,
            justify,
            lines,
        }
    } else {
        Equation::inline(row(root))
    }
}

fn is_m(n: X, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NS_M)
}

fn kid<'a, 'i>(n: X<'a, 'i>, name: &str) -> Option<X<'a, 'i>> {
    n.children().find(|c| is_m(*c, name))
}

fn m_attr(n: X, name: &str) -> Option<String> {
    n.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value().to_owned())
}

/// `m:val` of child `name` of `n`.
fn val(n: Option<X>, name: &str) -> Option<String> {
    kid(n?, name).map(|c| m_attr(c, "val").unwrap_or_default())
}

/// An on/off property: present without a value is on.
fn on_off(n: Option<X>, name: &str) -> Option<bool> {
    let c = kid(n?, name)?;
    Some(match m_attr(c, "val").as_deref() {
        None => true,
        Some(v) => matches!(v, "1" | "on" | "true"),
    })
}

/// A character property (`m:chr`, `m:begChr`…): the first character of
/// its value, `None` when absent. Present with an empty value is `Some("")`.
fn chr(n: Option<X>, name: &str) -> Option<String> {
    let c = kid(n?, name)?;
    Some(m_attr(c, "val").unwrap_or_default())
}

fn arg(n: X, name: &str) -> Row {
    kid(n, name).map(row).unwrap_or_default()
}

fn row(n: X) -> Row {
    let mut out = Row::new();
    for c in n.children() {
        nodes(c, &mut out);
    }
    out
}

fn nodes(n: X, out: &mut Row) {
    if !n.is_element() {
        return;
    }
    let local = n.tag_name().name();
    let ns = n.tag_name().namespace();
    if ns == Some(NS_W) {
        match local {
            // Tracked changes: deletions left out.
            "del" | "moveFrom" | "bookmarkStart" | "bookmarkEnd" | "proofErr" | "rPr" => {}
            "r" => {
                let text: String = n
                    .descendants()
                    .filter(|d| d.is_element() && d.tag_name().name() == "t")
                    .filter_map(|d| d.text())
                    .collect();
                push(out, Node::Text(Run::styled(text, Style::TEXT)));
            }
            _ => {
                for c in n.children() {
                    nodes(c, out);
                }
            }
        }
        return;
    }
    if ns != Some(NS_M) {
        for c in n.children() {
            nodes(c, out);
        }
        return;
    }
    let node = match local {
        "r" => {
            let pr = kid(n, "rPr");
            let mut style = Style::default();
            if let Some(v) = val(pr, "scr") {
                style.variant = Variant::from_omml(&v);
            }
            if let Some(v) = val(pr, "sty") {
                style.weight = Weight::from_omml(&v);
            }
            if on_off(pr, "nor") == Some(true) {
                style.normal = true;
                style.weight.get_or_insert(Weight::Plain);
            }
            let mut text = String::new();
            for c in n.children() {
                if is_m(c, "t") {
                    text.push_str(c.text().unwrap_or_default());
                } else if c.is_element() && c.tag_name().name() == "t" {
                    // `w:t` / `a:t` some writers put in math runs.
                    text.push_str(c.text().unwrap_or_default());
                }
            }
            Node::Text(Run::styled(text, style))
        }
        "f" => Node::Frac {
            kind: val(kid(n, "fPr"), "type")
                .map(|v| FracKind::from_omml(&v))
                .unwrap_or_default(),
            num: arg(n, "num"),
            den: arg(n, "den"),
        },
        "sSub" => Node::Scripts {
            base: arg(n, "e"),
            sub: Some(arg(n, "sub")),
            sup: None,
        },
        "sSup" => Node::Scripts {
            base: arg(n, "e"),
            sub: None,
            sup: Some(arg(n, "sup")),
        },
        "sSubSup" => Node::Scripts {
            base: arg(n, "e"),
            sub: Some(arg(n, "sub")),
            sup: Some(arg(n, "sup")),
        },
        "sPre" => Node::PreScripts {
            sub: arg(n, "sub"),
            sup: arg(n, "sup"),
            base: arg(n, "e"),
        },
        "rad" => {
            let hide = on_off(kid(n, "radPr"), "degHide").unwrap_or(false);
            let deg = arg(n, "deg");
            Node::Radical {
                degree: (!hide && !deg.is_empty()).then_some(deg),
                body: arg(n, "e"),
            }
        }
        "nary" => {
            let pr = kid(n, "naryPr");
            let op = chr(pr, "chr").and_then(|s| s.chars().next()).unwrap_or('∫');
            let under_over = match val(pr, "limLoc").as_deref() {
                Some("undOvr") => true,
                Some("subSup") => false,
                _ => !crate::symbols::is_integral(op),
            };
            let sub_hide = on_off(pr, "subHide").unwrap_or(false);
            let sup_hide = on_off(pr, "supHide").unwrap_or(false);
            let sub = arg(n, "sub");
            let sup = arg(n, "sup");
            Node::Nary {
                op,
                under_over,
                sub: (!sub_hide && !sub.is_empty()).then_some(sub),
                sup: (!sup_hide && !sup.is_empty()).then_some(sup),
                body: arg(n, "e"),
            }
        }
        "d" => {
            let pr = kid(n, "dPr");
            Node::Delim {
                open: chr(pr, "begChr").unwrap_or_else(|| "(".into()),
                close: chr(pr, "endChr").unwrap_or_else(|| ")".into()),
                sep: chr(pr, "sepChr").unwrap_or_else(|| "|".into()),
                items: n.children().filter(|c| is_m(*c, "e")).map(row).collect(),
                grow: on_off(pr, "grow").unwrap_or(true),
            }
        }
        "m" => Node::Matrix {
            rows: n
                .children()
                .filter(|c| is_m(*c, "mr"))
                .map(|r| r.children().filter(|c| is_m(*c, "e")).map(row).collect())
                .collect(),
        },
        "eqArr" => Node::EqArray {
            rows: n.children().filter(|c| is_m(*c, "e")).map(row).collect(),
        },
        "acc" => Node::Accent {
            chr: chr(kid(n, "accPr"), "chr")
                .and_then(|s| s.chars().next())
                .map(crate::symbols::combining)
                .unwrap_or('\u{0302}'),
            body: arg(n, "e"),
        },
        "bar" => Node::Bar {
            top: val(kid(n, "barPr"), "pos").as_deref() == Some("top"),
            body: arg(n, "e"),
        },
        "borderBox" => Node::Boxed { body: arg(n, "e") },
        "box" => Node::Group { body: arg(n, "e") },
        "groupChr" => {
            let pr = kid(n, "groupChrPr");
            Node::GroupChr {
                chr: chr(pr, "chr").and_then(|s| s.chars().next()).unwrap_or('⏟'),
                top: val(pr, "pos").as_deref() == Some("top"),
                body: arg(n, "e"),
            }
        }
        "limLow" | "limUpp" => Node::Limit {
            upper: local == "limUpp",
            base: arg(n, "e"),
            limit: arg(n, "lim"),
        },
        "func" => Node::Func {
            name: arg(n, "fName"),
            body: arg(n, "e"),
        },
        "phant" => {
            let pr = kid(n, "phantPr");
            Node::Phantom {
                body: arg(n, "e"),
                show: on_off(pr, "show").unwrap_or(true),
                zero_width: on_off(pr, "zeroWid").unwrap_or(false),
                zero_asc: on_off(pr, "zeroAsc").unwrap_or(false),
                zero_desc: on_off(pr, "zeroDesc").unwrap_or(false),
            }
        }
        // Properties and control properties hold no content.
        l if l.ends_with("Pr") => return,
        _ => {
            for c in n.children() {
                nodes(c, out);
            }
            return;
        }
    };
    push(out, node);
}

// ── writing ────────────────────────────────────────────────────────────

/// What kind of part the math goes in: run formatting is WordprocessingML
/// in Word documents, DrawingML in slides and shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Word,
    Drawing,
}

#[derive(Clone, Debug)]
pub struct WriteOptions {
    pub flavor: Flavor,
    /// The font math runs ask for (Word's default is Cambria Math).
    pub font: String,
    /// Declare `xmlns:m` on the outer element (PowerPoint does; Word
    /// declares it on the document).
    pub declare_ns: bool,
}

impl WriteOptions {
    pub fn word() -> Self {
        Self {
            flavor: Flavor::Word,
            font: "Cambria Math".into(),
            declare_ns: false,
        }
    }

    pub fn drawing() -> Self {
        Self {
            flavor: Flavor::Drawing,
            font: "Cambria Math".into(),
            declare_ns: true,
        }
    }
}

/// OMML for an equation: `m:oMathPara` for display math, else `m:oMath`.
pub fn write(eq: &Equation, o: &WriteOptions) -> String {
    let mut w = String::new();
    let ns = if o.declare_ns {
        format!(" xmlns:m=\"{NS_M}\"")
    } else {
        String::new()
    };
    if eq.display {
        w.push_str(&format!("<m:oMathPara{ns}>"));
        if eq.justify != Justify::CenterGroup {
            w.push_str(&format!(
                "<m:oMathParaPr><m:jc m:val=\"{}\"/></m:oMathParaPr>",
                eq.justify.omml()
            ));
        }
        for line in &eq.lines {
            w.push_str("<m:oMath>");
            write_row(&mut w, line, o);
            w.push_str("</m:oMath>");
        }
        w.push_str("</m:oMathPara>");
    } else {
        w.push_str(&format!("<m:oMath{ns}>"));
        for (i, line) in eq.lines.iter().enumerate() {
            if i > 0 {
                write_run(&mut w, &Run::new(" "), o);
            }
            write_row(&mut w, line, o);
        }
        w.push_str("</m:oMath>");
    }
    w
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

fn ctrl(w: &mut String, o: &WriteOptions) {
    match o.flavor {
        Flavor::Word => w.push_str(&format!(
            "<m:ctrlPr><w:rPr><w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\"/><w:i/></w:rPr></m:ctrlPr>",
            esc(&o.font)
        )),
        Flavor::Drawing => w.push_str(&format!(
            "<m:ctrlPr><a:rPr lang=\"en-US\" i=\"1\"><a:latin typeface=\"{}\"/></a:rPr></m:ctrlPr>",
            esc(&o.font)
        )),
    }
}

fn pr(w: &mut String, tag: &str, inner: &str, o: &WriteOptions) {
    w.push_str(&format!("<m:{tag}>{inner}"));
    ctrl(w, o);
    w.push_str(&format!("</m:{tag}>"));
}

fn arg_el(w: &mut String, tag: &str, r: &Row, o: &WriteOptions) {
    if r.is_empty() {
        w.push_str(&format!("<m:{tag}/>"));
    } else {
        w.push_str(&format!("<m:{tag}>"));
        write_row(w, r, o);
        w.push_str(&format!("</m:{tag}>"));
    }
}

fn mval(name: &str, v: &str) -> String {
    format!("<m:{name} m:val=\"{}\"/>", esc(v))
}

/// The OMML of a row's items.
pub fn write_row(w: &mut String, r: &Row, o: &WriteOptions) {
    for n in r {
        write_node(w, n, o);
    }
}

fn write_run(w: &mut String, r: &Run, o: &WriteOptions) {
    w.push_str("<m:r>");
    let s = &r.style;
    if !s.is_default() {
        w.push_str("<m:rPr>");
        if s.variant != Variant::Roman {
            w.push_str(&mval("scr", s.variant.omml()));
        }
        if let Some(wt) = s.weight
            && !(s.normal && wt == Weight::Plain)
        {
            w.push_str(&mval("sty", wt.omml()));
        }
        if s.normal {
            w.push_str("<m:nor/>");
        }
        w.push_str("</m:rPr>");
    }
    let italic = !s.normal && s.weight.is_none_or(|wt| wt.italic());
    let bold = s.weight.is_some_and(|wt| wt.bold());
    match o.flavor {
        Flavor::Word => {
            if s.normal {
                if bold {
                    w.push_str("<w:rPr><w:b/></w:rPr>");
                }
            } else {
                w.push_str(&format!(
                    "<w:rPr><w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\"/>{1}</w:rPr>",
                    esc(&o.font),
                    if bold { "<w:b/>" } else { "" }
                ));
            }
        }
        Flavor::Drawing => {
            let mut attrs = String::from(" lang=\"en-US\"");
            if italic {
                attrs.push_str(" i=\"1\"");
            }
            if bold {
                attrs.push_str(" b=\"1\"");
            }
            if s.normal {
                w.push_str(&format!("<a:rPr{attrs}/>"));
            } else {
                w.push_str(&format!(
                    "<a:rPr{attrs}><a:latin typeface=\"{}\"/></a:rPr>",
                    esc(&o.font)
                ));
            }
        }
    }
    let space = r.text.starts_with(char::is_whitespace) || r.text.ends_with(char::is_whitespace);
    if space {
        w.push_str("<m:t xml:space=\"preserve\">");
    } else {
        w.push_str("<m:t>");
    }
    w.push_str(&esc(&r.text));
    w.push_str("</m:t></m:r>");
}

fn write_node(w: &mut String, n: &Node, o: &WriteOptions) {
    match n {
        Node::Text(r) => write_run(w, r, o),
        Node::Frac { kind, num, den } => {
            w.push_str("<m:f>");
            let inner = if *kind == FracKind::Bar {
                String::new()
            } else {
                mval("type", kind.omml())
            };
            pr(w, "fPr", &inner, o);
            arg_el(w, "num", num, o);
            arg_el(w, "den", den, o);
            w.push_str("</m:f>");
        }
        Node::Scripts { base, sub, sup } => {
            let tag = match (sub, sup) {
                (Some(_), Some(_)) => "sSubSup",
                (Some(_), None) => "sSub",
                (None, Some(_)) => "sSup",
                (None, None) => {
                    write_row(w, base, o);
                    return;
                }
            };
            w.push_str(&format!("<m:{tag}>"));
            pr(w, &format!("{tag}Pr"), "", o);
            arg_el(w, "e", base, o);
            if let Some(s) = sub {
                arg_el(w, "sub", s, o);
            }
            if let Some(s) = sup {
                arg_el(w, "sup", s, o);
            }
            w.push_str(&format!("</m:{tag}>"));
        }
        Node::PreScripts { sub, sup, base } => {
            w.push_str("<m:sPre>");
            pr(w, "sPrePr", "", o);
            arg_el(w, "sub", sub, o);
            arg_el(w, "sup", sup, o);
            arg_el(w, "e", base, o);
            w.push_str("</m:sPre>");
        }
        Node::Radical { degree, body } => {
            w.push_str("<m:rad>");
            let inner = if degree.is_none() {
                mval("degHide", "1")
            } else {
                String::new()
            };
            pr(w, "radPr", &inner, o);
            arg_el(w, "deg", degree.as_ref().unwrap_or(&Row::new()), o);
            arg_el(w, "e", body, o);
            w.push_str("</m:rad>");
        }
        Node::Nary {
            op,
            under_over,
            sub,
            sup,
            body,
        } => {
            w.push_str("<m:nary>");
            let mut inner = String::new();
            if *op != '∫' {
                inner.push_str(&mval("chr", &op.to_string()));
            }
            inner.push_str(&mval(
                "limLoc",
                if *under_over { "undOvr" } else { "subSup" },
            ));
            if sub.is_none() {
                inner.push_str(&mval("subHide", "1"));
            }
            if sup.is_none() {
                inner.push_str(&mval("supHide", "1"));
            }
            pr(w, "naryPr", &inner, o);
            arg_el(w, "sub", sub.as_ref().unwrap_or(&Row::new()), o);
            arg_el(w, "sup", sup.as_ref().unwrap_or(&Row::new()), o);
            arg_el(w, "e", body, o);
            w.push_str("</m:nary>");
        }
        Node::Delim {
            open,
            close,
            sep,
            items,
            grow,
        } => {
            w.push_str("<m:d>");
            let mut inner = String::new();
            if open != "(" {
                inner.push_str(&mval("begChr", open));
            }
            if sep != "|" {
                inner.push_str(&mval("sepChr", sep));
            }
            if close != ")" {
                inner.push_str(&mval("endChr", close));
            }
            if !grow {
                inner.push_str(&mval("grow", "0"));
            }
            pr(w, "dPr", &inner, o);
            if items.is_empty() {
                w.push_str("<m:e/>");
            }
            for i in items {
                arg_el(w, "e", i, o);
            }
            w.push_str("</m:d>");
        }
        Node::Matrix { rows } => {
            let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
            w.push_str("<m:m>");
            pr(
                w,
                "mPr",
                &format!(
                    "<m:mcs><m:mc><m:mcPr>{}{}</m:mcPr></m:mc></m:mcs>",
                    mval("count", &cols.to_string()),
                    mval("mcJc", "center")
                ),
                o,
            );
            for r in rows {
                w.push_str("<m:mr>");
                for c in 0..cols {
                    arg_el(w, "e", r.get(c).unwrap_or(&Row::new()), o);
                }
                w.push_str("</m:mr>");
            }
            w.push_str("</m:m>");
        }
        Node::EqArray { rows } => {
            w.push_str("<m:eqArr>");
            pr(w, "eqArrPr", "", o);
            for r in rows {
                arg_el(w, "e", r, o);
            }
            w.push_str("</m:eqArr>");
        }
        Node::Accent { chr, body } => {
            w.push_str("<m:acc>");
            let inner = if *chr == '\u{0302}' {
                String::new()
            } else {
                mval("chr", &chr.to_string())
            };
            pr(w, "accPr", &inner, o);
            arg_el(w, "e", body, o);
            w.push_str("</m:acc>");
        }
        Node::Bar { top, body } => {
            w.push_str("<m:bar>");
            pr(
                w,
                "barPr",
                &mval("pos", if *top { "top" } else { "bot" }),
                o,
            );
            arg_el(w, "e", body, o);
            w.push_str("</m:bar>");
        }
        Node::Boxed { body } => {
            w.push_str("<m:borderBox>");
            pr(w, "borderBoxPr", "", o);
            arg_el(w, "e", body, o);
            w.push_str("</m:borderBox>");
        }
        Node::Group { body } => {
            w.push_str("<m:box>");
            pr(w, "boxPr", "", o);
            arg_el(w, "e", body, o);
            w.push_str("</m:box>");
        }
        Node::GroupChr { chr, top, body } => {
            w.push_str("<m:groupChr>");
            let mut inner = String::new();
            if *chr != '⏟' {
                inner.push_str(&mval("chr", &chr.to_string()));
            }
            if *top {
                inner.push_str(&mval("pos", "top"));
                inner.push_str(&mval("vertJc", "bot"));
            }
            pr(w, "groupChrPr", &inner, o);
            arg_el(w, "e", body, o);
            w.push_str("</m:groupChr>");
        }
        Node::Limit { upper, base, limit } => {
            let tag = if *upper { "limUpp" } else { "limLow" };
            w.push_str(&format!("<m:{tag}>"));
            pr(w, &format!("{tag}Pr"), "", o);
            arg_el(w, "e", base, o);
            arg_el(w, "lim", limit, o);
            w.push_str(&format!("</m:{tag}>"));
        }
        Node::Func { name, body } => {
            w.push_str("<m:func>");
            pr(w, "funcPr", "", o);
            // Function names are upright.
            let name: Row = name
                .iter()
                .map(|n| match n {
                    Node::Text(r) if r.style.is_default() => {
                        Node::Text(Run::styled(r.text.clone(), Style::PLAIN))
                    }
                    n => upright_limits(n),
                })
                .collect();
            arg_el(w, "fName", &name, o);
            arg_el(w, "e", body, o);
            w.push_str("</m:func>");
        }
        Node::Phantom {
            body,
            show,
            zero_width,
            zero_asc,
            zero_desc,
        } => {
            w.push_str("<m:phant>");
            let mut inner = String::new();
            if !show {
                inner.push_str(&mval("show", "0"));
            }
            if *zero_width {
                inner.push_str(&mval("zeroWid", "1"));
            }
            if *zero_asc {
                inner.push_str(&mval("zeroAsc", "1"));
            }
            if *zero_desc {
                inner.push_str(&mval("zeroDesc", "1"));
            }
            pr(w, "phantPr", &inner, o);
            arg_el(w, "e", body, o);
            w.push_str("</m:phant>");
        }
    }
}

/// `lim` under its limit in a function name: the name upright.
fn upright_limits(n: &Node) -> Node {
    match n {
        Node::Limit { upper, base, limit } => Node::Limit {
            upper: *upper,
            base: base
                .iter()
                .map(|b| match b {
                    Node::Text(r) if r.style.is_default() => {
                        Node::Text(Run::styled(r.text.clone(), Style::PLAIN))
                    }
                    b => b.clone(),
                })
                .collect(),
            limit: limit.clone(),
        },
        Node::Scripts { base, sub, sup } => Node::Scripts {
            base: base
                .iter()
                .map(|b| match b {
                    Node::Text(r) if r.style.is_default() => {
                        Node::Text(Run::styled(r.text.clone(), Style::PLAIN))
                    }
                    b => b.clone(),
                })
                .collect(),
            sub: sub.clone(),
            sup: sup.clone(),
        },
        n => n.clone(),
    }
}
