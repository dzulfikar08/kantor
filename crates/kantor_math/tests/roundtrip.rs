//! LaTeX → model → LaTeX and model → OMML → model round trips, and
//! layout sanity, over the constructs Word's equation tools make. The
//! bundled font is used throughout: nothing depends on system fonts.

use kantor_math::*;

/// One sample per construct (and a few combinations).
const SAMPLES: &[&str] = &[
    r"\frac{a}{b}",
    r"x^2 + y_i",
    r"x_i^2",
    r"\sqrt{x}",
    r"\sqrt[3]{x+1}",
    r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
    r"\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}",
    r"\int_0^1 f(x)\,dx",
    r"\oint_C \vec{F} \cdot d\vec{r}",
    r"\prod_{k=1}^{n} {a_k + 1}",
    r"\left( \frac{a}{b} \right)",
    r"\left| x \right| + \left\{ y \right\}",
    r"\left\langle u \middle| v \right\rangle",
    r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
    r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}",
    r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}",
    r"f(x) = \begin{cases} x & x \geq 0 \\ -x & x < 0 \end{cases}",
    r"\begin{aligned} a &= b + c \\ &= d \end{aligned}",
    r"\hat{x} + \vec{v} + \bar{y} + \tilde{z} + \dot{a} + \ddot{b}",
    r"\widehat{ABC}",
    r"\overline{AB} + \underline{x}",
    r"\underbrace{a + b}_{n} + \overbrace{c}^{m}",
    r"\lim_{n \to \infty} \frac{1}{n} = 0",
    r"\sin x + \cos(2\theta) + \log_2 n + \ln y",
    r"\sin^2 x + \cos^2 x = 1",
    r"\max_{x \in A} f(x)",
    r"\mathbb{R} + \mathbf{v} + \mathcal{L} + \mathfrak{g} + \mathrm{d}x",
    r"x \text{ if } x > 0",
    r"\boxed{E = mc^2}",
    r"\binom{n}{k} = \frac{n!}{k!(n-k)!}",
    r"\alpha + \beta = \Gamma \cdot \Omega",
    r"\prescript{14}{6}{C}",
    r"a + \phantom{x} b",
    r"\overset{\text{def}}{=}",
    r"\underset{x}{\operatorname{argmin}} f",
    r"\nabla \times \mathbf{E} = -\frac{\partial \mathbf{B}}{\partial t}",
    r"P(A \mid B) = \frac{P(B \mid A) P(A)}{P(B)}",
    r"\bar{x} = \frac{1}{n} \sum_{i=1}^{n} x_i",
    r"Y = \beta_0 + \beta_1 X_1 + \varepsilon",
    r"\sfrac{1}{2}",
];

#[test]
fn latex_round_trips() {
    for src in SAMPLES {
        let a = latex::parse(src, true).unwrap_or_else(|e| panic!("{src}: {e}"));
        let out = latex::write(&a);
        let b = latex::parse(&out, true).unwrap_or_else(|e| panic!("{src} → {out}: {e}"));
        assert_eq!(a, b, "{src} → {out}");
    }
}

#[test]
fn omml_round_trips() {
    for src in SAMPLES {
        let a = latex::parse(src, true).unwrap();
        for o in [omml::WriteOptions::word(), omml::WriteOptions::drawing()] {
            let xml = omml::write(&a, &o);
            let b = omml::parse(&xml).unwrap_or_else(|e| panic!("{src}: {e}\n{xml}"));
            assert_eq!(a, b, "{src}\n{xml}");
        }
        // Inline too.
        let a = latex::parse(src, false).unwrap();
        let xml = omml::write(&a, &omml::WriteOptions::word());
        assert!(xml.starts_with("<m:oMath>"), "{xml}");
        assert_eq!(omml::parse(&xml).unwrap(), a);
    }
}

#[test]
fn omml_is_well_formed_and_in_schema_order() {
    let a = latex::parse(SAMPLES[6], true).unwrap();
    let xml = omml::write(&a, &omml::WriteOptions::drawing());
    let doc = roxmltree::Document::parse(&xml).expect("well-formed with its own xmlns:m");
    let nary = doc
        .descendants()
        .find(|n| n.tag_name().name() == "nary")
        .unwrap();
    let kids: Vec<&str> = nary
        .children()
        .filter(|c| c.is_element())
        .map(|c| c.tag_name().name())
        .collect();
    assert_eq!(kids, ["naryPr", "sub", "sup", "e"]);
}

