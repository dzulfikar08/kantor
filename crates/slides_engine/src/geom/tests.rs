use super::*;

fn f(s: &str) -> f64 {
    Formula::parse(s).unwrap().eval(|a| match a {
        Arg::Lit(v) => *v as f64,
        Arg::Ref(n) => panic!("unexpected {n}"),
    })
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn every_operator() {
    assert_eq!(f("*/ 6 10 4"), 15.0);
    assert_eq!(f("*/ 6 10 0"), 0.0, "division by zero is 0");
    assert_eq!(f("+- 6 10 4"), 12.0);
    assert_eq!(f("+/ 6 10 4"), 4.0);
    assert_eq!(f("?: 1 2 3"), 2.0);
    assert_eq!(f("?: 0 2 3"), 3.0);
    assert_eq!(f("?: -1 2 3"), 3.0);
    assert_eq!(f("abs -7"), 7.0);
    assert!(near(f("at2 1 1"), 2_700_000.0, 1e-6));
    assert!(near(f("at2 -1 0"), 10_800_000.0, 1e-6));
    assert!(near(f("cat2 10 1 1"), 10.0 * 0.5f64.sqrt(), 1e-9));
    assert!(near(f("sat2 10 1 1"), 10.0 * 0.5f64.sqrt(), 1e-9));
    assert!(near(f("cos 10 5400000"), 0.0, 1e-9));
    assert!(near(f("cos 10 10800000"), -10.0, 1e-9));
    assert!(near(f("sin 10 5400000"), 10.0, 1e-9));
    assert!(near(f("tan 10 2700000"), 10.0, 1e-9));
    assert_eq!(f("max 3 9"), 9.0);
    assert_eq!(f("min 3 9"), 3.0);
    assert_eq!(f("mod 2 3 6"), 7.0);
    assert_eq!(f("pin 0 50 10"), 10.0);
    assert_eq!(f("pin 0 -5 10"), 0.0);
    assert_eq!(f("pin 0 5 10"), 5.0);
    assert_eq!(f("sqrt 49"), 7.0);
    assert_eq!(f("val -12"), -12.0);
    assert!(Formula::parse("nope 1").is_none());
    assert_eq!(
        Formula::parse("*/  ss adj 100000").unwrap().to_string(),
        "*/ ss adj 100000"
    );
}

#[test]
fn built_in_variables() {
    let b = |n| builtin(n, 200.0, 100.0).unwrap();
    assert_eq!(
        (b("w"), b("h"), b("l"), b("t"), b("r"), b("b")),
        (200.0, 100.0, 0.0, 0.0, 200.0, 100.0)
    );
    assert_eq!(
        (b("hc"), b("vc"), b("ss"), b("ls")),
        (100.0, 50.0, 100.0, 200.0)
    );
    assert_eq!(
        (b("wd2"), b("hd3"), b("ssd8"), b("wd32")),
        (100.0, 100.0 / 3.0, 12.5, 6.25)
    );
    assert_eq!(
        (b("cd2"), b("cd4"), b("cd8")),
        (10_800_000.0, 5_400_000.0, 2_700_000.0)
    );
    assert_eq!(
        (b("3cd4"), b("3cd8"), b("5cd8"), b("7cd8")),
        (16_200_000.0, 8_100_000.0, 13_500_000.0, 18_900_000.0)
    );
    assert_eq!(b("5cd4"), 27_000_000.0);
    assert!(builtin("adj", 1.0, 1.0).is_none());
}

fn def(av: &[(&str, &str)], gd: &[(&str, &str)]) -> ShapeDef {
    let g = |l: &[(&str, &str)]| l.iter().map(|(n, f)| Guide::new(n, f).unwrap()).collect();
    ShapeDef {
        av: g(av),
        gd: g(gd),
        rect: Some(["a", "b", "c", "d"].map(Arg::parse)),
        paths: Vec::new(),
    }
}

#[test]
fn guides_evaluate_in_order_and_adjustments_override() {
    let d = def(
        &[("adj", "val 25000")],
        &[
            // `d` refers to `c` before it's defined: 0 then.
            ("d", "+- c 1 0"),
            ("a", "*/ w adj 100000"),
            ("b", "+- a 10 0"),
            ("c", "+- b b 0"),
            // A later guide may redefine an earlier name.
            ("a", "val 7"),
        ],
    );
    let o = evaluate(&d, &[], 200.0, 100.0);
    assert_eq!(
        o.text_rect,
        Rect {
            l: 7.0,
            t: 60.0,
            r: 120.0,
            b: 1.0
        }
    );
    let o = evaluate(&d, &[Guide::new("adj", "val 50000").unwrap()], 200.0, 100.0);
    assert_eq!((o.text_rect.t, o.text_rect.r), (110.0, 220.0));
    // Overrides may be formulas over built-ins too.
    let o = evaluate(
        &d,
        &[Guide::new("adj", "*/ h 1000 1").unwrap()],
        200.0,
        100.0,
    );
    assert_eq!(o.text_rect.t, 210.0);
}

/// Every vertex the shape's flattened outline passes through (rounded).
fn vertices(o: &Outline) -> Vec<(i64, i64)> {
    o.paths
        .iter()
        .flat_map(|p| p.flatten(0.01))
        .flat_map(|(c, _)| c)
        .map(|(x, y)| (x.round() as i64, y.round() as i64))
        .collect()
}

fn prst(name: &str, w: f64, h: f64) -> Outline {
    assert!(preset(name).is_some(), "no preset {name}");
    outline(&Geometry::preset(name), w, h)
}

fn adjusted(name: &str, adj: &[(&str, i64)], w: f64, h: f64) -> Outline {
    let adjust = adj
        .iter()
        .map(|(n, v)| Guide::new(n, &format!("val {v}")).unwrap())
        .collect();
    outline(
        &Geometry::Preset {
            name: name.into(),
            adjust,
        },
        w,
        h,
    )
}

fn assert_polygon(name: &str, w: f64, h: f64, expect: &[(i64, i64)]) {
    let o = prst(name, w, h);
    let v = vertices(&o);
    for p in expect {
        assert!(v.contains(p), "{name}: {p:?} not in {v:?}");
    }
}

fn bounds(o: &Outline) -> (i64, i64, i64, i64) {
    let b = o.bounds().unwrap();
    (
        b.l.round() as i64,
        b.t.round() as i64,
        b.r.round() as i64,
        b.b.round() as i64,
    )
}

#[test]
fn all_presets_evaluate() {
    let names = preset_names();
    assert_eq!(names.len(), 187);
    for n in names {
        for (w, h) in [(300.0, 200.0), (100.0, 400.0), (0.0, 0.0)] {
            let o = prst(n, w, h);
            assert!(!o.paths.is_empty(), "{n}");
            for p in &o.paths {
                for (c, _) in p.flatten(1.0) {
                    for (x, y) in c {
                        assert!(x.is_finite() && y.is_finite(), "{n} at {w}×{h}");
                    }
                }
            }
        }
        // Everything but callout tails and connectors stays inside its box
        // (curves of `heart` and `cloud` bulge out by up to 0.5 %).
        let o = prst(n, 300.0, 200.0);
        let b = o.bounds().unwrap();
        let inside = b.l >= -1.5 && b.t >= -1.5 && b.r <= 301.5 && b.b <= 201.5;
        let overhang = n.contains("Callout") || n.starts_with("callout") || n.contains("Connector");
        assert!(inside || overhang, "{n}: {b:?}");
    }
}

#[test]
fn polygons_at_known_sizes() {
    assert_polygon(
        "rect",
        200.0,
        100.0,
        &[(0, 0), (200, 0), (200, 100), (0, 100)],
    );
    assert_polygon("triangle", 200.0, 100.0, &[(0, 100), (100, 0), (200, 100)]);
    assert_polygon("rtTriangle", 200.0, 100.0, &[(0, 0), (0, 100), (200, 100)]);
    assert_polygon(
        "diamond",
        200.0,
        100.0,
        &[(0, 50), (100, 0), (200, 50), (100, 100)],
    );
    // adj 25000 of ss (100): 25 in from the side.
    assert_polygon(
        "parallelogram",
        200.0,
        100.0,
        &[(0, 100), (25, 0), (200, 0), (175, 100)],
    );
    assert_polygon(
        "trapezoid",
        200.0,
        100.0,
        &[(0, 100), (25, 0), (175, 0), (200, 100)],
    );
    // adj 50000 of ss.
    assert_polygon(
        "rightArrow",
        200.0,
        100.0,
        &[
            (0, 25),
            (150, 25),
            (150, 0),
            (200, 50),
            (150, 100),
            (150, 75),
            (0, 75),
        ],
    );
    assert_polygon(
        "chevron",
        200.0,
        100.0,
        &[(0, 0), (150, 0), (200, 50), (150, 100), (0, 100), (50, 50)],
    );
    assert_polygon(
        "homePlate",
        200.0,
        100.0,
        &[(0, 0), (150, 0), (200, 50), (150, 100), (0, 100)],
    );
    assert_polygon(
        "leftRightArrow",
        200.0,
        100.0,
        &[(0, 50), (50, 0), (150, 0), (200, 50), (150, 100), (50, 100)],
    );
    assert_polygon(
        "plus",
        100.0,
        100.0,
        &[
            (0, 25),
            (25, 25),
            (25, 0),
            (75, 0),
            (75, 25),
            (100, 25),
            (100, 75),
            (25, 100),
        ],
    );
    assert_polygon(
        "hexagon",
        200.0,
        100.0,
        &[(0, 50), (25, 0), (175, 0), (200, 50), (175, 100), (25, 100)],
    );
    // octagon: adj 29289 of ss.
    assert_polygon(
        "octagon",
        100.0,
        100.0,
        &[(0, 29), (29, 0), (71, 0), (100, 29), (100, 71), (71, 100)],
    );
    assert_polygon("pentagon", 100.0, 100.0, &[(50, 0)]);
    assert_polygon("star5", 100.0, 100.0, &[(50, 0)]);
    assert_polygon(
        "flowChartProcess",
        200.0,
        100.0,
        &[(0, 0), (200, 0), (200, 100), (0, 100)],
    );
    assert_polygon(
        "flowChartDecision",
        200.0,
        100.0,
        &[(0, 50), (100, 0), (200, 50), (100, 100)],
    );
    assert_polygon(
        "snip1Rect",
        100.0,
        100.0,
        &[(0, 0), (83, 0), (100, 17), (100, 100), (0, 100)],
    );
    // bentConnector3: an elbow at adj1 = 50 % of the width.
    assert_polygon(
        "bentConnector3",
        200.0,
        100.0,
        &[(0, 0), (100, 0), (100, 100), (200, 100)],
    );
}

#[test]
fn round_and_curved_shapes() {
    // Ellipse: flattened points lie on the ellipse.
    let o = prst("ellipse", 200.0, 100.0);
    assert_eq!(bounds(&o), (0, 0, 200, 100));
    for (c, closed) in o.paths[0].flatten(0.01) {
        assert!(closed);
        for (x, y) in c {
            let e = ((x - 100.0) / 100.0).powi(2) + ((y - 50.0) / 50.0).powi(2);
            assert!(near(e, 1.0, 1e-3), "({x}, {y}): {e}");
        }
    }
    // roundRect: corners cut, edges touched.
    let o = prst("roundRect", 200.0, 100.0);
    assert_eq!(bounds(&o), (0, 0, 200, 100));
    let v = vertices(&o);
    // Corner radius: adj 16667 of ss (100).
    assert!(!v.contains(&(0, 0)) && v.contains(&(17, 0)) && v.contains(&(0, 17)));
    // adj 0 is a rectangle; 50000 on a square is a circle.
    let v = vertices(&adjusted("roundRect", &[("adj", 0)], 200.0, 100.0));
    assert!(v.contains(&(0, 0)) && v.contains(&(200, 100)));
    let o = adjusted("roundRect", &[("adj", 50_000)], 100.0, 100.0);
    for (x, y) in o.paths[0].flatten(0.01).into_iter().flat_map(|c| c.0) {
        assert!(near((x - 50.0).hypot(y - 50.0), 50.0, 0.05), "({x}, {y})");
    }
    for n in [
        "snipRoundRect",
        "round2SameRect",
        "round1Rect",
        "snip2SameRect",
        "frame",
        "flowChartTerminator",
        "heart",
        "cloud",
        "blockArc",
        "bracePair",
        "leftBrace",
        "curvedRightArrow",
    ] {
        let (l, t, r, b) = bounds(&prst(n, 200.0, 100.0));
        assert!(l >= -1 && t >= -1 && r <= 201 && b <= 101, "{n}");
        assert!(r - l >= 99 && b - t >= 49, "{n}: {:?}", (l, t, r, b));
    }
    // Donut: two contours (outer and hole), filled.
    let o = prst("donut", 100.0, 100.0);
    assert_eq!(o.paths[0].flatten(1.0).len(), 2);
    assert_eq!(bounds(&o), (0, 0, 100, 100));
    // Cloud uses its own path space (43200²) scaled to the shape.
    assert_eq!(preset("cloud").unwrap().paths[0].w, 43200);
}

#[test]
fn path_flags_and_fill_modes() {
    // arc: a filled wedge without outline, then the stroked arc unfilled.
    let o = prst("arc", 100.0, 100.0);
    assert_eq!(o.paths.len(), 2);
    assert!(o.paths[0].fill.is_filled() && !o.paths[0].stroke);
    assert!(!o.paths[1].fill.is_filled() && o.paths[1].stroke);
    // can: the top face is lightened.
    let can = prst("can", 100.0, 200.0);
    assert!(can.paths.iter().any(|p| p.fill == FillMode::Lighten));
    assert!(
        prst("curvedConnector3", 100.0, 100.0)
            .paths
            .iter()
            .all(|p| p.fill == FillMode::None)
    );
    // line: one open, stroked path.
    let line = prst("line", 10.0, 10.0);
    assert_eq!(line.paths.len(), 1);
    assert!(line.paths[0].stroke);
    assert_eq!(
        line.paths[0].flatten(1.0),
        vec![(vec![(0.0, 0.0), (10.0, 10.0)], false)]
    );
    let c = Rgb([100, 200, 0]);
    assert_eq!(FillMode::Norm.shade(c), c);
    assert_eq!(FillMode::Darken.shade(c), Rgb([60, 120, 0]));
    assert_eq!(FillMode::DarkenLess.shade(c), Rgb([80, 160, 0]));
    assert_eq!(FillMode::Lighten.shade(c), Rgb([162, 222, 102]));
    assert_eq!(FillMode::LightenLess.shade(c), Rgb([131, 211, 51]));
}

#[test]
fn callout_tails_and_text_rects() {
    // wedgeRectCallout's tail tip: hc + w·adj1, vc + h·adj2.
    let o = prst("wedgeRectCallout", 100.0, 100.0);
    assert!(vertices(&o).contains(&(29, 113)), "{:?}", vertices(&o));
    let o = adjusted(
        "wedgeRoundRectCallout",
        &[("adj1", 50_000), ("adj2", -100_000)],
        100.0,
        100.0,
    );
    assert!(vertices(&o).contains(&(100, -50)));
    // Text rectangles.
    assert_eq!(
        prst("rect", 200.0, 100.0).text_rect,
        Rect {
            l: 0.0,
            t: 0.0,
            r: 200.0,
            b: 100.0
        }
    );
    let r = prst("ellipse", 200.0, 100.0).text_rect;
    let k = 1.0 - std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        near(r.l, 100.0 * k, 0.01)
            && near(r.t, 50.0 * k, 0.01)
            && near(r.r, 200.0 - 100.0 * k, 0.01)
    );
    let r = prst("triangle", 200.0, 100.0).text_rect;
    assert_eq!((r.l, r.t, r.r, r.b), (50.0, 50.0, 150.0, 100.0));
}

