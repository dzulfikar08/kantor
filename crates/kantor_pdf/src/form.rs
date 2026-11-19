//! Interactive forms (AcroForm, ISO 32000-1 §12.7): reading the fields,
//! filling them, and writing their appearances so every viewer shows the
//! values without regenerating them (no `NeedAppearances`).
//!
//! Text is set in Helvetica (WinAnsi): characters outside Latin-1 show as
//! `?` in the appearance (the value itself keeps them). XFA-only forms have
//! no AcroForm fields to fill: [`Form::xfa_only`].

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::annot::{Font, parse_da, tj_string, wrap};
use crate::pages::{number, page_ids, rect};
use crate::pdfstr::{decode, get_text, num, text};
use crate::{Pdf, Result};

/// What kind of field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text {
        multiline: bool,
        password: bool,
        max_len: Option<usize>,
    },
    Check,
    Radio,
    /// A drop-down; `edit`: the user may type a value of their own.
    Combo {
        edit: bool,
    },
    List {
        multi: bool,
    },
    /// A push button (nothing to fill).
    Push,
    /// A signature field (certificate signatures aren't supported).
    Signature,
}

/// A field's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Text(String),
    /// A check box or radio button's state: `Off` or its on-state name.
    State(String),
    /// Chosen options of a list or drop-down (export values).
    Choice(Vec<String>),
}

/// One widget (the field's box on a page).
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// The field's full name.
    pub field: String,
    pub page: u32,
    /// Its place in the page's `/Annots`.
    pub annot_index: usize,
    pub rect: [f32; 4],
    /// The on-state of a check box or radio button.
    pub on_state: Option<String>,
    pub(crate) da: String,
    pub(crate) quadding: i64,
    pub(crate) background: Option<[f32; 3]>,
    pub(crate) border: Option<[f32; 3]>,
    pub(crate) kind: FieldKind,
    /// (export, shown) options of a choice field.
    pub(crate) options: Vec<(String, String)>,
    /// Quarter turns of its content (`/MK /R`).
    pub(crate) turn: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: FieldValue,
    pub default: FieldValue,
    /// (export, shown) options of a choice field.
    pub options: Vec<(String, String)>,
    pub read_only: bool,
    /// Its tooltip (`/TU`).
    pub tooltip: String,
    pub widgets: Vec<Widget>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub fields: Vec<Field>,
    /// The form has an XFA part.
    pub xfa: bool,
}

impl Form {
    /// Only XFA: nothing Kantor can fill.
    pub fn xfa_only(&self) -> bool {
        // A signature field or button beside the XFA is nothing to fill.
        self.xfa
            && self
                .fields
                .iter()
                .all(|f| f.widgets.is_empty() || !fillable(&f.kind))
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Every field at its default value.
    pub fn defaults(&self) -> HashMap<String, FieldValue> {
        self.fields
            .iter()
            .filter(|f| fillable(&f.kind))
            .map(|f| (f.name.clone(), f.default.clone()))
            .collect()
    }
}

fn fillable(k: &FieldKind) -> bool {
    !matches!(k, FieldKind::Push | FieldKind::Signature)
}

const FF_READ_ONLY: i64 = 1;
const FF_MULTILINE: i64 = 1 << 12;
const FF_PASSWORD: i64 = 1 << 13;
const FF_RADIO: i64 = 1 << 15;
const FF_PUSH: i64 = 1 << 16;
const FF_COMBO: i64 = 1 << 17;
const FF_EDIT: i64 = 1 << 18;
const FF_MULTI: i64 = 1 << 21;

fn acro_form(doc: &Document) -> Option<Dictionary> {
    let f = doc.catalog().ok()?.get(b"AcroForm").ok()?;
    doc.dereference(f).ok()?.1.as_dict().ok().cloned()
}

fn deref_dict(doc: &Document, o: &Object) -> Option<Dictionary> {
    doc.dereference(o).ok()?.1.as_dict().ok().cloned()
}

fn colors(doc: &Document, mk: &Dictionary, key: &[u8]) -> Option<[f32; 3]> {
    let a = doc.dereference(mk.get(key).ok()?).ok()?.1.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(|o| number(doc, o)).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => {
            let k = 1.0 - v[3];
            Some([(1.0 - v[0]) * k, (1.0 - v[1]) * k, (1.0 - v[2]) * k])
        }
        _ => None,
    }
}

