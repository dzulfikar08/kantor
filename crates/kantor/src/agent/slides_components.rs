//! Components: a row of stat cards, a grid of cards, steps, a timeline, a
//! callout, a numbered list, a comparison, a badge, and the `clean` table
//! preset. Each is laid out in its box (by default the slide's content area
//! under the header) and expanded into ordinary elements before the deck is
//! built, so the result is plain shapes and text boxes anyone can edit.
//! Sizes come from the deck's text styles, colours from its palette
//! ([`super::slides_style`]).

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};
use slides_engine::Emu;

use super::slides_author::{Ctx, keys_with, length, object, opt_bool, opt_str};
use super::slides_style::{CardStyle, Look, look_of};

/// Component `type`s.
pub const COMPONENTS: [&str; 8] = [
    "stat_cards",
    "cards",
    "steps",
    "timeline",
    "callout",
    "numbered_list",
    "comparison",
    "badge",
];

const IN: f64 = 914_400.0;

fn inch(v: f64) -> Emu {
    (v * IN).round() as Emu
}

fn e(v: Emu) -> Value {
    json!(format!("{v}emu"))
}

/// A text box with no insets.
fn text(name: &str, x: Emu, y: Emu, w: Emu, h: Emu, text: Value) -> Value {
    json!({"type": "text", "name": name, "x": e(x), "y": e(y), "w": e(w), "h": e(h.max(1)),
           "insets": 0, "text": text})
}

#[allow(clippy::too_many_arguments)]
fn shape(
    name: &str,
    geometry: &str,
    x: Emu,
    y: Emu,
    w: Emu,
    h: Emu,
    fill: Value,
    line: Value,
) -> Value {
    json!({"type": "shape", "name": name, "geometry": geometry, "x": e(x), "y": e(y), "w": e(w.max(1)), "h": e(h.max(1)),
           "fill": fill, "line": line})
}

/// A straight line.
fn rule(name: &str, x1: Emu, y1: Emu, x2: Emu, y2: Emu, color: &str, width: &str) -> Value {
    json!({"type": "line", "name": name, "x1": e(x1), "y1": e(y1), "x2": e(x2), "y2": e(y2),
           "color": color, "width": width})
}

/// The rule on top of a card in the rule look: 1pt ink, 2.5pt accent when
/// highlighted.
fn top_rule(name: &str, x: Emu, y: Emu, w: Emu, hi: bool) -> Value {
    if hi {
        rule(name, x, y, x + w, y, "accent", "2.5pt")
    } else {
        rule(name, x, y, x + w, y, "ink", "1pt")
    }
}

/// A paragraph in `style`, with more settings.
fn para(t: &str, style: &str, more: Value) -> Value {
    let mut p = json!({"text": t, "style": style});
    if let (Value::Object(p), Value::Object(m)) = (&mut p, more) {
        p.extend(m);
    }
    p
}

/// A string or number setting as text.
fn text_of(o: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    Ok(match o.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(_) => bail!("`{key}` must be text"),
    })
}

/// The component's box: `x`, `y`, `w`, `h`, defaulting to the content area.
fn area(o: &Map<String, Value>, cx: &Ctx, look: &Look) -> Result<([Emu; 4], [bool; 4])> {
    let mut a = cx.area.unwrap_or(look.content);
    let mut given = [false; 4];
    for (i, k) in ["x", "y", "w", "h"].iter().enumerate() {
        if let Some(v) = o.get(*k).filter(|v| !v.is_null()) {
            a[i] = length(v, cx, k)?;
            given[i] = true;
        }
    }
    // A box moved right or down without a size keeps to the slide.
    if given[0] && !given[2] {
        let content = cx.area.unwrap_or(look.content);
        a[2] = (content[0] + content[2] - a[0]).max(inch(1.0));
    }
    if given[1] && !given[3] {
        let content = cx.area.unwrap_or(look.content);
        a[3] = (content[1] + content[3] - a[1]).max(inch(0.5));
    }
    Ok((a, given))
}

fn gap(o: &Map<String, Value>, cx: &Ctx, look: &Look) -> Result<Emu> {
    match o.get("gap").filter(|v| !v.is_null()) {
        Some(g) => Ok(length(g, cx, "gap")?.max(0)),
        None => Ok(look.gap),
    }
}

/// The `items` array (`min` to `max` of them).
fn items<'a>(
    o: &'a Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<&'a Vec<Value>> {
    let list = o
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("`{key}` is required: an array"))?;
    if list.len() < min || list.len() > max {
        bail!("`{key}` needs {min} to {max} entries");
    }
    Ok(list)
}

/// An item as an object (a bare string is its `title`).
fn item<'a>(
    v: &'a Value,
    what: &str,
    keys: &[&str],
    owned: &'a mut Map<String, Value>,
) -> Result<&'a Map<String, Value>> {
    match v {
        Value::String(s) => {
            owned.insert("title".into(), json!(s));
            Ok(owned)
        }
        _ => object(v, what, keys),
    }
}

/// `01`, `02`, ... (or `1`, `2` with `"numbers": "1"`).
fn number(o: &Map<String, Value>, i: usize) -> Result<String> {
    let start = o.get("start").and_then(Value::as_u64).unwrap_or(1) as usize;
    let n = start + i;
    Ok(match opt_str(o, "numbers")?.unwrap_or("01") {
        "01" => format!("{n:02}"),
        "1" => n.to_string(),
        other => bail!("`numbers` must be \"01\" or \"1\", not \"{other}\""),
    })
}

/// Text colours on a filled card: heading and body.
type OnFill = Option<(&'static str, &'static str)>;

/// Paragraphs for an item's `title`, `text` and `bullets`.
fn body(
    io: &Map<String, Value>,
    title_style: &str,
    text_style: &str,
    on: OnFill,
) -> Result<Vec<Value>> {
    let mut paras = Vec::new();
    let (tc, bc) = match on {
        Some((t, b)) => (json!({"color": t}), json!({"color": b})),
        None => (json!({}), json!({})),
    };
    if let Some(t) = text_of(io, "title")? {
        paras.push(para(&t, title_style, tc.clone()));
    }
    let first = paras.is_empty();
    if let Some(t) = text_of(io, "text")? {
        let mut more = bc.clone();
        if !first {
            more["space_before"] = json!(4);
        }
        paras.push(para(&t, text_style, more));
    }
    if let Some(b) = io.get("bullets").filter(|v| !v.is_null()) {
        let list = b
            .as_array()
            .context("`bullets` must be an array of strings")?;
        for (k, x) in list.iter().enumerate() {
            let t = x
                .as_str()
                .context("`bullets` must be an array of strings")?;
            let mut more = bc.clone();
            more["bullet"] = json!(true);
            more["space_before"] = json!(if k == 0 && first { 0 } else { 4 });
            paras.push(para(t, text_style, more));
        }
    }
    if paras.is_empty() {
        paras.push(json!(""));
    }
    Ok(paras)
}