#[test]
fn arcs_become_accurate_cubics() {
    // A full circle from (100, 50) around the centre (50, 50).
    for (st, sw) in [
        (0i64, 21_600_000i64),
        (0, -21_600_000),
        (5_400_000, 8_100_000),
    ] {
        let mut segs = Vec::new();
        let end = arc_to(
            (100.0, 50.0),
            50.0,
            50.0,
            st as f64,
            sw as f64,
            &mut |a, b, c| segs.push((a, b, c)),
        );
        assert!(segs.len() >= (sw.abs() / 5_400_000) as usize);
        let mut at = (100.0, 50.0);
        let c = (
            100.0 - 50.0 * (st as f64 / 60000.0).to_radians().cos(),
            50.0 - 50.0 * (st as f64 / 60000.0).to_radians().sin(),
        );
        for (c1, c2, p) in &segs {
            let sub = SubPath {
                cmds: vec![PathCmd::MoveTo(at), PathCmd::CubicTo(*c1, *c2, *p)],
                fill: FillMode::None,
                stroke: true,
                extrusion_ok: false,
            };
            for (x, y) in sub.flatten(0.001).into_iter().flat_map(|c| c.0) {
                let r = (x - c.0).hypot(y - c.1);
                assert!(near(r, 50.0, 0.02), "radius {r}");
            }
            at = *p;
        }
        let a = ((st + sw) as f64 / 60000.0).to_radians();
        assert!(near(end.0, c.0 + 50.0 * a.cos(), 1e-9) && near(end.1, c.1 + 50.0 * a.sin(), 1e-9));
    }
    // Ellipse arcs use visual angles: 45° on a 2:1 ellipse is on the
    // diagonal from the centre.
    let end = arc_to(
        (100.0, 0.0),
        100.0,
        50.0,
        0.0,
        2_700_000.0,
        &mut |_, _, _| {},
    );
    assert!(near(end.0, end.1, 1e-9), "{end:?}");
    let e = (end.0 / 100.0).powi(2) + (end.1 / 50.0).powi(2);
    assert!(near(e, 1.0, 1e-9));
}