#[test]
fn reads_word_omml() {
    // As Word 365 saves the quadratic formula (run properties trimmed).
    let xml = r#"<m:oMathPara><m:oMath><m:r><m:t>x=</m:t></m:r><m:f><m:fPr><m:ctrlPr><w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Cambria Math"/><w:i/></w:rPr></m:ctrlPr></m:fPr><m:num><m:r><m:t>-b±</m:t></m:r><m:rad><m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/><m:e><m:sSup><m:e><m:r><m:t>b</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>-4ac</m:t></m:r></m:e></m:rad></m:num><m:den><m:r><m:t>2a</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara>"#;
    let eq = omml::parse(xml).unwrap();
    assert!(eq.display);
    assert_eq!(latex::write(&eq), r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}");
    assert_eq!(linear::write(&eq), "x=(-b±√(b^2-4ac))/(2a)");
    assert_eq!(
        typst::write(&eq),
        "$ x = frac(- b ± sqrt(b^(2) - 4 a c), 2 a) $"
    );
    // Tracked deletions are left out, insertions kept.
    let xml = r#"<m:oMath><w:del><m:r><m:t>y</m:t></m:r></w:del><w:ins><m:r><m:t>x</m:t></m:r></w:ins></m:oMath>"#;
    assert_eq!(latex::write(&omml::parse(xml).unwrap()), "x");
    // Equation numbers in an equation array.
    let xml = r#"<m:oMathPara><m:oMath><m:eqArr><m:e><m:r><m:t>E=m</m:t></m:r><m:sSup><m:e><m:r><m:t>c</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>#(1)</m:t></m:r></m:e></m:eqArr></m:oMath></m:oMathPara>"#;
    let eq = omml::parse(xml).unwrap();
    assert_eq!(
        latex::write(&eq),
        r"\begin{aligned}E=mc^2 \tag{1}\end{aligned}"
    );
}

#[test]
fn linear_input() {
    let eq = parse_input("a/b + x^2", false).unwrap();
    assert_eq!(latex::write(&eq), r"\frac{a}{b}+x^2");
    let eq = parse_input(r"\frac{a}{b}", false).unwrap();
    assert!(matches!(eq.lines[0][0], Node::Frac { .. }));
}

#[test]
fn latex_errors() {
    assert!(latex::parse(r"\frac{a}", false).is_err());
    assert!(latex::parse(r"\foo", false).is_err());
    assert!(latex::parse(r"{a", false).is_err());
    assert!(latex::parse(r"\left( a", false).is_err());
    assert!(latex::parse(r"\begin{pmatrix} a \end{bmatrix}", false).is_err());
}

fn lay(src: &str, display: bool) -> Layout {
    let eq = latex::parse(src, display).unwrap();
    layout(&eq, &Options::default())
}

#[test]
fn every_sample_lays_out() {
    for src in SAMPLES {
        for display in [true, false] {
            let l = lay(src, display);
            assert!(l.width > 0.0, "{src}");
            assert!(l.ascent > 0.0, "{src}");
            assert!(!l.glyphs.is_empty(), "{src}");
            assert!(
                l.glyphs.iter().all(|g| g.id != 0),
                "{src}: a character the math font lacks"
            );
            assert!(
                l.glyphs
                    .iter()
                    .all(|g| g.x.is_finite() && g.y.is_finite() && g.size > 0.0),
                "{src}"
            );
            for g in &l.glyphs {
                assert!(l.text.get(g.cluster.clone()).is_some(), "{src}");
            }
        }
    }
}

#[test]
fn structures_take_room() {
    let x = lay("x", false);
    let frac = lay(r"\frac{a}{b}", true);
    assert!(frac.height() > x.height() * 1.5);
    assert_eq!(frac.rules.len(), 1, "one fraction bar");
    // A display sum is bigger than an inline one.
    let d = lay(r"\sum_{i=1}^{n} i", true);
    let t = lay(r"\sum_{i=1}^{n} i", false);
    assert!(d.height() > t.height());
    // Brackets grow around a fraction.
    let small = lay("(a)", true);
    let big = lay(r"\left(\frac{a}{b}\right)", true);
    assert!(big.height() > small.height() * 1.5);
    // Scripts sit above and below the baseline.
    let s = lay("x_i^2", false);
    let sup = s
        .glyphs
        .iter()
        .find(|g| &s.text[g.cluster.clone()] == "2")
        .unwrap();
    let sub = s
        .glyphs
        .iter()
        .find(|g| &s.text[g.cluster.clone()] == "i")
        .unwrap();
    assert!(sup.y < 0.0 && sub.y > 0.0);
    // Radicals draw a rule over their body.
    let r = lay(r"\sqrt{x}", false);
    assert_eq!(r.rules.len(), 1);
    // Letters are math italic.
    let x = lay("x", false);
    assert_eq!(x.text, "𝑥");
}

#[test]
fn equation_numbers_go_right() {
    let eq = latex::parse(r"E = mc^2 \tag{1}", true).unwrap();
    let l = layout(
        &eq,
        &Options {
            width: Some(600.0),
            ..Options::default()
        },
    );
    assert_eq!(l.width, 600.0);
    let one = l
        .glyphs
        .iter()
        .find(|g| &l.text[g.cluster.clone()] == "1")
        .unwrap();
    assert!(one.x > 500.0);
}

#[test]
fn cached_omml_layout() {
    let eq = latex::parse(SAMPLES[5], true).unwrap();
    let xml = omml::write(&eq, &omml::WriteOptions::word());
    let a = layout_omml(&xml, 16.0, None).unwrap();
    let b = layout_omml(&xml, 16.0, None).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
}