/// Inherited field attributes while walking down the tree.
#[derive(Clone, Default)]
struct Inherit {
    name: String,
    ft: Option<Vec<u8>>,
    ff: i64,
    v: Option<Object>,
    dv: Option<Object>,
    da: Option<String>,
    q: i64,
    opt: Option<Object>,
    max_len: Option<usize>,
    tu: Option<String>,
}

/// The form's fields, or `None` when the document has no form.
pub fn read_form(pdf: &Pdf) -> Option<Form> {
    let doc = &pdf.doc;
    let af = acro_form(doc)?;
    let xfa = af.has(b"XFA");
    let default_da = get_text(doc, &af, b"DA").unwrap_or_else(|| "/Helv 0 Tf 0 g".into());
    // Where every annotation is: object id → (page, index).
    let mut places: HashMap<ObjectId, (u32, usize)> = HashMap::new();
    for (p, pid) in page_ids(doc).into_iter().enumerate() {
        for (i, (id, _)) in crate::annot::page_annots(doc, pid).into_iter().enumerate() {
            if let Some(id) = id {
                places.insert(id, (p as u32, i));
            }
        }
    }
    let mut fields = Vec::new();
    let roots = af
        .get(b"Fields")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default();
    let base = Inherit {
        da: Some(default_da),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    for r in roots {
        walk(doc, &r, &base, &places, &mut fields, &mut seen, 0);
    }
    Some(Form { fields, xfa })
}

fn walk(
    doc: &Document,
    node: &Object,
    up: &Inherit,
    places: &HashMap<ObjectId, (u32, usize)>,
    out: &mut Vec<Field>,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) {
    if depth > 32 {
        return;
    }
    let id = node.as_reference().ok();
    if let Some(id) = id
        && !seen.insert(id)
    {
        return;
    }
    let Some(d) = deref_dict(doc, node) else {
        return;
    };
    let mut h = up.clone();
    if let Some(t) = get_text(doc, &d, b"T") {
        h.name = if h.name.is_empty() {
            t
        } else {
            format!("{}.{t}", h.name)
        };
    }
    if let Ok(ft) = d.get(b"FT").and_then(Object::as_name) {
        h.ft = Some(ft.to_vec());
    }
    if let Some(ff) = d.get(b"Ff").ok().and_then(|o| number(doc, o)) {
        h.ff = ff as i64;
    }
    if let Ok(v) = d.get(b"V") {
        h.v = Some(v.clone());
    }
    if let Ok(v) = d.get(b"DV") {
        h.dv = Some(v.clone());
    }
    if let Some(da) = get_text(doc, &d, b"DA") {
        h.da = Some(da);
    }
    if let Some(q) = d.get(b"Q").ok().and_then(|o| number(doc, o)) {
        h.q = q as i64;
    }
    if let Ok(o) = d.get(b"Opt") {
        h.opt = Some(o.clone());
    }
    if let Some(m) = d.get(b"MaxLen").ok().and_then(|o| number(doc, o)) {
        h.max_len = Some(m as usize);
    }
    if let Some(t) = get_text(doc, &d, b"TU") {
        h.tu = Some(t);
    }
    let kids: Vec<Object> = d
        .get(b"Kids")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default();
    // Kids with a /T are fields; without, widgets of this field.
    let field_kids: Vec<&Object> = kids
        .iter()
        .filter(|k| deref_dict(doc, k).is_some_and(|k| k.has(b"T")))
        .collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk(doc, k, &h, places, out, seen, depth + 1);
        }
        return;
    }
    let widgets: Vec<Object> = if kids.is_empty() {
        id.map(Object::Reference).into_iter().collect()
    } else {
        kids
    };
    let ft = h.ft.clone().unwrap_or_default();
    let kind = match ft.as_slice() {
        b"Tx" => FieldKind::Text {
            multiline: h.ff & FF_MULTILINE != 0,
            password: h.ff & FF_PASSWORD != 0,
            max_len: h.max_len,
        },
        b"Btn" if h.ff & FF_PUSH != 0 => FieldKind::Push,
        b"Btn" if h.ff & FF_RADIO != 0 => FieldKind::Radio,
        b"Btn" => FieldKind::Check,
        b"Ch" if h.ff & FF_COMBO != 0 => FieldKind::Combo {
            edit: h.ff & FF_EDIT != 0,
        },
        b"Ch" => FieldKind::List {
            multi: h.ff & FF_MULTI != 0,
        },
        b"Sig" => FieldKind::Signature,
        _ => return,
    };
    let options: Vec<(String, String)> = h
        .opt
        .as_ref()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| match doc.dereference(o).ok()?.1 {
            Object::String(s, _) => {
                let s = decode(s);
                Some((s.clone(), s))
            }
            Object::Array(pair) if pair.len() == 2 => {
                let e = match doc.dereference(&pair[0]).ok()?.1 {
                    Object::String(s, _) => decode(s),
                    _ => return None,
                };
                let d = match doc.dereference(&pair[1]).ok()?.1 {
                    Object::String(s, _) => decode(s),
                    _ => e.clone(),
                };
                Some((e, d))
            }
            _ => None,
        })
        .collect();
    let value_of = |o: &Option<Object>| -> FieldValue {
        let o = o
            .as_ref()
            .and_then(|o| doc.dereference(o).ok())
            .map(|(_, o)| o.clone());
        match (&kind, o) {
            (FieldKind::Check | FieldKind::Radio, Some(Object::Name(n))) => {
                FieldValue::State(String::from_utf8_lossy(&n).into_owned())
            }
            (FieldKind::Check | FieldKind::Radio, _) => FieldValue::State("Off".into()),
            (FieldKind::Combo { .. } | FieldKind::List { .. }, Some(Object::String(s, _))) => {
                FieldValue::Choice(vec![decode(&s)])
            }
            (FieldKind::Combo { .. } | FieldKind::List { .. }, Some(Object::Array(a))) => {
                FieldValue::Choice(
                    a.iter()
                        .filter_map(|o| match o {
                            Object::String(s, _) => Some(decode(s)),
                            _ => None,
                        })
                        .collect(),
                )
            }
            (FieldKind::Combo { .. } | FieldKind::List { .. }, _) => FieldValue::Choice(Vec::new()),
            (_, Some(Object::String(s, _))) => FieldValue::Text(decode(&s)),
            (_, Some(Object::Stream(s))) => {
                FieldValue::Text(decode(&s.decompressed_content().unwrap_or_default()))
            }
            _ => FieldValue::Text(String::new()),
        }
    };
    let mut value = value_of(&h.v);
    let default = value_of(&h.dv);
    let mut ws = Vec::new();
    for w in &widgets {
        let Some(wid) = w.as_reference().ok() else {
            continue;
        };
        let Some(wd) = deref_dict(doc, w) else {
            continue;
        };
        let Some(&(page, annot_index)) = places.get(&wid) else {
            continue;
        };
        let r = wd
            .get(b"Rect")
            .ok()
            .and_then(|o| rect(doc, o))
            .unwrap_or([0.0; 4]);
        let mk = wd
            .get(b"MK")
            .ok()
            .and_then(|o| deref_dict(doc, o))
            .unwrap_or_default();
        let on_state = wd
            .get(b"AP")
            .ok()
            .and_then(|o| deref_dict(doc, o))
            .and_then(|ap| ap.get(b"N").ok().and_then(|n| deref_dict(doc, n)))
            .and_then(|n| {
                n.iter()
                    .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
                    .find(|k| k != "Off")
            })
            .or_else(|| {
                matches!(kind, FieldKind::Check | FieldKind::Radio).then(|| "Yes".to_owned())
            });
        // A check box without /V shows its /AS.
        if matches!(kind, FieldKind::Check)
            && h.v.is_none()
            && let Ok(Object::Name(n)) = wd.get(b"AS")
            && n != b"Off"
        {
            value = FieldValue::State(String::from_utf8_lossy(n).into_owned());
        }
        ws.push(Widget {
            field: h.name.clone(),
            page,
            annot_index,
            rect: r,
            on_state: if matches!(kind, FieldKind::Check | FieldKind::Radio) {
                on_state
            } else {
                None
            },
            da: get_text(doc, &wd, b"DA")
                .or_else(|| h.da.clone())
                .unwrap_or_default(),
            quadding: wd
                .get(b"Q")
                .ok()
                .and_then(|o| number(doc, o))
                .map_or(h.q, |q| q as i64),
            background: colors(doc, &mk, b"BG"),
            border: colors(doc, &mk, b"BC"),
            kind: kind.clone(),
            options: options.clone(),
            turn: mk
                .get(b"R")
                .ok()
                .and_then(|o| number(doc, o))
                .map_or(0, |r| (r as i64).rem_euclid(360) / 90),
        });
    }
    out.push(Field {
        name: h.name,
        kind,
        value,
        default,
        options,
        read_only: h.ff & FF_READ_ONLY != 0,
        tooltip: h.tu.unwrap_or_default(),
        widgets: ws,
    });
}

