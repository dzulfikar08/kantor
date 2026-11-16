//! SmartArt text shrunk to fit its shapes after Kantor lays a SmartArt out
//! again (resized, items added or removed), as Office's autofit does: each
//! shape's text gets the largest size (in 5 % steps, down to 25 %) at
//! which it fits its text box, and shapes whose text is the same size
//! keep the same size (Office sizes a SmartArt's items alike). The size
//! goes in the shape's `a:bodyPr` as `a:normAutofit/@fontScale`, so the
//! runs keep their own sizes: a SmartArt made bigger again gets its text
//! back up to them.
//!
//! Measuring text needs fonts, which this crate doesn't have: the app
//! hands in a measure ([`set_text_measure`], Slides' scene code); without
//! one a rough estimate is used (tests).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use roxmltree::Document;

use crate::frame::{DiagramDrawing, NS_DSP, Vert};
use crate::model::{EMU_PER_PX, Shape, Xfrm};
use crate::render::RenderText;
use crate::xml::*;

/// How tall (px) a text is laid out `width` px wide.
pub type TextMeasure = fn(&RenderText, f32) -> f32;

static MEASURE: OnceLock<TextMeasure> = OnceLock::new();

/// Give SmartArt fitting the app's text measure (set once).
pub fn set_text_measure(m: TextMeasure) {
    let _ = MEASURE.set(m);
}

/// A text's height laid out `width` px wide: the app's measure, else an
/// estimate (half an em per character, lines 1.2 em).
pub fn measure(t: &RenderText, width: f32) -> f32 {
    if let Some(m) = MEASURE.get() {
        return m(t, width);
    }
    let pt = 4.0 / 3.0;
    t.paras
        .iter()
        .map(|p| {
            let size = p
                .runs
                .iter()
                .map(|r| r.size_pt)
                .fold(p.base.size_pt, f32::max)
                * pt;
            let chars = p.text.chars().count() as f32;
            let lines = if t.wrap {
                (chars * size * 0.5 / width.max(1.0)).ceil().max(1.0)
            } else {
                1.0
            };
            lines * size * 1.2 + (p.space_before_pt + p.space_after_pt) * pt
        })
        .sum()
}

/// Smallest text size autofit goes down to, and its step (100 000 = 100 %).
const MIN_SCALE: u32 = 25_000;
const STEP: u32 = 5_000;