/// A square badge with a number or an icon (the fill look's marker).
#[allow(clippy::too_many_arguments)]
fn square(
    out: &mut Vec<Value>,
    name: &str,
    x: Emu,
    y: Emu,
    d: Emu,
    label: Option<&str>,
    icon: Option<&str>,
    on: bool,
) {
    let (fill, fg) = if on {
        ("accent", "on_accent")
    } else {
        ("panel", "ink")
    };
    let mut b = shape(name, "rect", x, y, d, d, json!(fill), json!("none"));
    if icon.is_none()
        && let Some(l) = label
    {
        b["text"] = json!(l);
        b["style"] = json!("badge");
        b["color"] = json!(fg);
        b["insets"] = json!(0);
    }
    out.push(b);
    if let Some(i) = icon {
        let s = (d as f64 * 0.54).round() as Emu;
        out.push(json!({"type": "icon", "name": format!("{name} icon"), "icon": i, "x": e(x + (d - s) / 2), "y": e(y + (d - s) / 2), "size": e(s), "color": fg}));
    }
}

/// A number in `style` (or an icon) with no box (the rule look's marker);
/// returns its height.
#[allow(clippy::too_many_arguments)]
fn numeral(
    out: &mut Vec<Value>,
    look: &Look,
    name: &str,
    x: Emu,
    y: Emu,
    w: Emu,
    label: Option<&str>,
    icon: Option<&str>,
    style: &str,
    color: &str,
) -> Emu {
    if let Some(i) = icon {
        let s = inch(0.4);
        out.push(json!({"type": "icon", "name": format!("{name} icon"), "icon": i, "x": e(x), "y": e(y + inch(0.04)), "size": e(s), "color": color}));
        return s + inch(0.08);
    }
    let Some(l) = label else { return 0 };
    let h = look.line_h(style, 30.0);
    let mut t = text(
        name,
        x,
        y,
        w,
        h,
        json!([para(l, style, json!({"color": color}))]),
    );
    t["wrap"] = json!(false);
    out.push(t);
    h
}

const ITEM_KEYS: &[&str] = &["title", "text", "bullets", "icon"];

/// Expand a component into elements; `None` if `v` isn't one.
pub fn expand(v: &Value, cx: &Ctx) -> Result<Option<Vec<Value>>> {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    let is_table_preset = ty == "table" && v.get("preset").is_some_and(|p| !p.is_null());
    if !COMPONENTS.contains(&ty) && !is_table_preset {
        return Ok(None);
    }
    let look = look_of(cx);
    let look: &Look = &look;
    let out = match ty {
        "stat_cards" => stat_cards(v, cx, look)?,
        "cards" => cards(v, cx, look)?,
        "steps" => steps(v, cx, look)?,
        "timeline" => timeline(v, cx, look)?,
        "callout" => callout(v, cx, look)?,
        "numbered_list" => numbered_list(v, cx, look)?,
        "comparison" => comparison(v, cx, look)?,
        "badge" => badge(v, cx)?,
        _ => clean_table(v, cx, look)?,
    };
    Ok(Some(out))
}