/// The widgets on page `page`.
pub(crate) fn page_widgets(pdf: &Pdf, page: u32) -> Vec<Widget> {
    read_form(pdf)
        .map(|f| {
            f.fields
                .into_iter()
                .flat_map(|f| f.widgets)
                .filter(|w| w.page == page)
                .collect()
        })
        .unwrap_or_default()
}

/// Set `d` (a widget's dictionary in `doc`) to show `value`.
pub(crate) fn widget_look(
    doc: &mut Document,
    w: &Widget,
    value: &FieldValue,
    d: &mut Dictionary,
) -> Result<()> {
    match (&w.kind, value) {
        (FieldKind::Check | FieldKind::Radio, FieldValue::State(s)) => {
            let on = w.on_state.clone().unwrap_or_else(|| "Yes".into());
            let state = if *s == on { on.clone() } else { "Off".into() };
            let has_ap = d
                .get(b"AP")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_dict().ok())
                .and_then(|ap| ap.get(b"N").ok())
                .and_then(|n| doc.dereference(n).ok())
                .is_some_and(|(_, n)| n.as_dict().is_ok_and(|n| n.has(on.as_bytes())));
            if !has_ap {
                let mut n = Dictionary::new();
                n.set(on.clone(), doc.add_object(check_ap(w, true)));
                n.set("Off", doc.add_object(check_ap(w, false)));
                let mut ap = Dictionary::new();
                ap.set("N", n);
                d.set("AP", ap);
            }
            d.set("AS", Object::Name(state.into_bytes()));
        }
        (FieldKind::Text { .. } | FieldKind::Combo { .. } | FieldKind::List { .. }, v) => {
            let ap = text_ap(doc, w, v);
            let id = doc.add_object(ap);
            let mut apd = Dictionary::new();
            apd.set("N", id);
            d.set("AP", apd);
        }
        _ => {}
    }
    Ok(())
}

