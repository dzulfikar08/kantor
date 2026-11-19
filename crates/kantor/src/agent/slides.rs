//! Slides tools on a [`Presentation`]. Slides are numbered from 1 in the
//! tools (as the user sees them), 0-based inside.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use slides_engine::agent::BodyLine;
use slides_engine::{LINE_BREAK, Presentation, ShapeKind};

use super::{check_len, opt_count, opt_int, opt_str, req_int};

/// Bullets one new slide may have.
const MAX_BULLETS: usize = 200;

/// The 0-based index of slide number `args[key]`.
pub fn slide_arg(pres: &Presentation, args: &Value, key: &str) -> Result<usize> {
    let n = req_int(args, key)?;
    if n < 1 || n as usize > pres.len() {
        bail!("there is no slide {n}; the presentation has {}", pres.len());
    }
    Ok(n as usize - 1)
}

fn shape_kind(s: &slides_engine::Shape) -> String {
    match &s.kind {
        ShapeKind::Shape { text_box: true, .. } => "text box".into(),
        ShapeKind::Shape {
            geom: slides_engine::Geom::Line,
            ..
        } => "line".into(),
        ShapeKind::Shape { .. } => "shape".into(),
        ShapeKind::Picture { .. } => "picture".into(),
        ShapeKind::Other { .. } if s.table().is_some() => "table".into(),
        ShapeKind::Other { .. } if s.is_group() => "group".into(),
        ShapeKind::Other { .. } if s.is_chart() => "chart".into(),
        ShapeKind::Other { .. } if s.is_smartart() => "SmartArt".into(),
        ShapeKind::Other { what, .. } => what.clone(),
    }
}

pub fn list(pres: &Presentation, current: usize) -> Value {
    let slides: Vec<Value> = (0..pres.len())
        .map(|i| {
            let slide = pres.slide(i);
            let mut title = String::new();
            let shapes: Vec<Value> = slide
                .shapes
                .iter()
                .enumerate()
                .map(|(ix, s)| {
                    let text = s
                        .text
                        .as_ref()
                        .map(|t| t.plain_text().replace(LINE_BREAK, "\n"));
                    if s.ph.as_ref().is_some_and(|p| p.is_title()) && title.is_empty() {
                        title = text.clone().unwrap_or_default();
                    }
                    let mut v = json!({"id": s.id, "name": s.name, "kind": shape_kind(s)});
                    // Where it is (EMU; placeholders without their own
                    // position: as the layout places them).
                    if let Some(x) = s.xfrm.or_else(|| pres.placed((i, ix)).map(|p| p.xfrm)) {
                        v["x"] = json!(x.x);
                        v["y"] = json!(x.y);
                        v["w"] = json!(x.cx);
                        v["h"] = json!(x.cy);
                        if x.rot != 0 {
                            v["rotation"] = json!(x.rot as f64 / 60_000.0);
                        }
                    }
                    let geometry = match &s.kind {
                        ShapeKind::Shape { .. } => s.kind.geometry(),
                        ShapeKind::Picture { geometry, .. } => geometry.clone(),
                        ShapeKind::Other { .. } => None,
                    };
                    if let Some(name) = geometry.as_ref().and_then(|g| g.preset_name()) {
                        v["geometry"] = json!(name);
                    }
                    if let Some(ph) = &s.ph {
                        v["placeholder"] = json!(ph.ty());
                    }
                    if let Some(text) = text {
                        v["text"] = json!(text);
                        if !s.text_editable() {
                            v["text_locked"] = json!(true);
                        }
                    }
                    v
                })
                .collect();
            let mut v = json!({
                "slide": i + 1,
                "layout": pres.layout_of(i).name,
                "title": title,
                "shapes": shapes,
                "notes": pres.notes_text(i),
            });
            if let Some(why) = &slide.locked {
                v["locked"] = json!(why);
            }
            v
        })
        .collect();
    let layouts: Vec<Value> = pres
        .layouts
        .iter()
        .enumerate()
        .map(|(i, l)| json!({"number": i + 1, "name": l.name}))
        .collect();
    json!({"current_slide": current + 1, "slides": slides, "layouts": layouts})
}