fn base_keys(more: &[&'static str]) -> Vec<&'static str> {
    keys_with(&[&["type", "name", "x", "y", "w", "h", "gap"], more])
}

fn prefix(o: &Map<String, Value>, d: &str) -> Result<String> {
    Ok(opt_str(o, "name")?.unwrap_or(d).to_owned())
}

fn stat_cards(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(v, "stat_cards", &base_keys(&["items"]))?;
    let ([x, y, w, mut h], given) = area(o, cx, look)?;
    if !given[3] {
        h = h.min(inch(3.1));
    }
    let list = items(o, "items", 1, 8)?;
    let name = prefix(o, "Stat card")?;
    let n = list.len() as Emu;
    let gap = gap(o, cx, look)?;
    let cw = (w - gap * (n - 1)) / n;
    let ruled = look.card_style == CardStyle::Rule;
    let pad = if ruled {
        0
    } else if cw > inch(2.4) {
        inch(0.32)
    } else {
        inch(0.22)
    };
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let io = object(
            it,
            "a stat card",
            &["value", "label", "detail", "highlight"],
        )?;
        let hi = opt_bool(io, "highlight")?.unwrap_or(false);
        let x0 = x + i as Emu * (cw + gap);
        let on = (hi && !ruled).then_some(("on_accent", "accent_soft"));
        let mut ty = if ruled {
            out.push(top_rule(&format!("{name} {} rule", i + 1), x0, y, cw, hi));
            y + inch(0.12)
        } else {
            out.push(shape(
                &format!("{name} {}", i + 1),
                "rect",
                x0,
                y,
                cw,
                h,
                json!(if hi { "accent" } else { "panel" }),
                json!("none"),
            ));
            y + pad * 3 / 4
        };
        let tw = cw - 2 * pad;
        let value = text_of(io, "value")?.context("a stat card needs `value`")?;
        let vh = look.line_h("stat", 40.0) * look.lines("stat", 40.0, &value, tw).min(3) as Emu;
        let mut vp = para(&value, "stat", json!({}));
        if let Some((c, _)) = on {
            vp["color"] = json!(c);
        } else if hi {
            vp["color"] = json!("accent");
        }
        out.push(text(
            &format!("{name} {} value", i + 1),
            x0 + pad,
            ty,
            tw,
            vh,
            json!([vp]),
        ));
        ty += vh + inch(0.1);
        let mut sub = Map::new();
        if let Some(l) = text_of(io, "label")? {
            sub.insert("title".into(), json!(l));
        }
        if let Some(d) = text_of(io, "detail")? {
            sub.insert("text".into(), json!(d));
        }
        if !sub.is_empty() {
            let mut paras = body(&sub, "card_title", "small", on)?;
            if let Some(Value::Object(f)) = paras.first_mut()
                && sub.contains_key("title")
            {
                f.insert("size".into(), json!(look.size_of("card_title", 16.0) - 2.0));
            }
            out.push(text(
                &format!("{name} {} label", i + 1),
                x0 + pad,
                ty,
                tw,
                (y + h - pad / 2 - ty).max(inch(0.3)),
                json!(paras),
            ));
        }
    }
    Ok(out)
}

fn cards(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(
        v,
        "cards",
        &base_keys(&[
            "items",
            "columns",
            "numbered",
            "numbers",
            "start",
            "fill",
            "highlight",
        ]),
    )?;
    let ([x, y, w, h], given) = area(o, cx, look)?;
    let list = items(o, "items", 1, 24)?;
    let name = prefix(o, "Card")?;
    let n = list.len();
    let cols = match o.get("columns").filter(|c| !c.is_null()) {
        Some(c) => c
            .as_u64()
            .filter(|c| (1..=6).contains(c))
            .context("`columns` must be 1 to 6")? as usize,
        None => match n {
            1..=3 => n,
            4 => 2,
            5 | 6 | 9 => 3,
            _ => 4,
        },
    };
    let rows = n.div_ceil(cols);
    let gap = gap(o, cx, look)?;
    let cw = (w - gap * (cols as Emu - 1)) / cols as Emu;
    let full = (h - gap * (rows as Emu - 1)) / rows as Emu;
    let numbered = opt_bool(o, "numbered")?.unwrap_or(true);
    let ruled = look.card_style == CardStyle::Rule;
    let pad = if ruled { 0 } else { inch(0.3) };
    let marker_h = |io: &Map<String, Value>| -> Emu {
        if io.get("icon").is_some_and(|i| !i.is_null()) {
            inch(0.48)
        } else if numbered {
            look.line_h("number", 24.0) + inch(0.1)
        } else {
            0
        }
    };
    let mut ch = full;
    if !given[3] {
        // As tall as the fullest card needs, at least 2in, at most the box.
        let tw = cw - 2 * pad;
        let mut need = 0;
        for it in list {
            let mut owned = Map::new();
            let io = item(it, "a card", ITEM_KEYS, &mut owned)?;
            let mut hgt = 2 * pad.max(inch(0.15)) + marker_h(io) + inch(0.2);
            if let Some(t) = text_of(io, "title")? {
                hgt +=
                    look.line_h("card_title", 16.0) * look.lines("card_title", 16.0, &t, tw) as Emu;
            }
            if let Some(t) = text_of(io, "text")? {
                hgt += look.line_h("small", 12.0) * look.lines("small", 12.0, &t, tw) as Emu
                    + inch(0.06);
            }
            if let Some(Value::Array(b)) = io.get("bullets") {
                for x in b {
                    let t = x.as_str().unwrap_or("");
                    hgt += look.line_h("small", 12.0)
                        * look.lines("small", 12.0, t, tw - inch(0.3)) as Emu
                        + inch(0.06);
                }
            }
            need = need.max(hgt);
        }
        ch = need.max(inch(2.0)).min(full);
    }
    let fill = o.get("fill").cloned().unwrap_or(json!("panel"));
    let hi_at = o
        .get("highlight")
        .and_then(Value::as_u64)
        .map(|k| k as usize);
    let vertical = ch >= inch(1.6);
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a card", ITEM_KEYS, &mut owned)?;
        let (r, c) = (i / cols, i % cols);
        let x0 = x + c as Emu * (cw + gap);
        let y0 = y + r as Emu * (ch + gap);
        let hi = hi_at == Some(i + 1);
        let on = (hi && !ruled).then_some(("on_accent", "accent_soft"));
        if ruled {
            out.push(top_rule(&format!("{name} {} rule", i + 1), x0, y0, cw, hi));
        } else {
            out.push(shape(
                &format!("{name} {}", i + 1),
                "rect",
                x0,
                y0,
                cw,
                ch,
                if hi { json!("accent") } else { fill.clone() },
                json!("none"),
            ));
        }
        let icon = opt_str(io, "icon")?;
        let label = numbered.then(|| number(o, i)).transpose()?;
        let mark_color = if on.is_some() { "on_accent" } else { "accent" };
        let paras = body(io, "card_title", "small", on)?;
        let top = if ruled { inch(0.14) } else { pad };
        if vertical {
            let mut ty = y0 + top;
            let mh = numeral(
                &mut out,
                look,
                &format!("{name} {} number", i + 1),
                x0 + pad,
                ty,
                inch(1.2),
                label.as_deref(),
                icon,
                "number",
                mark_color,
            );
            if mh > 0 {
                ty += mh + inch(0.1);
            }
            out.push(text(
                &format!("{name} {} text", i + 1),
                x0 + pad,
                ty,
                cw - 2 * pad,
                (y0 + ch - pad / 2 - ty).max(inch(0.3)),
                json!(paras),
            ));
        } else {
            let mut tx = x0 + pad;
            if icon.is_some() || label.is_some() {
                let mh = look.line_h("number", 24.0).max(inch(0.48));
                numeral(
                    &mut out,
                    look,
                    &format!("{name} {} number", i + 1),
                    tx,
                    y0 + (ch - mh) / 2 + if ruled { inch(0.06) } else { 0 },
                    inch(0.8),
                    label.as_deref(),
                    icon,
                    "number",
                    mark_color,
                );
                tx += inch(0.75);
            }
            let mut t = text(
                &format!("{name} {} text", i + 1),
                tx,
                y0 + if ruled { inch(0.06) } else { 0 },
                (x0 + cw - pad / 2 - tx).max(inch(0.5)),
                ch,
                json!(paras),
            );
            t["anchor"] = json!("middle");
            out.push(t);
        }
    }
    Ok(out)
}

