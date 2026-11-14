//! Charts generated here, laid out with a fixed-width measure (no fonts).

use slides_engine::Rgb;
use slides_engine::theme::{ClrMap, ColorCtx, Theme};

use super::layout::*;
use super::model::*;
use crate::{Paint, Prim};

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme name="t"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F1F1F"/></a:dk2><a:lt2><a:srgbClr val="EEEEEE"/></a:lt2><a:accent1><a:srgbClr val="110000"/></a:accent1><a:accent2><a:srgbClr val="220000"/></a:accent2><a:accent3><a:srgbClr val="330000"/></a:accent3><a:accent4><a:srgbClr val="440000"/></a:accent4><a:accent5><a:srgbClr val="550000"/></a:accent5><a:accent6><a:srgbClr val="660000"/></a:accent6></a:clrScheme><a:fontScheme name="f"><a:majorFont><a:latin typeface="Major Face"/></a:majorFont><a:minorFont><a:latin typeface="Minor Face"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;

fn with_cx<R>(f: impl FnOnce(&ColorCtx) -> R) -> R {
    let theme = Theme::parse(THEME);
    let map = ClrMap::default();
    f(&ColorCtx {
        theme: &theme,
        map: &map,
        ph: None,
    })
}

/// Every character is half the font size wide; lines are 1.2 × tall.
struct Fixed;

impl Measure for Fixed {
    fn measure(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32) {
        let em = font.size_pt * PX_PER_PT;
        let cw = em * 0.5;
        let mut lines = 0;
        let mut widest: f32 = 0.0;
        for para in text.split('\n') {
            let mut line = 0.0f32;
            lines += 1;
            for (i, word) in para.split(' ').enumerate() {
                let ww = word.chars().count() as f32 * cw;
                let add = if i == 0 { ww } else { ww + cw };
                if let Some(w) = wrap
                    && line > 0.0
                    && line + add > w
                {
                    widest = widest.max(line);
                    lines += 1;
                    line = ww;
                } else {
                    line += add;
                }
            }
            widest = widest.max(line);
        }
        (widest, lines as f32 * em * 1.2)
    }
}

fn cats(names: &[&str]) -> String {
    let pts: String = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!(r#"<c:pt idx="{i}"><c:v>{n}</c:v></c:pt>"#))
        .collect();
    format!(
        r#"<c:cat><c:strRef><c:f>S!A2</c:f><c:strCache><c:ptCount val="{}"/>{pts}</c:strCache></c:strRef></c:cat>"#,
        names.len()
    )
}

fn nums(tag: &str, vals: &[f64], fmt: &str) -> String {
    let pts: String = vals
        .iter()
        .enumerate()
        .map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#))
        .collect();
    format!(
        r#"<c:{tag}><c:numRef><c:f>S!B2</c:f><c:numCache><c:formatCode>{fmt}</c:formatCode><c:ptCount val="{}"/>{pts}</c:numCache></c:numRef></c:{tag}>"#,
        vals.len()
    )
}

fn ser(i: usize, name: &str, c: &[&str], vals: &[f64], extra: &str) -> String {
    format!(
        r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:strRef><c:f>S!B1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{name}</c:v></c:pt></c:strCache></c:strRef></c:tx>{extra}{}{}</c:ser>"#,
        cats(c),
        nums("val", vals, "General")
    )
}

const AXES: &str = r#"<c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/><c:crosses val="autoZero"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="1"/><c:crossAx val="1"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx>"#;

fn space(plot: &str, extra: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:chart>{extra}<c:plotArea><c:layout/>{plot}</c:plotArea><c:legend><c:legendPos val="r"/></c:legend><c:plotVisOnly val="1"/></c:chart><c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1000"/></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr></c:chartSpace>"#
    )
}

const CATS: [&str; 3] = ["Alpha", "Beta", "Gamma"];

fn bar_chart(grouping: &str, dir: &str, extra_group: &str) -> String {
    space(
        &format!(
            r#"<c:barChart><c:barDir val="{dir}"/><c:grouping val="{grouping}"/><c:varyColors val="0"/>{}{}{extra_group}<c:gapWidth val="100"/><c:axId val="1"/><c:axId val="2"/></c:barChart>{AXES}"#,
            ser(0, "First", &CATS, &[10.0, 20.0, 30.0], ""),
            ser(1, "Second", &CATS, &[5.0, -5.0, 15.0], ""),
        ),
        r#"<c:autoTitleDeleted val="1"/>"#,
    )
}