fn rgb(c: [f32; 3], op: &str) -> String {
    format!("{} {} {} {op}\n", num(c[0]), num(c[1]), num(c[2]))
}

/// Background and border of a widget box `w` × `h`.
fn frame(w: &Widget, bw: f32, bh: f32) -> String {
    let mut s = String::new();
    if let Some(bg) = w.background {
        s.push_str(&rgb(bg, "rg"));
        s.push_str(&format!("0 0 {} {} re f\n", num(bw), num(bh)));
    }
    if let Some(bc) = w.border {
        s.push_str(&rgb(bc, "RG"));
        s.push_str(&format!(
            "1 w 0.5 0.5 {} {} re S\n",
            num(bw - 1.0),
            num(bh - 1.0)
        ));
    }
    s
}

fn bbox_obj(bw: f32, bh: f32) -> Object {
    Object::Array(vec![0.into(), 0.into(), Object::Real(bw), Object::Real(bh)])
}

/// The size of the widget's content box (turned by /MK /R) and the matrix.
fn turned(w: &Widget) -> (f32, f32, Option<Object>) {
    let (rw, rh) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    match w.turn {
        1 => (
            rh,
            rw,
            Some(Object::Array(vec![
                0.into(),
                1.into(),
                (-1).into(),
                0.into(),
                Object::Real(rw),
                0.into(),
            ])),
        ),
        2 => (
            rw,
            rh,
            Some(Object::Array(vec![
                (-1).into(),
                0.into(),
                0.into(),
                (-1).into(),
                Object::Real(rw),
                Object::Real(rh),
            ])),
        ),
        3 => (
            rh,
            rw,
            Some(Object::Array(vec![
                0.into(),
                (-1).into(),
                1.into(),
                0.into(),
                0.into(),
                Object::Real(rh),
            ])),
        ),
        _ => (rw, rh, None),
    }
}