fn steps(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(
        v,
        "steps",
        &base_keys(&["items", "current", "numbers", "start"]),
    )?;
    let ([x, y, w, h], given) = area(o, cx, look)?;
    let list = items(o, "items", 1, 12)?;
    let name = prefix(o, "Step")?;
    let n = list.len() as Emu;
    let mut pitch = h / n;
    if !given[3] {
        pitch = pitch.min(inch(1.0));
    }
    let current = o.get("current").and_then(Value::as_u64).unwrap_or(1) as usize;
    let ruled = look.card_style == CardStyle::Rule;
    let d = inch(0.46);
    let tx = x + inch(0.75);
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a step", ITEM_KEYS, &mut owned)?;
        let by = y + i as Emu * pitch;
        let on = i + 1 == current;
        let icon = opt_str(io, "icon")?;
        if ruled {
            let label = number(o, i)?.trim_start_matches('0').to_owned();
            numeral(
                &mut out,
                look,
                &format!("{name} {} number", i + 1),
                x,
                by - inch(0.08),
                inch(0.65),
                Some(label.as_str()),
                icon,
                "numeral",
                if on { "accent" } else { "muted" },
            );
            if (i as Emu) < n - 1 {
                out.push(rule(
                    &format!("{name} {} rule", i + 1),
                    tx,
                    by + pitch - inch(0.1),
                    x + w,
                    by + pitch - inch(0.1),
                    "line",
                    "0.75pt",
                ));
            }
        } else {
            let label = number(o, i)?;
            square(
                &mut out,
                &format!("{name} {} badge", i + 1),
                x,
                by,
                d,
                Some(label.as_str()),
                icon,
                on,
            );
        }
        let paras = body(io, "card_title", "small", None)?;
        out.push(text(
            &format!("{name} {} text", i + 1),
            tx,
            by + inch(0.02),
            (x + w - tx).max(inch(1.0)),
            pitch - inch(0.16),
            json!(paras),
        ));
    }
    Ok(out)
}

fn timeline(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(
        v,
        "timeline",
        &base_keys(&["items", "current", "style", "numbers", "start"]),
    )?;
    let ([x, y, w, h], _) = area(o, cx, look)?;
    let list = items(o, "items", 1, 10)?;
    let name = prefix(o, "Phase")?;
    let n = list.len() as Emu;
    let current = o.get("current").and_then(Value::as_u64).unwrap_or(1) as usize;
    let keys = &["label", "title", "text", "bullets"];
    let mut out = Vec::new();
    match opt_str(o, "style")?.unwrap_or("track") {
        "track" | "dots" => {
            let pitch = w / n;
            let d = inch(0.3);
            let ly = y + d / 2;
            out.push(rule(
                &format!("{name} track"),
                x,
                ly,
                x + w,
                ly,
                "line",
                "2pt",
            ));
            if current > 1 {
                let to = (x + (current as Emu - 1) * pitch).min(x + w);
                out.push(rule(
                    &format!("{name} progress"),
                    x,
                    ly,
                    to,
                    ly,
                    "accent",
                    "2pt",
                ));
            }
            for (i, it) in list.iter().enumerate() {
                let mut owned = Map::new();
                let io = item(it, "a timeline item", keys, &mut owned)?;
                let x0 = x + i as Emu * pitch;
                let (fill, line) = if i + 1 == current {
                    (json!("accent"), json!("none"))
                } else if i + 1 < current {
                    (json!("ink"), json!("none"))
                } else {
                    (
                        json!("background"),
                        json!({"color": "ink", "width": "1.25pt"}),
                    )
                };
                out.push(shape(
                    &format!("{name} {} marker", i + 1),
                    "rect",
                    x0,
                    y,
                    d,
                    d,
                    fill,
                    line,
                ));
                let mut paras = Vec::new();
                if let Some(l) = text_of(io, "label")? {
                    let color = if i + 1 == current { "accent" } else { "muted" };
                    paras.push(para(&l, "label", json!({"color": color})));
                }
                let mut rest = body(io, "card_title", "small", None)?;
                if let Some(Value::Object(f)) = rest.first_mut() {
                    if io.contains_key("title") {
                        f.insert("size".into(), json!(look.size_of("card_title", 16.0) + 1.0));
                    }
                    if !paras.is_empty() {
                        f.insert("space_before".into(), json!(5));
                    }
                }
                paras.extend(rest);
                let ty = y + d + inch(0.25);
                out.push(text(
                    &format!("{name} {} text", i + 1),
                    x0,
                    ty,
                    pitch - inch(0.3),
                    (y + h - ty).max(inch(0.6)),
                    json!(paras),
                ));
            }
        }
        "chevrons" => {
            let ch = inch(0.85);
            let depth = (ch as f64 * 0.3).round() as Emu;
            let step = (w - depth) / n;
            for (i, it) in list.iter().enumerate() {
                let mut owned = Map::new();
                let io = item(it, "a timeline item", keys, &mut owned)?;
                let x0 = x + i as Emu * step;
                let (fill, fg) = if i + 1 < current {
                    ("ink", "on_accent")
                } else if i + 1 == current {
                    ("accent", "on_accent")
                } else {
                    ("panel", "ink")
                };
                let mut c = shape(
                    &format!("{name} {}", i + 1),
                    if i == 0 { "homePlate" } else { "chevron" },
                    x0,
                    y,
                    step - inch(0.05) + depth,
                    ch,
                    json!(fill),
                    json!("none"),
                );
                c["adjust"] = json!({"adj": 30000});
                c["text"] = json!(text_of(io, "title")?.unwrap_or_default());
                c["style"] = json!("card_title");
                c["color"] = json!(fg);
                let left = if i == 0 { inch(0.2) } else { depth + inch(0.1) };
                c["insets"] = json!([e(left), 0, e(depth), 0]);
                out.push(c);
                let mut paras = Vec::new();
                if let Some(l) = text_of(io, "label")? {
                    let color = if i + 1 == current { "accent" } else { "muted" };
                    paras.push(para(&l, "label", json!({"color": color})));
                }
                let mut sub = io.clone();
                sub.remove("title");
                let rest = body(&sub, "card_title", "small", None)?;
                let only_blank = rest.len() == 1 && rest[0] == json!("");
                if !only_blank {
                    let mut rest = rest;
                    if !paras.is_empty()
                        && let Some(Value::Object(f)) = rest.first_mut()
                    {
                        f.insert("space_before".into(), json!(4));
                    }
                    paras.extend(rest);
                }
                if !paras.is_empty() {
                    let ty = y + ch + inch(0.25);
                    out.push(text(
                        &format!("{name} {} text", i + 1),
                        x0 + inch(0.1),
                        ty,
                        step - inch(0.3),
                        (y + h - ty).max(inch(0.6)),
                        json!(paras),
                    ));
                }
            }
        }
        other => bail!("timeline `style` must be track or chevrons, not \"{other}\""),
    }
    Ok(out)
}