fn parse(xml: &str) -> Chart {
    with_cx(|cx| Chart::parse(xml, cx)).expect("a chart")
}

fn marks(xml: &str, w: f32, h: f32) -> Vec<Mark> {
    let chart = parse(xml);
    assert!(chart.drawable(), "{:?}", chart.unsupported);
    let d = with_cx(super::defaults);
    layout(&chart, w, h, &d, &mut Fixed)
}

fn labels(m: &[Mark]) -> Vec<&Label> {
    m.iter()
        .filter_map(|m| match m {
            Mark::Text(l) => Some(l),
            _ => None,
        })
        .collect()
}

fn label<'a>(m: &'a [Mark], text: &str) -> &'a Label {
    labels(m)
        .into_iter()
        .find(|l| l.text == text)
        .unwrap_or_else(|| panic!("no label {text:?} in {:?}", texts(m)))
}

fn texts(m: &[Mark]) -> Vec<String> {
    labels(m).iter().map(|l| l.text.clone()).collect()
}

/// Solid filled boxes `(colour, [x0, y0, x1, y1])` in frame px.
fn solid_boxes(m: &[Mark]) -> Vec<(Rgb, [f32; 4])> {
    m.iter()
        .filter_map(|m| match m {
            Mark::Prim {
                prim:
                    Prim::Fill {
                        path,
                        paint: Paint::Solid(c),
                        ..
                    },
                at,
            } => {
                let b = path.transformed(at).bounds()?;
                Some((Rgb([c.0[0], c.0[1], c.0[2]]), b))
            }
            _ => None,
        })
        .collect()
}

/// Boxes of colour `c` after the legend's key (the legend comes first).
fn drawn(boxes: &[(Rgb, [f32; 4])], c: Rgb) -> Vec<[f32; 4]> {
    boxes
        .iter()
        .filter(|b| b.0 == c)
        .skip(1)
        .map(|b| b.1)
        .collect()
}

const A1: Rgb = Rgb([0x11, 0, 0]);
const A2: Rgb = Rgb([0x22, 0, 0]);

#[test]
fn theme_colours_cycle_and_vary() {
    let d = with_cx(super::defaults);
    assert_eq!(d.minor, "Minor Face");
    assert_eq!(d.text, Rgb([0, 0, 0]));
    for i in 0..6 {
        assert_eq!(auto_color(&d, i), d.accents[i]);
    }
    assert_eq!(d.accents[0], A1);
    // The seventh series: the first accent, darker.
    let c = auto_color(&d, 6);
    assert!(c.0[0] < A1.0[0] && c != A1);
    assert_ne!(auto_color(&d, 12), auto_color(&d, 6));
}

#[test]
fn stacking_maths() {
    let a = [Some(10.0), Some(-4.0), None];
    let b = [Some(5.0), Some(-6.0), Some(3.0)];
    let s = stacked_values(&[&a, &b], Grouping::Stacked);
    assert_eq!(s[0], vec![(0.0, 10.0), (0.0, -4.0), (0.0, 0.0)]);
    // Positives on positives, negatives below negatives.
    assert_eq!(s[1], vec![(10.0, 15.0), (-4.0, -10.0), (0.0, 3.0)]);
    let p = stacked_values(&[&a, &b], Grouping::PercentStacked);
    assert!((p[1][0].1 - 1.0).abs() < 1e-9);
    assert!((p[0][0].1 - 10.0 / 15.0).abs() < 1e-9);
    assert!((p[1][1].1 + 1.0).abs() < 1e-9);
    let c = stacked_values(&[&a, &b], Grouping::Clustered);
    assert_eq!(c[1][1], (0.0, -6.0));
}

#[test]
fn pie_angles_follow_values() {
    let a = pie_angles(&[Some(1.0), Some(1.0), Some(2.0)], 0.0);
    // Clockwise from 12 o'clock (−90° from 3 o'clock).
    assert_eq!(a, vec![(-90.0, 90.0), (0.0, 90.0), (90.0, 180.0)]);
    let b = pie_angles(&[Some(3.0), None, Some(-1.0), Some(1.0)], 90.0);
    assert_eq!(b[0], (0.0, 270.0));
    assert_eq!(b[1].1, 0.0);
    assert_eq!(b[2].1, 0.0);
    assert_eq!(b[3], (270.0, 90.0));
}