fn check_ap(w: &Widget, on: bool) -> Stream {
    let (bw, bh, m) = turned(w);
    let mut s = frame(w, bw, bh);
    if on {
        let (_, _, c) = parse_da(&w.da);
        s.push_str(&rgb(c, "rg"));
        if w.kind == FieldKind::Radio {
            let r = bw.min(bh) * 0.25;
            let (cx, cy) = (bw / 2.0, bh / 2.0);
            let k = r * 0.5523;
            s.push_str(&format!(
                "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f\n",
                num(cx + r), num(cy),
                num(cx + r), num(cy + k), num(cx + k), num(cy + r), num(cx), num(cy + r),
                num(cx - k), num(cy + r), num(cx - r), num(cy + k), num(cx - r), num(cy),
                num(cx - r), num(cy - k), num(cx - k), num(cy - r), num(cx), num(cy - r),
                num(cx + k), num(cy - r), num(cx + r), num(cy - k), num(cx + r), num(cy),
            ));
        } else {
            let (x, y) = (bw, bh);
            s.push_str(&rgb(c, "RG"));
            s.push_str(&format!(
                "{} w 1 J 1 j {} {} m {} {} l {} {} l S\n",
                num(bw.min(bh) * 0.12),
                num(x * 0.2),
                num(y * 0.5),
                num(x * 0.42),
                num(y * 0.25),
                num(x * 0.8),
                num(y * 0.78)
            ));
        }
    }
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", bbox_obj(bw, bh));
    if let Some(m) = m {
        d.set("Matrix", m);
    }
    d.set("Resources", Dictionary::new());
    Stream::new(d, s.into_bytes())
}

fn text_ap(doc: &mut Document, w: &Widget, v: &FieldValue) -> Stream {
    let (bw, bh, m) = turned(w);
    let (_, size, c) = parse_da(&w.da);
    let font = Font::Helvetica;
    let shown = |e: &str| -> String {
        w.options
            .iter()
            .find(|(x, _)| x == e)
            .map_or_else(|| e.to_owned(), |(_, d)| d.clone())
    };
    let mut s = frame(w, bw, bh);
    s.push_str("/Tx BMC q\n");
    s.push_str(&format!("1 1 {} {} re W n\n", num(bw - 2.0), num(bh - 2.0)));
    let pad = 2.0;
    match (&w.kind, v) {
        (FieldKind::List { .. }, FieldValue::Choice(sel)) => {
            let size = if size <= 0.0 { 12.0 } else { size };
            let lead = size * 1.15;
            for (i, (e, d)) in w.options.iter().enumerate() {
                let y = bh - pad - lead * (i as f32 + 1.0);
                if y < -lead {
                    break;
                }
                if sel.contains(e) {
                    s.push_str(&format!(
                        "0.6 0.75 0.94 rg 1 {} {} {} re f\n",
                        num(y - size * 0.25),
                        num(bw - 2.0),
                        num(lead)
                    ));
                }
                s.push_str(&format!(
                    "BT /Helv {} Tf {}{} {} Td {} Tj ET\n",
                    num(size),
                    rgb(c, "rg"),
                    num(pad),
                    num(y + size * 0.05),
                    tj_string(d)
                ));
            }
        }
        _ => {
            let value = match v {
                FieldValue::Text(t) => t.clone(),
                FieldValue::Choice(c) => c.first().map(|e| shown(e)).unwrap_or_default(),
                FieldValue::State(_) => String::new(),
            };
            let (multiline, password) = match w.kind {
                FieldKind::Text {
                    multiline,
                    password,
                    ..
                } => (multiline, password),
                _ => (false, false),
            };
            let value = if password {
                "*".repeat(value.chars().count())
            } else {
                value
            };
            let size = if size > 0.0 {
                size
            } else if multiline {
                12.0
            } else {
                // Auto size: fit the height, then the width.
                let by_h = ((bh - 2.0 * pad) / 1.15).clamp(4.0, 12.0);
                let wd = font.width(&value, 1.0);
                if wd > 0.0 {
                    by_h.min((bw - 2.0 * pad) / wd).max(4.0)
                } else {
                    by_h
                }
            };
            let lines = if multiline {
                wrap(&value, font, size, bw - 2.0 * pad)
            } else {
                vec![value]
            };
            let lead = size * 1.15;
            for (i, l) in lines.iter().enumerate() {
                let lw = font.width(l, size);
                let x = match w.quadding {
                    1 => (bw - lw) / 2.0,
                    2 => bw - pad - lw,
                    _ => pad,
                };
                let y = if multiline {
                    bh - pad - size * 0.9 - lead * i as f32
                } else {
                    (bh - size * 0.75) / 2.0 + size * 0.05
                };
                s.push_str(&format!(
                    "BT /Helv {} Tf {}{} {} Td {} Tj ET\n",
                    num(size),
                    rgb(c, "rg"),
                    num(x),
                    num(y),
                    tj_string(l)
                ));
            }
        }
    }
    s.push_str("Q EMC\n");
    let mut font_d = Dictionary::new();
    font_d.set("Type", "Font");
    font_d.set("Subtype", "Type1");
    font_d.set("BaseFont", "Helvetica");
    font_d.set("Encoding", "WinAnsiEncoding");
    let font_id = doc.add_object(font_d);
    let mut fonts = Dictionary::new();
    fonts.set("Helv", font_id);
    let mut res = Dictionary::new();
    res.set("Font", fonts);
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", bbox_obj(bw, bh));
    if let Some(m) = m {
        d.set("Matrix", m);
    }
    d.set("Resources", res);
    let mut st = Stream::new(d, s.into_bytes());
    let _ = st.compress();
    st
}