/// The palette colour of a callout tone.
fn tone(t: &str) -> Result<&'static str> {
    Ok(match t {
        "info" => "accent",
        "warning" => "warning",
        "success" => "good",
        "danger" => "bad",
        "note" => "muted",
        other => {
            bail!("callout `tone` must be info, warning, success, danger or note, not \"{other}\"")
        }
    })
}

fn callout(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(
        v,
        "a callout",
        &base_keys(&["tone", "title", "text", "icon"]),
    )?;
    let content = cx.area.unwrap_or(look.content);
    let base = tone(opt_str(o, "tone")?.unwrap_or("info"))?;
    let title = text_of(o, "title")?;
    let body_t = text_of(o, "text")?.unwrap_or_default();
    let icon = opt_str(o, "icon")?;
    let ruled = look.card_style == CardStyle::Rule;
    let x = match o.get("x").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "x")?,
        None => content[0],
    };
    let w = match o.get("w").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "w")?,
        None => content[0] + content[2] - x,
    };
    let icon_w = if icon.is_some() {
        inch(0.34) + inch(0.18)
    } else {
        0
    };
    let left = inch(0.3) + icon_w;
    let all = format!("{} {body_t}", title.as_deref().unwrap_or(""));
    let lines = look
        .lines("callout", 14.0, all.trim(), w - left - inch(0.3))
        .min(6);
    let auto_h =
        look.line_h("callout", 14.0) * lines as Emu + inch(if ruled { 0.16 } else { 0.32 });
    let h = match o.get("h").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "h")?,
        None => auto_h.max(inch(if ruled { 0.45 } else { 0.6 })),
    };
    let y = match o.get("y").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "y")?,
        None => content[1] + content[3] - h,
    };
    let name = prefix(o, "Callout")?;
    // The rule look: a pull quote beside a rule in the tone's colour. The
    // fill look: a bar in the tone's colour (a panel for notes).
    let (lead, fg, fill) = if ruled {
        (base, "ink", None)
    } else if base == "muted" {
        ("ink", "ink", Some("panel"))
    } else {
        ("on_accent", "on_accent", Some(base))
    };
    let mut runs = Vec::new();
    if let Some(t) = &title {
        runs.push(json!({"text": format!("{t} "), "bold": !ruled, "color": lead}));
    }
    if !body_t.is_empty() {
        runs.push(json!({"text": body_t, "color": fg}));
    }
    let mut out = Vec::new();
    let mut s = match fill {
        Some(f) => shape(&name, "rect", x, y, w, h, json!(f), json!("none")),
        None => {
            out.push(rule(
                &format!("{name} rule"),
                x,
                y,
                x,
                y + h,
                if base == "muted" { "line" } else { base },
                "2.5pt",
            ));
            text(&name, x, y, w, h, json!(""))
        }
    };
    s["anchor"] = json!("middle");
    s["align"] = json!("left");
    s["insets"] = json!([e(left), e(inch(0.06)), e(inch(0.3)), e(inch(0.06))]);
    s["text"] = json!([{"runs": runs, "style": "callout"}]);
    out.push(s);
    if let Some(i) = icon {
        let sz = inch(0.34);
        out.push(json!({"type": "icon", "name": format!("{name} icon"), "icon": i, "x": e(x + inch(0.28)), "y": e(y + (h - sz) / 2), "size": e(sz), "color": lead}));
    }
    Ok(out)
}

fn numbered_list(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(
        v,
        "numbered_list",
        &base_keys(&["items", "columns", "marker", "boxed", "numbers", "start"]),
    )?;
    let ([x, y, w, h], given) = area(o, cx, look)?;
    let list = items(o, "items", 1, 24)?;
    let name = prefix(o, "Item")?;
    let n = list.len();
    let cols = o
        .get("columns")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 4) as usize;
    let rows = n.div_ceil(cols);
    let col_gap = gap(o, cx, look)?;
    let row_gap = inch(0.18).min(col_gap);
    let cw = (w - col_gap * (cols as Emu - 1)) / cols as Emu;
    let mut rh = (h - row_gap * (rows as Emu - 1)) / rows as Emu;
    if !given[3] {
        rh = rh.min(inch(1.15));
    }
    let boxed = opt_bool(o, "boxed")?.unwrap_or(true);
    let big = match opt_str(o, "marker")?.unwrap_or("badge") {
        "badge" => false,
        "number" => true,
        other => bail!("`marker` must be badge or number, not \"{other}\""),
    };
    let ruled = look.card_style == CardStyle::Rule;
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a list item", ITEM_KEYS, &mut owned)?;
        let (r, c) = (i / cols, i % cols);
        let x0 = x + c as Emu * (cw + col_gap);
        let y0 = y + r as Emu * (rh + row_gap);
        let pad = if boxed && !ruled { inch(0.3) } else { 0 };
        if boxed {
            if ruled {
                out.push(rule(
                    &format!("{name} {} rule", i + 1),
                    x0,
                    y0,
                    x0 + cw,
                    y0,
                    "line",
                    "0.75pt",
                ));
            } else {
                out.push(shape(
                    &format!("{name} {}", i + 1),
                    "rect",
                    x0,
                    y0,
                    cw,
                    rh,
                    json!("panel"),
                    json!("none"),
                ));
            }
        }
        let icon = opt_str(io, "icon")?;
        let mut tx = x0 + pad;
        if big || ruled {
            let mut label = number(o, i)?;
            if !big {
                label = label.trim_start_matches('0').to_owned();
            }
            let nw = if big { inch(0.95) } else { inch(0.7) };
            let mh = look.line_h("numeral", 30.0);
            numeral(
                &mut out,
                look,
                &format!("{name} {} number", i + 1),
                tx,
                y0 + (rh - mh) / 2,
                nw,
                Some(label.as_str()),
                icon,
                "numeral",
                "accent",
            );
            tx += nw;
        } else {
            let d = inch(0.46).min(rh - inch(0.1)).max(inch(0.3));
            let label = number(o, i)?;
            square(
                &mut out,
                &format!("{name} {} badge", i + 1),
                tx,
                y0 + (rh - d) / 2,
                d,
                Some(label.as_str()),
                icon,
                true,
            );
            tx += d + inch(0.25);
        }
        let paras = body(io, "card_title", "small", None)?;
        let mut t = text(
            &format!("{name} {} text", i + 1),
            tx,
            y0,
            (x0 + cw - pad / 2 - tx).max(inch(0.5)),
            rh,
            json!(paras),
        );
        t["anchor"] = json!("middle");
        out.push(t);
    }
    Ok(out)
}

