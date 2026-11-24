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
use super::slides_style::{Look, look_of};

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

fn hairline() -> Value {
    json!({"color": "divider", "width": "0.75pt"})
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

/// Paragraphs for an item's `title`, `text` and `bullets`.
fn body(
    io: &Map<String, Value>,
    title_style: &str,
    text_style: &str,
    on_dark: bool,
) -> Result<Vec<Value>> {
    let mut paras = Vec::new();
    let (tc, bc) = if on_dark {
        (json!({"color": "white"}), json!({"color": "on_dark_muted"}))
    } else {
        (json!({}), json!({}))
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

/// A round badge with a number or an icon.
#[allow(clippy::too_many_arguments)]
fn marker(
    out: &mut Vec<Value>,
    name: &str,
    x: Emu,
    y: Emu,
    d: Emu,
    label: Option<&str>,
    icon: Option<&str>,
    filled: bool,
) {
    let (fill, fg) = if filled {
        ("primary", "white")
    } else {
        ("tint", "primary")
    };
    let mut b = shape(name, "ellipse", x, y, d, d, json!(fill), json!("none"));
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
    let pad = if cw > inch(2.4) {
        inch(0.35)
    } else {
        inch(0.25)
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
        let mut card = shape(
            &format!("{name} {}", i + 1),
            "roundRect",
            x0,
            y,
            cw,
            h,
            json!(if hi { "primary" } else { "paper" }),
            if hi { json!("none") } else { hairline() },
        );
        card["radius"] = e(inch(0.08));
        out.push(card);
        let mut ty = y + pad;
        out.push(shape(
            &format!("{name} {} rule", i + 1),
            "rect",
            x0 + pad,
            ty,
            inch(0.5),
            inch(0.06),
            json!(if hi { "highlight" } else { "primary" }),
            json!("none"),
        ));
        ty += inch(0.06) + inch(0.22);
        let tw = cw - 2 * pad;
        let value = text_of(io, "value")?.context("a stat card needs `value`")?;
        let vh = look.line_h("stat", 40.0) * look.lines("stat", 40.0, &value, tw).min(3) as Emu;
        let mut vp = para(&value, "stat", json!({}));
        if hi {
            vp["color"] = json!("white");
        }
        out.push(text(
            &format!("{name} {} value", i + 1),
            x0 + pad,
            ty,
            tw,
            vh,
            json!([vp]),
        ));
        ty += vh + inch(0.12);
        let mut sub = Map::new();
        if let Some(l) = text_of(io, "label")? {
            sub.insert("title".into(), json!(l));
        }
        if let Some(d) = text_of(io, "detail")? {
            sub.insert("text".into(), json!(d));
        }
        if !sub.is_empty() {
            let paras = body(&sub, "card_title", "small", hi)?;
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
    let mut ch = (h - gap * (rows as Emu - 1)) / rows as Emu;
    if !given[3] {
        ch = ch.min(inch(2.4));
    }
    let numbered = opt_bool(o, "numbered")?.unwrap_or(true);
    let fill = o.get("fill").cloned().unwrap_or(json!("white"));
    let hi_at = o
        .get("highlight")
        .and_then(Value::as_u64)
        .map(|k| k as usize);
    let vertical = ch >= inch(1.6);
    let pad = inch(0.3);
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a card", ITEM_KEYS, &mut owned)?;
        let (r, c) = (i / cols, i % cols);
        let x0 = x + c as Emu * (cw + gap);
        let y0 = y + r as Emu * (ch + gap);
        let hi = hi_at == Some(i + 1);
        let mut card = shape(
            &format!("{name} {}", i + 1),
            "roundRect",
            x0,
            y0,
            cw,
            ch,
            if hi { json!("primary") } else { fill.clone() },
            if hi { json!("none") } else { hairline() },
        );
        card["radius"] = e(inch(0.08));
        out.push(card);
        let icon = opt_str(io, "icon")?;
        let label = numbered.then(|| number(o, i)).transpose()?;
        let has_marker = icon.is_some() || label.is_some();
        let d = if vertical { inch(0.52) } else { inch(0.46) };
        let paras = body(io, "card_title", "small", hi)?;
        if vertical {
            let mut ty = y0 + pad;
            if has_marker {
                marker(
                    &mut out,
                    &format!("{name} {} badge", i + 1),
                    x0 + pad,
                    ty,
                    d,
                    label.as_deref(),
                    icon,
                    false,
                );
                ty += d + inch(0.2);
            }
            out.push(text(
                &format!("{name} {} text", i + 1),
                x0 + pad,
                ty,
                cw - 2 * pad,
                y0 + ch - pad / 2 - ty,
                json!(paras),
            ));
        } else {
            let mut tx = x0 + pad;
            if has_marker {
                marker(
                    &mut out,
                    &format!("{name} {} badge", i + 1),
                    tx,
                    y0 + (ch - d) / 2,
                    d,
                    label.as_deref(),
                    icon,
                    false,
                );
                tx += d + inch(0.22);
            }
            let mut t = text(
                &format!("{name} {} text", i + 1),
                tx,
                y0,
                x0 + cw - pad / 2 - tx,
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
    let d = inch(0.5);
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a step", ITEM_KEYS, &mut owned)?;
        let by = y + i as Emu * pitch;
        let label = number(o, i)?;
        marker(
            &mut out,
            &format!("{name} {} badge", i + 1),
            x,
            by,
            d,
            Some(label.as_str()),
            opt_str(io, "icon")?,
            i < current,
        );
        if (i as Emu) < n - 1 {
            out.push(json!({"type": "line", "name": format!("{name} {} link", i + 1),
                "x1": e(x + d / 2), "y1": e(by + d + inch(0.05)), "x2": e(x + d / 2), "y2": e(by + pitch - inch(0.05)),
                "color": "divider", "width": "1.5pt"}));
        }
        let tx = x + d + inch(0.25);
        let paras = body(io, "card_title", "small", false)?;
        out.push(text(
            &format!("{name} {} text", i + 1),
            tx,
            by + inch(0.02),
            (x + w - tx).max(inch(1.0)),
            pitch - inch(0.08),
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
    let keys = &["label", "title", "text", "bullets", "badge"];
    let mut out = Vec::new();
    match opt_str(o, "style")?.unwrap_or("dots") {
        "dots" => {
            let pitch = w / n;
            let d = inch(0.6);
            let ly = y + d / 2;
            let start = x + inch(0.1) + d / 2;
            let end = x + w - inch(0.2);
            out.push(shape(
                &format!("{name} track"),
                "rect",
                start,
                ly - inch(0.03),
                end - start,
                inch(0.06),
                json!("divider"),
                json!("none"),
            ));
            if current >= 1 {
                let to =
                    (x + (current as Emu - 1) * pitch + inch(0.1) + d / 2 + pitch / 2).min(end);
                out.push(shape(
                    &format!("{name} progress"),
                    "rect",
                    start,
                    ly - inch(0.03),
                    to - start,
                    inch(0.06),
                    json!("primary"),
                    json!("none"),
                ));
            }
            for (i, it) in list.iter().enumerate() {
                let mut owned = Map::new();
                let io = item(it, "a timeline item", keys, &mut owned)?;
                let cx0 = x + i as Emu * pitch + inch(0.1);
                let done = i < current;
                let label = match text_of(io, "badge")? {
                    Some(b) => b,
                    None => number(o, i)?.trim_start_matches('0').to_owned(),
                };
                let mut c = shape(
                    &format!("{name} {} dot", i + 1),
                    "ellipse",
                    cx0,
                    ly - d / 2,
                    d,
                    d,
                    json!(if done { "primary" } else { "white" }),
                    if done {
                        json!("none")
                    } else {
                        json!({"color": "primary", "width": "1.5pt"})
                    },
                );
                c["text"] = json!(label);
                c["style"] = json!("badge");
                c["size"] = json!(15);
                c["color"] = json!(if done { "white" } else { "primary" });
                c["insets"] = json!(0);
                out.push(c);
                let mut paras = Vec::new();
                if let Some(l) = text_of(io, "label")? {
                    let mut more = json!({});
                    if i + 1 == current {
                        more["color"] = json!("highlight");
                    }
                    paras.push(para(&l, "label", more));
                }
                let mut rest = body(io, "card_title", "small", false)?;
                if !paras.is_empty()
                    && let Some(Value::Object(f)) = rest.first_mut()
                {
                    f.insert("space_before".into(), json!(4));
                }
                paras.extend(rest);
                let ty = y + d + inch(0.3);
                out.push(text(
                    &format!("{name} {} text", i + 1),
                    cx0,
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
                    ("primary", "white")
                } else if i + 1 == current {
                    ("dark", "white")
                } else {
                    ("tint", "primary")
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
                    paras.push(para(&l, "label", json!({})));
                }
                let mut sub = io.clone();
                sub.remove("title");
                let rest = body(&sub, "card_title", "small", false)?;
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
        other => bail!("timeline `style` must be dots or chevrons, not \"{other}\""),
    }
    Ok(out)
}

/// The base colour of a callout tone.
fn tone(t: &str) -> Result<&'static str> {
    Ok(match t {
        "info" => "info",
        "warning" => "warning",
        "success" => "success",
        "danger" => "danger",
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
    let x = match o.get("x").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "x")?,
        None => content[0],
    };
    let w = match o.get("w").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "w")?,
        None => content[0] + content[2] - x,
    };
    let icon_w = if icon.is_some() {
        inch(0.32) + inch(0.18)
    } else {
        0
    };
    let left = inch(0.28) + icon_w;
    let all = format!("{} {body_t}", title.as_deref().unwrap_or(""));
    let lines = look
        .lines("callout", 13.0, all.trim(), w - left - inch(0.28))
        .min(6);
    let auto_h = look.line_h("callout", 13.0) * lines as Emu + inch(0.3);
    let h = match o.get("h").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "h")?,
        None => auto_h.max(inch(0.55)),
    };
    let y = match o.get("y").filter(|v| !v.is_null()) {
        Some(v) => length(v, cx, "y")?,
        None => content[1] + content[3] - h,
    };
    let name = prefix(o, "Callout")?;
    let ink = look.mixed(base, "black", 0.45);
    let mut runs = Vec::new();
    if let Some(t) = &title {
        runs.push(json!({"text": format!("{t} "), "bold": true, "color": ink}));
    }
    if !body_t.is_empty() {
        runs.push(json!({"text": body_t}));
    }
    let mut s = shape(
        &name,
        "roundRect",
        x,
        y,
        w,
        h,
        look.mixed(base, "white", 0.86),
        json!("none"),
    );
    s["radius"] = e(inch(0.12).min(h / 2));
    s["anchor"] = json!("middle");
    s["align"] = json!("left");
    s["insets"] = json!([e(left), e(inch(0.06)), e(inch(0.28)), e(inch(0.06))]);
    s["text"] = json!([{"runs": runs, "style": "callout"}]);
    let mut out = vec![s];
    if let Some(i) = icon {
        let sz = inch(0.32);
        out.push(json!({"type": "icon", "name": format!("{name} icon"), "icon": i, "x": e(x + inch(0.24)), "y": e(y + (h - sz) / 2), "size": e(sz), "color": look.color(base).unwrap_or(json!("accent1"))}));
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
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let mut owned = Map::new();
        let io = item(it, "a list item", ITEM_KEYS, &mut owned)?;
        let (r, c) = (i / cols, i % cols);
        let x0 = x + c as Emu * (cw + col_gap);
        let y0 = y + r as Emu * (rh + row_gap);
        let pad = if boxed { inch(0.3) } else { 0 };
        if boxed {
            let mut b = shape(
                &format!("{name} {}", i + 1),
                "roundRect",
                x0,
                y0,
                cw,
                rh,
                json!("paper"),
                json!("none"),
            );
            b["radius"] = e(inch(0.1).min(rh / 2));
            out.push(b);
        }
        let label = number(o, i)?;
        let mut tx = x0 + pad;
        if big {
            let nw = inch(0.85);
            let mut t = text(
                &format!("{name} {} number", i + 1),
                tx,
                y0,
                nw,
                rh,
                json!([para(&label, "number", json!({}))]),
            );
            t["anchor"] = json!("middle");
            out.push(t);
            tx += nw;
        } else {
            let d = inch(0.5).min(rh - inch(0.1)).max(inch(0.3));
            let label = label.trim_start_matches('0').to_owned();
            marker(
                &mut out,
                &format!("{name} {} badge", i + 1),
                tx,
                y0 + (rh - d) / 2,
                d,
                Some(label.as_str()),
                opt_str(io, "icon")?,
                true,
            );
            tx += d + inch(0.25);
        }
        let paras = body(io, "card_title", "small", false)?;
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
    let pad = inch(0.3);
    let mut out = Vec::new();
    for (i, it) in list.iter().enumerate() {
        let io = object(
            it,
            "a comparison column",
            &["title", "tag", "items", "text", "highlight", "icon"],
        )?;
        let hi = opt_bool(io, "highlight")?.unwrap_or(false);
        let x0 = x + i as Emu * (cw + gap);
        out.push(shape(
            &format!("{name} {}", i + 1),
            "rect",
            x0,
            y,
            cw,
            h,
            json!(if hi { "tint" } else { "paper" }),
            json!("none"),
        ));
        out.push(shape(
            &format!("{name} {} bar", i + 1),
            "rect",
            x0,
            y,
            cw,
            inch(0.06),
            json!(if hi { "primary" } else { "muted" }),
            json!("none"),
        ));
        let mut ty = y + inch(0.06) + pad * 3 / 4;
        if let Some(icon) = opt_str(io, "icon")? {
            let s = inch(0.36);
            out.push(json!({"type": "icon", "name": format!("{name} {} icon", i + 1), "icon": icon, "x": e(x0 + pad), "y": e(ty), "size": e(s), "color": if hi { "primary" } else { "muted" }}));
            ty += s + inch(0.15);
        }
        let mut paras = Vec::new();
        if let Some(t) = text_of(io, "tag")? {
            paras.push(para(
                &t,
                "label",
                json!({"color": if hi { "primary" } else { "muted" }}),
            ));
        }
        if let Some(t) = text_of(io, "title")? {
            let mut more = json!({"size": look.size_of("card_title", 16.0) + 2.0});
            if !paras.is_empty() {
                more["space_before"] = json!(2);
            }
            paras.push(para(&t, "card_title", more));
        }
        if let Some(t) = text_of(io, "text")? {
            paras.push(para(&t, "small", json!({"space_before": 6})));
        }
        if let Some(list) = io.get("items").filter(|v| !v.is_null()) {
            let list = list
                .as_array()
                .context("comparison `items` must be an array of strings")?;
            for (k, b) in list.iter().enumerate() {
                let t = b
                    .as_str()
                    .context("comparison `items` must be an array of strings")?;
                paras.push(para(t, "small", json!({"bullet": true, "color": "ink", "space_before": if k == 0 { 10 } else { 6 }})));
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
            "type", "name", "x", "y", "size", "text", "icon", "fill", "color",
        ],
    )?;
    let d = match o.get("size").filter(|v| !v.is_null()) {
        Some(s) => length(s, cx, "size")?,
        None => inch(0.5),
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
    let fill = o.get("fill").cloned().unwrap_or(json!("primary"));
    let fg = o.get("color").cloned().unwrap_or(json!("white"));
    let mut b = shape(&name, "ellipse", x, y, d, d, fill, json!("none"));
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

/// `{"type": "table", "preset": "clean"}`: a rounded header bar behind a
/// borderless header row, soft banded rows, no grid lines.
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
    let header_h = inch(0.46);
    let body_h = match t.remove("row_height").filter(|v| !v.is_null()) {
        Some(v) => length(&v, cx, "row_height")?,
        None => inch(0.55),
    };
    if !t.contains_key("row_heights") {
        let hs: Vec<Value> = (0..n_rows)
            .map(|r| e(if header && r == 0 { header_h } else { body_h }))
            .collect();
        t.insert("row_heights".into(), json!(hs));
    }
    for (k, d) in [
        ("header_fill", json!("none")),
        ("header_color", json!("white")),
        ("banded", json!(true)),
        ("band_fill", json!("paper")),
        ("border", json!("none")),
        ("anchor", json!("middle")),
        (
            "cell_margins",
            json!([e(inch(0.25)), e(inch(0.05)), e(inch(0.15)), e(inch(0.05))]),
        ),
        ("style", json!("table")),
    ] {
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
    let mut out = Vec::new();
    if header {
        let mut bar = shape(
            "Table header",
            "roundRect",
            x,
            y,
            w,
            header_h,
            json!("primary"),
            json!("none"),
        );
        bar["radius"] = e(header_h / 5);
        out.push(bar);
    }
    out.push(Value::Object(t));
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
                        "highlight": {"type": "boolean", "description": "Filled with the primary colour."}}}},
                })),
                &["items"],
                "A row of stat cards (big value, label, detail) spread across the box; one may be highlighted.",
            ),
        ),
        (
            "cards",
            closed(
                "cards",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 24, "items": item},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 6},
                    "numbered": {"type": "boolean", "description": "A numbered badge on each card (default true; an item's icon replaces it)."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                    "fill": r("fill"),
                    "highlight": {"type": "integer", "minimum": 1, "description": "This card (from 1) filled with the primary colour."},
                })),
                &["items"],
                "A grid of cards: badge or icon, heading, text or bullets. Short cards put the badge beside the text.",
            ),
        ),
        (
            "steps",
            closed(
                "steps",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 12, "items": item},
                    "current": {"type": "integer", "minimum": 0, "description": "Steps up to this one (from 1) are filled (default 1)."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                })),
                &["items"],
                "Numbered steps down the box, joined by a line.",
            ),
        ),
        (
            "timeline",
            closed(
                "timeline",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 10, "items": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "properties": {
                        "label": {"type": "string", "description": "Small caps tag such as \"Phase 1\"."}, "title": {"type": "string"}, "text": {"type": "string"},
                        "bullets": {"type": "array", "items": {"type": "string"}}, "badge": {"type": "string", "description": "The dot's text (default the number)."}}}]}},
                    "current": {"type": "integer", "minimum": 0, "description": "The current phase, from 1 (default 1)."},
                    "style": {"type": "string", "enum": ["dots", "chevrons"], "description": "Dots on a line (default) or chevrons."},
                    "numbers": numbers, "start": {"type": "integer", "minimum": 0},
                })),
                &["items"],
                "Phases across the box: dots on a line or chevrons, with a label, title and text under each.",
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
                "A tinted box with a bold lead-in and text; by default across the bottom of the content area.",
            ),
        ),
        (
            "numbered_list",
            closed(
                "numbered_list",
                box_schema(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 24, "items": item},
                    "columns": {"type": "integer", "minimum": 1, "maximum": 4},
                    "marker": {"type": "string", "enum": ["badge", "number"], "description": "A filled round badge (default) or a big number."},
                    "boxed": {"type": "boolean", "description": "Each row on a soft rounded box (default true)."},
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
                    "size": {"description": "Diameter (default 0.5in).", "allOf": [r("length")]},
                    "text": text, "icon": {"type": "string"}, "fill": r("fill"), "color": r("color"),
                })),
                &[],
                "A round badge with a number, short text or icon.",
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

    #[std::prelude::v1::test]
    fn components_expand_into_elements() {
        let c = cx();
        for v in [
            json!({"type": "stat_cards", "items": [{"value": "96%", "label": "On time", "detail": "Up 4 points", "highlight": true}, {"value": 42, "label": "Sites"}]}),
            json!({"type": "cards", "columns": 3, "items": [{"title": "One", "text": "a"}, {"title": "Two", "bullets": ["x", "y"], "icon": "rocket"}, "Three"]}),
            json!({"type": "cards", "h": 1.2, "items": ["A", "B"]}),
            json!({"type": "steps", "items": [{"title": "Share data", "text": "Two months"}, "Go live"], "current": 2}),
            json!({"type": "timeline", "items": [{"label": "Phase 1", "title": "Digital work", "text": "Assets"}, {"label": "Later", "title": "Predictive", "badge": "+"}]}),
            json!({"type": "timeline", "style": "chevrons", "items": [{"title": "Plan", "text": "a"}, {"title": "Build"}, {"title": "Run"}], "current": 2}),
            json!({"type": "callout", "tone": "warning", "title": "One option:", "text": "start with utilities", "icon": "lightbulb"}),
            json!({"type": "numbered_list", "columns": 2, "items": [{"title": "Feed pump", "text": "Vibration"}, "Vacuum"]}),
            json!({"type": "numbered_list", "marker": "number", "boxed": false, "items": ["A"]}),
            json!({"type": "comparison", "columns": [{"title": "Today", "items": ["Chat"]}, {"title": "With us", "tag": "After", "items": ["Tickets"], "highlight": true}]}),
            json!({"type": "badge", "x": 1, "y": 1, "text": "3"}),
            json!({"type": "table", "preset": "clean", "columns": [2, 1], "rows": [["Name", "Score"], ["Ann", 9], ["Bo", 7]]}),
        ] {
            let els = expand(&v, &c).unwrap().unwrap_or_else(|| panic!("{v}"));
            assert!(!els.is_empty());
            for el in &els {
                super::super::slides_author::element(el, &c)
                    .unwrap_or_else(|e| panic!("{v}\n{el}\n{e:#}"));
            }
        }
        assert!(expand(&json!({"type": "shape"}), &c).unwrap().is_none());
        assert!(expand(&json!({"type": "cards", "items": []}), &c).is_err());
        assert!(expand(&json!({"type": "callout", "tone": "loud"}), &c).is_err());
        assert!(
            expand(
                &json!({"type": "stat_cards", "items": [{"value": 1, "colour": "red"}]}),
                &c
            )
            .is_err()
        );
        assert!(
            expand(
                &json!({"type": "table", "preset": "fancy", "rows": [["a"]]}),
                &c
            )
            .is_err()
        );
    }
}