/// Fill the form: each named field gets its value (`/V`) and every widget
/// of it a new appearance. Fields not named keep theirs. A hybrid form's
/// XFA part is dropped, so viewers show the values filled here.
pub fn set_values(pdf: &mut Pdf, values: &HashMap<String, FieldValue>) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let Some(form) = read_form(pdf) else {
        return Ok(());
    };
    // Field objects by full name: walk again to find their ids.
    let ids = field_ids(&pdf.doc);
    let pages = page_ids(&pdf.doc);
    for f in &form.fields {
        let Some(v) = values.get(&f.name) else {
            continue;
        };
        if let Some(&fid) = ids.get(&f.name) {
            let obj = match v {
                FieldValue::Text(t) => text(t),
                FieldValue::State(s) => Object::Name(s.clone().into_bytes()),
                FieldValue::Choice(c) if c.len() == 1 => text(&c[0]),
                FieldValue::Choice(c) => Object::Array(c.iter().map(|s| text(s)).collect()),
            };
            if let Ok(d) = pdf.doc.get_dictionary_mut(fid) {
                d.set("V", obj);
            }
        }
        for w in &f.widgets {
            let Some(&pid) = pages.get(w.page as usize) else {
                continue;
            };
            let Some((Some(wid), mut d)) = crate::annot::page_annots(&pdf.doc, pid)
                .into_iter()
                .nth(w.annot_index)
            else {
                continue;
            };
            widget_look(&mut pdf.doc, w, v, &mut d)?;
            pdf.doc.objects.insert(wid, Object::Dictionary(d));
        }
    }
    if let Ok(cat) = pdf.doc.catalog()
        && let Ok(Object::Reference(af)) = cat.get(b"AcroForm").cloned()
        && let Ok(d) = pdf.doc.get_dictionary_mut(af)
    {
        d.remove(b"XFA");
    } else if let Ok(cat) = pdf.doc.catalog_mut()
        && let Ok(Object::Dictionary(d)) = cat.get_mut(b"AcroForm")
    {
        d.remove(b"XFA");
    }
    Ok(())
}

/// Full name → object id of every terminal field.
fn field_ids(doc: &Document) -> HashMap<String, ObjectId> {
    let mut out = HashMap::new();
    let Some(af) = acro_form(doc) else {
        return out;
    };
    fn go(
        doc: &Document,
        o: &Object,
        prefix: &str,
        out: &mut HashMap<String, ObjectId>,
        depth: u32,
    ) {
        if depth > 32 {
            return;
        }
        let Ok(id) = o.as_reference() else { return };
        let Some(d) = deref_dict(doc, o) else { return };
        let name = match get_text(doc, &d, b"T") {
            Some(t) if prefix.is_empty() => t,
            Some(t) => format!("{prefix}.{t}"),
            None => prefix.to_owned(),
        };
        let kids: Vec<Object> = d
            .get(b"Kids")
            .ok()
            .and_then(|k| doc.dereference(k).ok())
            .and_then(|(_, k)| k.as_array().ok().cloned())
            .unwrap_or_default();
        let field_kids: Vec<&Object> = kids
            .iter()
            .filter(|k| deref_dict(doc, k).is_some_and(|k| k.has(b"T")))
            .collect();
        if field_kids.is_empty() {
            out.entry(name).or_insert(id);
        } else {
            for k in field_kids {
                go(doc, k, &name, out, depth + 1);
            }
        }
    }
    if let Ok(Object::Array(roots)) = af.get(b"Fields").map(|o| {
        doc.dereference(o)
            .map(|(_, o)| o.clone())
            .unwrap_or(Object::Null)
    }) {
        for r in &roots {
            go(doc, r, "", &mut out, 0);
        }
    }
    out
}