#[test]
fn legend_layouts() {
    let sizes = [(40.0, 10.0), (60.0, 10.0), (20.0, 10.0)];
    let area = [0.0, 0.0, 400.0, 300.0];
    // Right: one column at the right edge, centred vertically.
    let r = legend_layout(LegendPos::R, &sizes, 8.0, area, 6.0);
    assert_eq!(r.items.len(), 3);
    let right = r.rect[0] + r.rect[2];
    assert!((right - 400.0).abs() < 1e-3, "{r:?}");
    assert!(r.items.windows(2).all(|w| w[1].0[1] > w[0].0[1]));
    let mid = r.rect[1] + r.rect[3] / 2.0;
    assert!((mid - 150.0).abs() < 1.0, "{r:?}");
    // Bottom: a centred row.
    let b = legend_layout(LegendPos::B, &sizes, 8.0, area, 6.0);
    assert!(
        b.items
            .iter()
            .all(|(k, _)| (k[1] - b.items[0].0[1]).abs() < 1e-3)
    );
    let left = b.items[0].0[0];
    let end = b.items[2].1[0] + b.items[2].1[2];
    assert!((left - (400.0 - end)).abs() < 1e-3, "centred: {b:?}");
    assert!(b.rect[1] + b.rect[3] <= 300.0 + 1e-3);
    // Narrow: rows wrap.
    let n = legend_layout(LegendPos::T, &sizes, 8.0, [0.0, 0.0, 100.0, 300.0], 6.0);
    assert!(n.items[1].0[1] > n.items[0].0[1], "{n:?}");
    assert!(n.items[0].0[1] < 2.0, "top");
}

#[test]
fn clustered_columns_with_labels_and_axes() {
    let xml = bar_chart(
        "clustered",
        "col",
        r#"<c:dLbls><c:numFmt formatCode="0.0" sourceLinked="0"/><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls>"#,
    );
    let m = marks(&xml, 600.0, 400.0);
    let t = texts(&m);
    for c in CATS {
        assert!(t.contains(&c.to_owned()), "{t:?}");
    }
    assert!(t.contains(&"First".into()) && t.contains(&"Second".into()));
    // Data labels with their format; axis −10..35 by 5 (auto scale).
    assert!(
        t.contains(&"30.0".into()) && t.contains(&"-5.0".into()),
        "{t:?}"
    );
    assert!(
        t.contains(&"35".into()) && t.contains(&"-10".into()),
        "{t:?}"
    );
    // Bars: accent colours in series order, Gamma's first bar the tallest.
    let boxes = solid_boxes(&m);
    let first = drawn(&boxes, A1);
    let second = drawn(&boxes, A2);
    assert_eq!((first.len(), second.len()), (3, 3));
    let h = |b: &[f32; 4]| b[3] - b[1];
    assert!(h(&first[2]) > h(&first[1]) && h(&first[1]) > h(&first[0]));
    // The negative bar hangs below the zero line, where the positive ones start.
    let zero = first[0][3];
    assert!((second[1][1] - zero).abs() < 0.01 && second[1][3] > zero);
    // Value labels above positive bars, below the negative one.
    assert!(label(&m, "30.0").y + label(&m, "30.0").h <= first[2][1] + 0.01);
    assert!(label(&m, "-5.0").y >= second[1][3] - 0.01);
    // Category labels below the plot, under their bars.
    let beta = label(&m, "Beta");
    let cx = beta.x + beta.w / 2.0;
    assert!(cx > first[1][0] && cx < second[1][2]);
    // No title (deleted).
    assert!(!t.iter().any(|s| s == "Chart Title"));
}