fn comparison(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let o = object(v, "comparison", &base_keys(&["columns"]))?;
    let ([x, y, w, h], _) = area(o, cx, look)?;
    let list = items(o, "columns", 2, 4)?;
    let name = prefix(o, "Column")?;
    let n = list.len() as Emu;
    let gap = gap(o, cx, look)?;
    let cw = (w - gap * (n - 1)) / n;
    let ruled = look.card_style == CardStyle::Rule;
    let pad = if ruled { 0 } else { inch(0.3) };
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let io = object(
            it,
            "a comparison column",
            &["title", "tag", "items", "text", "highlight", "icon"],
        )?;
        let hi = opt_bool(io, "highlight")?.unwrap_or(false);
        let x0 = x + i as Emu * (cw + gap);
        let on = hi && !ruled;
        if ruled {
            out.push(top_rule(&format!("{name} {} rule", i + 1), x0, y, cw, hi));
        } else {
            out.push(shape(
                &format!("{name} {}", i + 1),
                "rect",
                x0,
                y,
                cw,
                h,
                json!(if hi { "accent" } else { "panel" }),
                json!("none"),
            ));
        }
        let (tag_c, title_c, text_c, item_c) = if on {
            ("accent_soft", "on_accent", "accent_soft", "on_accent")
        } else if hi {
            ("accent", "ink", "muted", "ink")
        } else {
            ("muted", "ink", "muted", "ink")
        };
        let mut ty = y + if ruled { inch(0.18) } else { pad };
        if let Some(icon) = opt_str(io, "icon")? {
            let s = inch(0.36);
            let c = if on {
                "on_accent"
            } else if hi {
                "accent"
            } else {
                "muted"
            };
            out.push(json!({"type": "icon", "name": format!("{name} {} icon", i + 1), "icon": icon, "x": e(x0 + pad), "y": e(ty), "size": e(s), "color": c}));
            ty += s + inch(0.15);
        }
        let mut paras = Vec::new();
        if let Some(t) = text_of(io, "tag")? {
            paras.push(para(&t, "label", json!({"color": tag_c})));
        }
        if let Some(t) = text_of(io, "title")? {
            let mut more =
                json!({"size": look.size_of("card_title", 16.0) + 2.0, "color": title_c});
            if !paras.is_empty() {
                more["space_before"] = json!(4);
            }
            paras.push(para(&t, "card_title", more));
        }
        if let Some(t) = text_of(io, "text")? {
            paras.push(para(
                &t,
                "small",
                json!({"space_before": 6, "color": text_c}),
            ));
        }
        if let Some(list) = io.get("items").filter(|v| !v.is_null()) {
            let list = list
                .as_array()
                .context("comparison `items` must be an array of strings")?;
            for (k, b) in list.iter().enumerate() {
                let t = b
                    .as_str()
                    .context("comparison `items` must be an array of strings")?;
                paras.push(para(t, "small", json!({"bullet": true, "color": item_c, "space_before": if k == 0 { 10 } else { 6 }})));
            }
        }
        if paras.is_empty() {
            paras.push(json!(""));
        }
        out.push(text(
            &format!("{name} {} text", i + 1),
            x0 + pad,
            ty,
            cw - 2 * pad,
            (y + h - pad / 2 - ty).max(inch(0.4)),
            json!(paras),
        ));
    }
    Ok(out)
}

fn badge(v: &Value, cx: &Ctx) -> Result<Vec<Value>> {
    let o = object(
        v,
        "a badge",
        &[
            "type", "name", "x", "y", "size", "text", "icon", "fill", "color", "geometry",
        ],
    )?;
    let d = match o.get("size").filter(|v| !v.is_null()) {
        Some(s) => length(s, cx, "size")?,
        None => inch(0.46),
    };
    let x = o
        .get("x")
        .filter(|v| !v.is_null())
        .map(|v| length(v, cx, "x"))
        .transpose()?
        .unwrap_or(0);
    let y = o
        .get("y")
        .filter(|v| !v.is_null())
        .map(|v| length(v, cx, "y"))
        .transpose()?
        .unwrap_or(0);
    let name = prefix(o, "Badge")?;
    let fill = o.get("fill").cloned().unwrap_or(json!("accent"));
    let fg = o.get("color").cloned().unwrap_or(json!("on_accent"));
    let geometry = opt_str(o, "geometry")?.unwrap_or("rect");
    let mut b = shape(&name, geometry, x, y, d, d, fill, json!("none"));
    let mut out = Vec::new();
    match (text_of(o, "text")?, opt_str(o, "icon")?) {
        (_, Some(i)) => {
            out.push(b);
            let s = (d as f64 * 0.54).round() as Emu;
            out.push(json!({"type": "icon", "name": format!("{name} icon"), "icon": i, "x": e(x + (d - s) / 2), "y": e(y + (d - s) / 2), "size": e(s), "color": fg}));
        }
        (t, None) => {
            b["text"] = json!(t.unwrap_or_default());
            b["style"] = json!("badge");
            b["color"] = fg;
            b["insets"] = json!(0);
            out.push(b);
        }
    }
    Ok(out)
}