/// The layout `args.layout` names (name or number from 1).
pub fn layout_arg(pres: &Presentation, args: &Value) -> Result<Option<usize>> {
    let names = || {
        pres.layouts
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match args.get("layout") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(i) if i >= 1 && (i as usize) <= pres.layouts.len() => Ok(Some(i as usize - 1)),
            _ => bail!("there is no layout {n}; layouts: {}", names()),
        },
        Some(Value::String(name)) => pres
            .layouts
            .iter()
            .position(|l| l.name.eq_ignore_ascii_case(name.trim()))
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("there is no layout \"{name}\"; layouts: {}", names())),
        Some(_) => bail!("`layout` must be a layout name or number"),
    }
}

/// What `slides_add_slide` adds.
#[derive(Debug, PartialEq)]
pub struct AddPlan {
    pub after: Option<usize>,
    pub layout: Option<usize>,
    pub title: Option<String>,
    pub body: Vec<BodyLine>,
}

pub fn plan_add(pres: &Presentation, args: &Value) -> Result<AddPlan> {
    let after = match opt_int(args, "after_slide")? {
        None => pres.len().checked_sub(1),
        Some(0) => None,
        Some(n) if n >= 1 && n as usize <= pres.len() => Some(n as usize - 1),
        Some(n) => bail!(
            "`after_slide` {n} is not a slide (there are {})",
            pres.len()
        ),
    };
    let title = opt_str(args, "title")?.map(str::to_owned);
    if let Some(t) = &title {
        check_len("the title", t)?;
    }
    let mut body = Vec::new();
    if let Some(items) = args.get("bullets").filter(|v| !v.is_null()) {
        let Some(items) = items.as_array() else {
            bail!("`bullets` must be an array");
        };
        if items.len() > MAX_BULLETS {
            bail!("more than {MAX_BULLETS} bullets on one slide");
        }
        for item in items {
            let (text, level) = match item {
                Value::String(s) => (s.as_str(), 0),
                obj => (
                    super::req_str(obj, "text")?,
                    opt_count(obj, "level", 0)?.unwrap_or(0).min(8) as u8,
                ),
            };
            check_len("a bullet", text)?;
            body.push(BodyLine {
                text: text.to_owned(),
                level,
            });
        }
    }
    Ok(AddPlan {
        after,
        layout: layout_arg(pres, args)?,
        title,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn adds_slides_and_lists_them() {
        let mut pres = Presentation::new();
        let plan = plan_add(
            &pres,
            &json!({"title": "Plan", "bullets": ["One", {"text": "Sub", "level": 1}]}),
        )
        .unwrap();
        assert_eq!(plan.after, Some(pres.len() - 1));
        assert_eq!(plan.body[1].level, 1);
        let i = pres
            .add_slide_with_text(plan.after, plan.layout, plan.title.as_deref(), &plan.body)
            .unwrap();
        let out = list(&pres, i);
        assert_eq!(out["current_slide"], i + 1);
        let slide = &out["slides"][i];
        assert_eq!(slide["title"], "Plan");
        assert!(
            slide["shapes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["text"] == "One\nSub")
        );
        assert!(!out["layouts"].as_array().unwrap().is_empty());
        assert_eq!(
            slide_arg(&pres, &json!({"slide": i + 1}), "slide").unwrap(),
            i
        );
        assert!(slide_arg(&pres, &json!({"slide": 0}), "slide").is_err());
        assert!(plan_add(&pres, &json!({"layout": "No Such Layout"})).is_err());
        let name = pres.layouts[0].name.to_uppercase();
        assert_eq!(
            plan_add(&pres, &json!({"layout": name})).unwrap().layout,
            Some(0)
        );
        assert_eq!(
            plan_add(&pres, &json!({"after_slide": 0})).unwrap().after,
            None
        );
    }
}