#[test]
fn stacked_and_percent_bars() {
    let m = marks(
        &bar_chart("stacked", "bar", "<c:overlap val=\"100\"/>"),
        600.0,
        400.0,
    );
    let boxes = solid_boxes(&m);
    let (a, b) = (drawn(&boxes, A1), drawn(&boxes, A2));
    // Horizontal: Alpha's second bar continues where the first ends.
    assert!((b[0][0] - a[0][2]).abs() < 0.01, "{a:?} {b:?}");
    assert!((b[0][1] - a[0][1]).abs() < 0.01, "same row");
    // Beta's negative part goes left of zero.
    assert!((b[1][2] - a[1][0]).abs() < 0.01, "{a:?} {b:?}");
    let m = marks(
        &bar_chart("percentStacked", "col", "<c:overlap val=\"100\"/>"),
        600.0,
        400.0,
    );
    let t = texts(&m);
    assert!(
        t.contains(&"100%".into()) && t.contains(&"0%".into()),
        "{t:?}"
    );
    let boxes = solid_boxes(&m);
    let (a, b) = (drawn(&boxes, A1), drawn(&boxes, A2));
    // Alpha: 10 / 15 and 5 / 15 of the plot's height, together the whole.
    let (ha, hb) = (a[0][3] - a[0][1], b[0][3] - b[0][1]);
    assert!((ha / (ha + hb) - 2.0 / 3.0).abs() < 1e-3);
}

#[test]
fn explicit_fills_points_and_titles() {
    let sp = r#"<c:spPr><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></c:spPr><c:dPt><c:idx val="1"/><c:bubble3D val="0"/><c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr></c:dPt>"#;
    let xml = space(
        &format!(
            r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>{}<c:axId val="1"/><c:axId val="2"/></c:barChart>{AXES}"#,
            ser(0, "Only", &CATS, &[1.0, 2.0, 3.0], sp)
        ),
        "",
    );
    let m = marks(&xml, 500.0, 300.0);
    let boxes = solid_boxes(&m);
    let green = boxes.iter().filter(|b| b.0 == Rgb([0, 255, 0])).count();
    assert_eq!(green, 2 + 1, "two bars and the legend key");
    assert_eq!(boxes.iter().filter(|b| b.0 == A2).count(), 1, "the dPt");
    // One series and no title element: its name is the title.
    let t = label(&m, "Only");
    assert!(t.font.bold && t.y < 20.0);
    // A rich title wins, with its own size.
    let xml2 = xml.replace(
        "<c:plotArea>",
        r#"<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr sz="2400" b="0"/><a:t>Big title</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/><c:plotArea>"#,
    );
    let m2 = marks(&xml2, 500.0, 300.0);
    let t2 = label(&m2, "Big title");
    assert_eq!((t2.font.size_pt, t2.font.bold), (24.0, false));
    assert_eq!(t2.font.face, "Minor Face");
}

#[test]
fn lines_with_markers_and_combo_secondary_axis() {
    let line = |ids: (u32, u32), i: usize, vals: &[f64]| {
        format!(
            r#"<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>{}<c:marker val="1"/><c:axId val="{}"/><c:axId val="{}"/></c:lineChart>"#,
            ser(
                i,
                &format!("Line{i}"),
                &CATS,
                vals,
                r#"<c:marker><c:symbol val="circle"/><c:size val="7"/></c:marker>"#
            ),
            ids.0,
            ids.1
        )
    };
    let secondary = r#"<c:valAx><c:axId val="4"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="r"/><c:numFmt formatCode="0%" sourceLinked="0"/><c:crossAx val="3"/><c:crosses val="max"/><c:crossBetween val="between"/></c:valAx><c:catAx><c:axId val="3"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="1"/><c:axPos val="b"/><c:crossAx val="4"/><c:crosses val="autoZero"/></c:catAx>"#;
    let xml = space(
        &format!(
            r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>{}<c:axId val="1"/><c:axId val="2"/></c:barChart>{}{AXES}{secondary}"#,
            ser(0, "Bars", &CATS, &[100.0, 200.0, 300.0], ""),
            line((3, 4), 1, &[0.2, 0.5, 0.9]),
        ),
        r#"<c:autoTitleDeleted val="1"/>"#,
    );
    let chart = parse(&xml);
    assert_eq!(chart.groups.len(), 2);
    let m = marks(&xml, 600.0, 400.0);
    let t = texts(&m);
    // Both value axes: the primary's numbers, the secondary's percentages.
    assert!(
        t.contains(&"300".into()) && t.contains(&"100%".into()),
        "{t:?}"
    );
    let pct = label(&m, "100%");
    let num = label(&m, "300");
    assert!(pct.x > num.x + 300.0, "secondary axis on the right");
    // Seven-point markers on the line (circles: four cubic arcs).
    let markers = m
        .iter()
        .filter(|m| matches!(m, Mark::Prim { prim: Prim::Fill { path, paint: Paint::Solid(c), .. }, .. } if c.0[0] == 0x22 && path.segs.len() == 6))
        .count();
    assert_eq!(markers, 3 + 1, "points and legend key");
    // The line itself.
    assert!(m.iter().any(|m| matches!(m, Mark::Prim { prim: Prim::Stroke { stroke, .. }, .. } if stroke.color.0[0] == 0x22 && stroke.width > 2.0)));
}