#[test]
fn custom_geometry_parses_and_evaluates() {
    let xml = r#"<a:custGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:avLst><a:gd name="k" fmla="val 25000"/></a:avLst><a:gdLst><a:gd name="x1" fmla="*/ w k 100000"/></a:gdLst><a:ahLst/><a:cxnLst/><a:rect l="x1" t="t" r="r" b="b"/><a:pathLst><a:path w="100" h="100"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="100" y="0"/></a:lnTo><a:quadBezTo><a:pt x="100" y="100"/><a:pt x="0" y="100"/></a:quadBezTo><a:close/></a:path><a:path fill="none" stroke="0"><a:moveTo><a:pt x="x1" y="t"/></a:moveTo><a:arcTo wR="10" hR="10" stAng="0" swAng="cd4"/><a:cubicBezTo><a:pt x="1" y="2"/><a:pt x="3" y="4"/><a:pt x="5" y="6"/></a:cubicBezTo></a:path></a:pathLst></a:custGeom>"#;
    let doc = roxmltree::Document::parse(xml).unwrap();
    let g = Geometry::from_xml(doc.root_element()).unwrap();
    let Geometry::Custom(d) = &g else { panic!() };
    assert_eq!(d.av.len(), 1);
    assert_eq!(d.gd[0].fmla.to_string(), "*/ w k 100000");
    assert_eq!(d.paths.len(), 2);
    assert_eq!(d.paths[0].cmds.len(), 4);
    assert_eq!(
        (d.paths[1].fill, d.paths[1].stroke),
        (FillMode::None, false)
    );
    let o = outline(&g, 400.0, 200.0);
    assert_eq!(
        o.text_rect,
        Rect {
            l: 100.0,
            t: 0.0,
            r: 400.0,
            b: 200.0
        }
    );
    // The first path is in its own 100 × 100 space.
    assert_eq!(
        o.paths[0].cmds[..3],
        [
            PathCmd::MoveTo((0.0, 0.0)),
            PathCmd::LineTo((400.0, 0.0)),
            PathCmd::QuadTo((400.0, 200.0), (0.0, 200.0)),
        ]
    );
    // The arc ends a quarter turn on: from (100, 0) round the centre (90, 0).
    let PathCmd::CubicTo(_, _, end) = o.paths[1].cmds[1] else {
        panic!()
    };
    assert!(near(end.0, 90.0, 1e-9) && near(end.1, 10.0, 1e-9));
    assert_eq!(o.paths[1].cmds.len(), 3);
}

#[test]
fn preset_geometry_reads_adjustments() {
    let xml = r#"<a:prstGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" prst="rightArrow"><a:avLst><a:gd name="adj1" fmla="val 80000"/><a:gd name="adj2" fmla="val 20000"/></a:avLst></a:prstGeom>"#;
    let doc = roxmltree::Document::parse(xml).unwrap();
    let g = Geometry::from_xml(doc.root_element()).unwrap();
    assert_eq!(g.preset_name(), Some("rightArrow"));
    assert!(!g.is_basic());
    // Shaft 80 % of the height, head 20 % of ss long.
    let v = vertices(&outline(&g, 200.0, 100.0));
    for p in [(0, 10), (180, 10), (180, 0), (200, 50), (0, 90)] {
        assert!(v.contains(&p), "{p:?} not in {v:?}");
    }
    // Unknown presets draw as their box.
    assert_eq!(bounds(&prst_unknown()), (0, 0, 10, 10));
}

fn prst_unknown() -> Outline {
    outline(&Geometry::preset("noSuchShape"), 10.0, 10.0)
}