/// `{"type": "table", "preset": "clean"}`, drawn by the deck's card style.
/// Rule: no fills, an ink rule above and below, a thinner one under the
/// header and hairlines between rows. Fill: an ink header row and banded
/// rows. No grid lines either way.
fn clean_table(v: &Value, cx: &Ctx, look: &Look) -> Result<Vec<Value>> {
    let mut t = v.as_object().cloned().unwrap_or_default();
    let preset = t.remove("preset");
    if preset.as_ref().and_then(Value::as_str) != Some("clean") {
        bail!("table `preset` must be \"clean\"");
    }
    let first_bold = match t.remove("first_bold") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => b,
        Some(_) => bail!("`first_bold` must be true or false"),
    };
    let ruled = look.card_style == CardStyle::Rule;
    let content = cx.area.unwrap_or(look.content);
    let x = match t.get("x").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "x")?,
        None => content[0],
    };
    let y = match t.get("y").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "y")?,
        None => content[1],
    };
    let w = match t.get("w").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "w")?,
        None => content[0] + content[2] - x,
    };
    t.insert("x".into(), e(x));
    t.insert("y".into(), e(y));
    t.insert("w".into(), e(w));
    let header = t.get("header").and_then(Value::as_bool).unwrap_or(true);
    let n_rows = t.get("rows").and_then(Value::as_array).map_or(0, Vec::len);
    if n_rows == 0 {
        bail!("a table needs `rows`");
    }
    let header_h = inch(0.42);
    let body_h = match t.remove("row_height").filter(|v| !v.is_null()) {
        Some(v) => length(&v, cx, "row_height")?,
        None => inch(0.48),
    };
    let heights: Vec<Emu> = (0..n_rows)
        .map(|r| if header && r == 0 { header_h } else { body_h })
        .collect();
    if !t.contains_key("row_heights") {
        let hs: Vec<Value> = heights.iter().map(|h| e(*h)).collect();
        t.insert("row_heights".into(), json!(hs));
    }
    let defaults = if ruled {
        vec![
            ("header_fill", json!("none")),
            ("header_color", json!("muted")),
            ("banded", json!(false)),
        ]
    } else {
        vec![
            ("header_fill", json!("ink")),
            ("header_color", json!("on_accent")),
            ("banded", json!(true)),
            ("band_fill", json!("panel")),
        ]
    };
    for (k, d) in defaults.into_iter().chain([
        ("border", json!("none")),
        ("anchor", json!("middle")),
        (
            "cell_margins",
            json!([e(inch(0.08)), e(inch(0.04)), e(inch(0.08)), e(inch(0.04))]),
        ),
        ("style", json!("table")),
    ]) {
        t.entry(k).or_insert(d);
    }
    if let Some(Value::Array(rows)) = t.get_mut("rows") {
        for (r, row) in rows.iter_mut().enumerate() {
            let Value::Array(cells) = row else { continue };
            for (c, cell) in cells.iter_mut().enumerate() {
                let extra = if header && r == 0 {
                    json!({"style": "table_header"})
                } else if first_bold && c == 0 {
                    json!({"bold": true})
                } else {
                    continue;
                };
                match cell {
                    Value::String(_) | Value::Number(_) => {
                        let mut o = extra;
                        o["text"] = cell.clone();
                        *cell = o;
                    }
                    Value::Object(co) => {
                        if let Value::Object(m) = extra {
                            for (k, v) in m {
                                co.entry(k).or_insert(v);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut out = vec![Value::Object(t)];
    if ruled {
        let mut ry = y;
        out.push(rule("Table rule top", x, ry, x + w, ry, "ink", "1pt"));
        for (r, rh) in heights.iter().enumerate() {
            ry += rh;
            let last = r + 1 == heights.len();
            let (name, color, width) = if last {
                ("Table rule bottom".to_owned(), "ink", "1pt")
            } else if header && r == 0 {
                ("Table rule header".to_owned(), "ink", "0.75pt")
            } else {
                (format!("Table rule {}", r + 1), "line", "0.5pt")
            };
            out.push(rule(&name, x, ry, x + w, ry, color, width));
        }
    }
    Ok(out)
}

// ── schema ─────────────────────────────────────────────────────────────────

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

/// The box keys every component has.
fn box_schema(more: Value) -> Map<String, Value> {
    let mut m = json!({
        "name": {"type": "string", "description": "Prefix of the shapes' names."},
        "x": r("length"), "y": r("length"),
        "w": r("length"), "h": r("length"),
        "gap": {"description": "Space between cards (default the deck's content gap, 0.24in).", "allOf": [r("length")]},
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    if let Value::Object(o) = more {
        m.extend(o);
    }
    m
}

fn closed(ty: &str, props: Map<String, Value>, required: &[&str], description: &str) -> Value {
    let mut props = props;
    props.insert("type".into(), json!({"const": ty}));
    let mut req = vec!["type"];
    req.extend_from_slice(required);
    json!({"type": "object", "description": description, "properties": props, "required": req, "additionalProperties": false})
}

/// The JSON Schema definitions of the components (`$defs` names, schemas).
pub fn schema_defs() -> Vec<(&'static str, Value)> {
    let text = json!({"type": ["string", "number"]});
    let item = json!({"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "properties": {
        "title": {"type": "string"}, "text": {"type": "string"},
        "bullets": {"type": "array", "items": {"type": "string"}},
        "icon": {"type": "string", "description": "A Lucide icon name (instead of the number)."},
    }}]});
    let numbers =
        json!({"type": "string", "enum": ["01", "1"], "description": "Number style (default 01)."});
    vec![
        (
            "stat_cards",
            closed(
                "stat_cards",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 8, "items": {"type": "object", "additionalProperties": false, "required": ["value"], "properties": {
                        "value": text, "label": {"type": "string"}, "detail": {"type": "string"},
                        "highlight": {"type": "boolean", "description": "Stands out: an accent rule (rule card style) or an accent fill (fill card style)."}}}},
                })),
                &["items"],
                "A row of stats (big value, label, detail) spread across the box: under a rule (rule card style) or on square panels (fill); one may be highlighted.",
            ),
        ),
        (
            "cards",
            closed(
                "cards",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 24, "items": item},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 6},
                    "numbered": {"type": "boolean", "description": "A number on each card (default true; an item's icon replaces it)."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                    "fill": {"description": "The cards' fill in the fill card style (default the palette's panel).", "allOf": [r("fill")]},
                    "highlight": {"type": "integer", "minimum": 1, "description": "This card (from 1) stands out: an accent rule or an accent fill."},
                })),
                &["items"],
                "A grid of cards: number or icon, heading, text or bullets; under a rule (rule card style) or on square panels (fill). Short cards put the number beside the text.",
            ),
        ),
        (
            "steps",
            closed(
                "steps",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 12, "items": item},
                    "current": {"type": "integer", "minimum": 0, "description": "The highlighted step, from 1 (default 1; 0: none)."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                })),
                &["items"],
                "Numbered steps down the box: big numerals with hairlines between (rule card style) or square number badges (fill).",
            ),
        ),
        (
            "timeline",
            closed(
                "timeline",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 10, "items": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "properties": {
                        "label": {"type": "string", "description": "Small caps tag such as \"Phase 1\"."}, "title": {"type": "string"}, "text": {"type": "string"},
                        "bullets": {"type": "array", "items": {"type": "string"}}}}]}},
                    "current": {"type": "integer", "minimum": 0, "description": "The current phase, from 1 (default 1)."},
                    "style": {"type": "string", "enum": ["track", "chevrons"], "description": "Square markers on a line (default) or chevrons."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                })),
                &["items"],
                "Phases across the box: square markers on a line (done ink, current accent, later outlined) or chevrons, with a label, title and text under each.",
            ),
        ),
        (
            "callout",
            closed(
                "callout",
                box_schema(json!({
                    "tone": {"type": "string", "enum": ["info", "warning", "success", "danger", "note"]},
                    "title": {"type": "string", "description": "Bold lead-in."}, "text": {"type": "string"},
                    "icon": {"type": "string", "description": "A Lucide icon name."},
                })),
                &[],
                "The one sentence to remember, with a lead-in: a pull quote beside an accent rule (rule card style) or a solid bar (fill); by default across the bottom of the content area.",
            ),
        ),
        (
            "numbered_list",
            closed(
                "numbered_list",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 24, "items": item},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 4},
                    "marker": {"type": "string", "enum": ["badge", "number"], "description": "badge (default): a numeral (rule card style) or a square badge (fill); number: a big 01, 02."},
                    "boxed": {"type": "boolean", "description": "A hairline above each row (rule card style) or a panel behind it (fill); default true."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                })),
                &["items"],
                "Rows of a number and text, centred on each other.",
            ),
        ),
        (
            "comparison",
            closed(
                "comparison",
                box_schema(json!({
                    "columns": {"type": "array", "minItems": 2, "maxItems": 4, "items": {"type": "object", "additionalProperties": false, "properties": {
                        "title": {"type": "string"}, "tag": {"type": "string"}, "text": {"type": "string"},
                        "items": {"type": "array", "items": {"type": "string"}},
                        "icon": {"type": "string"},
                        "highlight": {"type": "boolean"}}}},
                })),
                &["columns"],
                "Two to four columns side by side (before/after, options), each with a tag, title and bullets.",
            ),
        ),
        (
            "badge",
            closed(
                "badge",
                obj(json!({
                    "name": {"type": "string"}, "x": r("length"), "y": r("length"),
                    "size": {"description": "Side (default 0.46in).", "allOf": [r("length")]},
                    "text": text, "icon": {"type": "string"}, "fill": r("fill"), "color": r("color"),
                    "geometry": {"type": "string", "description": "A preset shape (default rect; ellipse for a circle)."},
                })),
                &[],
                "A square badge with a number, short text or icon.",
            ),
        ),
    ]
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Ctx {
        Ctx {
            units: super::super::slides_author::Unit::In,
            ..Ctx::default()
        }
    }

    /// The plain look and every preset's (rule and fill card styles).
    fn looks() -> Vec<Ctx> {
        let p = slides_engine::Presentation::new();
        let mut out = vec![cx()];
        for name in super::super::slides_style::PRESETS {
            let deck = json!({"preset": name});
            let look = Look::build(&p, deck.as_object().unwrap(), &cx()).unwrap();
            out.push(Ctx {
                look: Some(std::sync::Arc::new(look)),
                ..cx()
            });
        }
        out
    }

    #[std::prelude::v1::test]
    fn components_expand_into_elements() {
        for c in looks() {
            expands_in(&c);
        }
    }

    fn expands_in(c: &Ctx) {
        for v in [
            json!({"type": "stat_cards", "items": [{"value": "96%", "label": "On time", "detail": "Up 4 points", "highlight": true}, {"value": 42, "label": "Sites"}]}),
            json!({"type": "cards", "columns": 3, "items": [{"title": "One", "text": "a"}, {"title": "Two", "bullets": ["x", "y"], "icon": "rocket"}, "Three"]}),
            json!({"type": "cards", "h": 1.2, "items": ["A", "B"]}),
            json!({"type": "steps", "items": [{"title": "Share data", "text": "Two months"}, "Go live"], "current": 2}),
            json!({"type": "timeline", "items": [{"label": "Phase 1", "title": "Digital work", "text": "Assets"}, {"label": "Later", "title": "Predictive"}]}),
            json!({"type": "timeline", "style": "chevrons", "items": [{"title": "Plan", "text": "a"}, {"title": "Build"}, {"title": "Run"}], "current": 2}),
            json!({"type": "callout", "tone": "warning", "title": "One option:", "text": "start with utilities", "icon": "lightbulb"}),
            json!({"type": "numbered_list", "columns": 2, "items": [{"title": "Feed pump", "text": "Vibration"}, "Vacuum"]}),
            json!({"type": "numbered_list", "marker": "number", "boxed": false, "items": ["A"]}),
            json!({"type": "comparison", "columns": [{"title": "Today", "items": ["Chat"]}, {"title": "With us", "tag": "After", "items": ["Tickets"], "highlight": true}]}),
            json!({"type": "badge", "x": 1, "y": 1, "text": "3"}),
            json!({"type": "table", "preset": "clean", "columns": [2, 1], "rows": [["Name", "Score"], ["Ann", 9], ["Bo", 7]]}),
        ] {
            let els = expand(&v, c).unwrap().unwrap_or_else(|| panic!("{v}"));
            assert!(!els.is_empty());
            for el in &els {
                super::super::slides_author::element(el, c)
                    .unwrap_or_else(|e| panic!("{v}\n{el}\n{e:#}"));
            }
        }
        assert!(expand(&json!({"type": "shape"}), c).unwrap().is_none());
        assert!(expand(&json!({"type": "cards", "items": []}), c).is_err());
        assert!(expand(&json!({"type": "callout", "tone": "loud"}), c).is_err());
        assert!(
            expand(
                &json!({"type": "stat_cards", "items": [{"value": 1, "colour": "red"}]}),
                c
            )
            .is_err()
        );
        assert!(
            expand(
                &json!({"type": "table", "preset": "fancy", "rows": [["a"]]}),
                c
            )
            .is_err()
        );
    }
}