#[test]
fn pies_and_doughnuts() {
    let pie = |kind: &str, extra: &str| {
        space(
            &format!(
                r#"<c:{kind}><c:varyColors val="1"/>{}<c:dLbls><c:showLegendKey val="0"/><c:showVal val="0"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="1"/><c:showBubbleSize val="0"/></c:dLbls><c:firstSliceAng val="0"/>{extra}</c:{kind}>"#,
                ser(
                    0,
                    "Share",
                    &CATS,
                    &[1.0, 1.0, 2.0],
                    r#"<c:dPt><c:idx val="2"/><c:bubble3D val="0"/><c:explosion val="20"/></c:dPt>"#
                )
            ),
            r#"<c:autoTitleDeleted val="1"/>"#,
        )
    };
    let m = marks(&pie("pieChart", ""), 400.0, 300.0);
    let t = texts(&m);
    assert!(
        t.contains(&"25%".into()) && t.contains(&"50%".into()),
        "{t:?}"
    );
    // Legend: the categories, coloured like their slices.
    for c in CATS {
        assert!(t.contains(&c.to_owned()), "{t:?}");
    }
    let boxes = solid_boxes(&m);
    for a in [A1, A2, Rgb([0x33, 0, 0])] {
        assert_eq!(boxes.iter().filter(|b| b.0 == a).count(), 2, "slice + key");
    }
    // The exploded half sits apart from the centre.
    let gamma = drawn(&boxes, Rgb([0x33, 0, 0]))[0];
    let alpha = drawn(&boxes, A1)[0];
    assert!(
        gamma[0] < alpha[0] - 1.0,
        "exploded to the left: {gamma:?} {alpha:?}"
    );
    let d = parse(&pie("doughnutChart", r#"<c:holeSize val="60"/>"#));
    assert!(matches!(d.groups[0].kind, Kind::Pie { hole: Some(h), .. } if h == 60.0));
    let m = marks(
        &pie("doughnutChart", r#"<c:holeSize val="60"/>"#),
        400.0,
        300.0,
    );
    assert!(texts(&m).contains(&"25%".into()));
}

#[test]
fn scatter_area_and_unsupported() {
    let xs = nums("xVal", &[1.0, 2.0, 4.0], "General");
    let ys = nums("yVal", &[3.0, 1.0, 2.0], "General");
    let xml = space(
        &format!(
            r#"<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/><c:ser><c:idx val="0"/><c:order val="0"/>{xs}{ys}<c:smooth val="0"/></c:ser><c:axId val="5"/><c:axId val="6"/></c:scatterChart><c:valAx><c:axId val="5"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="6"/></c:valAx><c:valAx><c:axId val="6"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:crossAx val="5"/></c:valAx>"#
        ),
        r#"<c:autoTitleDeleted val="1"/>"#,
    );
    let m = marks(&xml, 400.0, 300.0);
    let t = texts(&m);
    // Both axes numeric: x 0..5, y 0..3.5.
    assert!(
        t.contains(&"4".into()) && t.contains(&"3.5".into()),
        "{t:?}"
    );
    assert!(t.contains(&"Series1".into()), "unnamed series: {t:?}");
    let area = space(
        &format!(
            r#"<c:areaChart><c:grouping val="stacked"/><c:varyColors val="0"/>{}{}<c:axId val="1"/><c:axId val="2"/></c:areaChart>{}"#,
            ser(0, "A", &CATS, &[1.0, 2.0, 3.0], ""),
            ser(1, "B", &CATS, &[1.0, 1.0, 1.0], ""),
            AXES.replace("between", "midCat")
        ),
        "",
    );
    let m = marks(&area, 400.0, 300.0);
    assert_eq!(solid_boxes(&m).iter().filter(|b| b.0 == A2).count(), 2);
    // Radar: not drawn (placeholder).
    let radar = space(
        r#"<c:radarChart><c:radarStyle val="marker"/></c:radarChart>"#,
        "",
    );
    let c = parse(&radar);
    assert!(!c.drawable());
}

#[test]
fn crowded_categories_rotate() {
    let names: Vec<String> = (0..12).map(|i| format!("Month{i:02}LongName")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let vals: Vec<f64> = (0..12).map(f64::from).collect();
    let xml = space(
        &format!(
            r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>{}<c:axId val="1"/><c:axId val="2"/></c:barChart>{AXES}"#,
            ser(0, "S", &refs, &vals, "")
        ),
        "",
    );
    let m = marks(&xml, 400.0, 300.0);
    let rotated: Vec<_> = labels(&m).into_iter().filter(|l| l.rot != 0.0).collect();
    assert!(!rotated.is_empty());
    assert!(rotated.iter().all(|l| l.rot == -45.0));
}

/// A horizontal bar chart as openpyxl writes it: both axes `axPos="l"`, no
/// `c:crosses`. `cat_scaling` is the category axis' `c:scaling`, `val_extra`
/// goes after the value axis' `c:crossAx`.
fn hbar(cat_scaling: &str, val_extra: &str) -> String {
    space(
        &format!(
            r#"<c:barChart><c:barDir val="bar"/><c:grouping val="clustered"/>{}<c:gapWidth val="60"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling>{cat_scaling}</c:scaling><c:axPos val="l"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:axPos val="l"/><c:majorGridlines/><c:crossAx val="1"/>{val_extra}</c:valAx>"#,
            ser(0, "First", &CATS, &[100.0, 300.0, 600.0], ""),
        ),
        r#"<c:autoTitleDeleted val="1"/>"#,
    )
}

/// The plot's top and bottom: the top and bottom of the bars' rows.
fn bar_rows(m: &[Mark]) -> Vec<[f32; 4]> {
    drawn(&solid_boxes(m), A1)
}

#[test]
fn horizontal_bar_value_labels_sit_outside_the_plot() {
    let orient = |o: &str| format!(r#"<c:orientation val="{o}"/>"#);
    let below = |m: &[Mark], text: &str| {
        let bars = bar_rows(m);
        let bottom = bars.iter().map(|b| b[3]).fold(f32::MIN, f32::max);
        let top = bars.iter().map(|b| b[1]).fold(f32::MAX, f32::min);
        let l = label(m, text);
        (l.y >= bottom - 0.01, l.y + l.h <= top + 0.01)
    };
    // Default: first category at the bottom, value labels below the plot.
    let m = marks(&hbar(&orient("minMax"), ""), 600.0, 400.0);
    let bars = bar_rows(&m);
    assert!(bars[0][1] > bars[2][1], "Alpha at the bottom: {bars:?}");
    assert_eq!(below(&m, "600"), (true, false), "{:?}", label(&m, "600"));
    // Categories reversed (first at the top): the value axis crosses at the
    // first category, so it and its labels move above the plot, as in Excel,
    // and stay off the bars.
    let m = marks(&hbar(&orient("maxMin"), ""), 600.0, 400.0);
    let bars = bar_rows(&m);
    assert!(bars[0][1] < bars[2][1], "Alpha at the top: {bars:?}");
    assert_eq!(below(&m, "600"), (false, true), "{:?}", label(&m, "600"));
    // Reversed with "crosses at maximum category": labels back at the bottom.
    let m = marks(
        &hbar(&orient("maxMin"), r#"<c:crosses val="max"/>"#),
        600.0,
        400.0,
    );
    assert_eq!(below(&m, "600"), (true, false), "{:?}", label(&m, "600"));
    // Tick labels "high" go to the top edge, "low" to the bottom.
    let m = marks(
        &hbar(&orient("minMax"), r#"<c:tickLblPos val="high"/>"#),
        600.0,
        400.0,
    );
    assert_eq!(below(&m, "600"), (false, true), "{:?}", label(&m, "600"));
    let m = marks(
        &hbar(&orient("maxMin"), r#"<c:tickLblPos val="low"/>"#),
        600.0,
        400.0,
    );
    assert_eq!(below(&m, "600"), (false, true), "low end of a reversed axis");
}