/// The drawing XML of `dd` with each text shape's `fontScale` set so its
/// text fits ([`self`]); `None` when nothing changes. `text` resolves a
/// shape's text for its box (`true`: the shape's own text box,
/// `dsp:txXfrm`), as it's drawn.
pub fn fit_text(
    dd: &DiagramDrawing,
    text: &dyn Fn(&Shape, &Xfrm, bool) -> Option<RenderText>,
) -> Option<String> {
    let g = &dd.group;
    // Each text shape's best scale, and its group (its text's size).
    let mut best: Vec<Option<(u32, u32)>> = vec![None; g.children.len()];
    for (k, shape) in g.children.iter().enumerate() {
        let Some(xfrm) = shape.xfrm else { continue };
        if shape.text.as_ref().is_none_or(|t| t.is_empty()) {
            continue;
        }
        let own = g.text_boxes.get(k).copied().flatten();
        let bx = own.unwrap_or(xfrm);
        let at = |scale: u32| {
            let mut s = shape.clone();
            if let Some(b) = s.text.as_mut() {
                b.body.font_scale = (scale < 100_000).then_some(scale);
                b.body.line_reduction = None;
            }
            text(&s, &bx, own.is_some())
        };
        let Some(full) = at(100_000) else { continue };
        let fits = |t: &RenderText| {
            let [l, tp, r, b] = t.insets;
            let (w, h) = ((bx.cx - l - r) as f32, (bx.cy - tp - b) as f32);
            let (w, h) = match t.vert {
                Vert::Horz => (w, h),
                _ => (h, w),
            };
            measure(t, (w / EMU_PER_PX).max(1.0)) <= h / EMU_PER_PX + 0.5
        };
        let mut scale = 100_000;
        let mut t = full.clone();
        while scale > MIN_SCALE && !fits(&t) {
            scale -= STEP;
            match at(scale) {
                Some(n) => t = n,
                None => break,
            }
        }
        let size = full
            .paras
            .iter()
            .flat_map(|p| p.runs.first())
            .map(|r| (r.size_pt * 10.0).round() as u32)
            .next()
            .unwrap_or(0);
        best[k] = Some((scale, size));
    }
    // Shapes with text of one size share the smallest scale.
    let mut by_size: HashMap<u32, u32> = HashMap::new();
    for (scale, size) in best.iter().flatten() {
        let e = by_size.entry(*size).or_insert(*scale);
        *e = (*e).min(*scale);
    }
    let src: &str = &dd.xml;
    let doc = Document::parse(src).ok()?;
    let tree = doc
        .root_element()
        .children()
        .find(|n| is(*n, NS_DSP, "spTree"))?;
    let shapes: Vec<_> = tree
        .children()
        .filter(|n| n.is_element() && (is(*n, NS_DSP, "sp") || is(*n, NS_DSP, "grpSp")))
        .collect();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for (k, b) in best.iter().enumerate() {
        let Some((_, size)) = b else { continue };
        let scale = by_size[size];
        let now = g.children[k]
            .text
            .as_ref()
            .and_then(|t| t.body.font_scale)
            .unwrap_or(100_000);
        if now == scale {
            continue;
        }
        let Some(sp) = shapes
            .iter()
            .find(|n| n.range() == *dd.ranges.get(k).unwrap_or(&(0..0)))
        else {
            continue;
        };
        let Some(bp) = sp.descendants().find(|n| is(*n, NS_A, "bodyPr")) else {
            continue;
        };
        edits.extend(autofit_edit(src, bp, scale));
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = src.to_owned();
    for (r, t) in edits {
        out.replace_range(r, &t);
    }
    Some(out)
}

/// The edit giving `a:bodyPr` node `bp` the autofit for `scale`.
fn autofit_edit(src: &str, bp: roxmltree::Node, scale: u32) -> Option<(Range<usize>, String)> {
    let tag = start_tag(src, bp);
    let name = tag_name(tag);
    let prefix = name.strip_suffix("bodyPr").unwrap_or("");
    let fit = if scale < 100_000 {
        format!("<{prefix}normAutofit fontScale=\"{scale}\"/>")
    } else {
        format!("<{prefix}normAutofit/>")
    };
    let r = bp.range();
    if let Some(open) = tag.strip_suffix("/>") {
        let open = format!("{}>", open.trim_end());
        return Some((r, format!("{open}{fit}</{name}>")));
    }
    let old = bp.children().find(|n| {
        n.tag_name().namespace() == Some(NS_A)
            && matches!(
                n.tag_name().name(),
                "normAutofit" | "noAutofit" | "spAutoFit"
            )
    });
    match old {
        Some(n) => Some((n.range(), fit)),
        None => {
            let at = r.start + tag.len();
            Some((at..at, fit))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autofit_goes_in_body_props() {
        let src = format!(
            "<a:x xmlns:a=\"{NS_A}\"><a:bodyPr lIns=\"0\"/><a:bodyPr><a:noAutofit/></a:bodyPr><a:bodyPr><a:lnSpc/></a:bodyPr></a:x>"
        );
        let doc = Document::parse(&src).unwrap();
        let bps: Vec<_> = doc
            .descendants()
            .filter(|n| is(*n, NS_A, "bodyPr"))
            .collect();
        let edit = |i: usize, s: u32| {
            let (r, t) = autofit_edit(&src, bps[i], s).unwrap();
            let mut out = src.clone();
            out.replace_range(r, &t);
            out
        };
        assert!(
            edit(0, 80_000)
                .contains("<a:bodyPr lIns=\"0\"><a:normAutofit fontScale=\"80000\"/></a:bodyPr>")
        );
        assert!(
            edit(1, 70_000).contains("<a:bodyPr><a:normAutofit fontScale=\"70000\"/></a:bodyPr>")
        );
        assert!(edit(2, 100_000).contains("<a:bodyPr><a:normAutofit/><a:lnSpc/>"));
    }
}
