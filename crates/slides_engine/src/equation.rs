//! Equations in text (`a14:m` Office Math): a text box holding one is
//! inserted from LaTeX or the linear format, and changed in place. Text
//! with equations stays locked against typing (their linear text stands in
//! the paragraph); the equation itself is edited through these commands.

use std::sync::Arc;

use anyhow::{Context as _, bail};
use kantor_math::Equation;
use kantor_math::omml::WriteOptions;

use crate::group::ShapeRef;
use crate::history::{Focus, Kind};
use crate::model::*;
use crate::presentation::Presentation;

/// Why a text body with equations can't be typed in.
pub const LOCKED: &str = "equations";

/// A paragraph holding only an equation.
pub fn equation_para(eq: &Equation) -> TextPara {
    let linear = kantor_math::linear::write(eq);
    let linear = if linear.is_empty() {
        " ".to_owned()
    } else {
        linear
    };
    let xml = kantor_math::omml::write(eq, &WriteOptions::drawing());
    let mut p = TextPara::new(linear.clone());
    p.maths = vec![(0..linear.len(), Arc::from(xml))];
    if eq.display {
        p.props.align = Some(Align::Center);
    }
    p
}

impl Presentation {
    /// Inserts a text box holding an equation in the middle of the slide.
    pub fn add_equation(&mut self, slide: usize, eq: &Equation) -> anyhow::Result<usize> {
        let mut body = TextBody::new(vec![equation_para(eq)]);
        body.locked = Some(LOCKED.into());
        let xfrm = self.centered(self.size.cx / 2, 738_664);
        self.add_shape(
            slide,
            Shape {
                id: 0,
                name: "Equation".into(),
                kind: ShapeKind::Shape {
                    geom: Geom::Rect,
                    text_box: true,
                    geometry: None,
                },
                xfrm: Some(xfrm),
                ph: None,
                look: Look::default(),
                text: Some(body),
                paint: PaintEdit::default(),
                src: None,
            },
        )
    }

    /// The first equation in a shape's text: paragraph, index in it, and
    /// the equation.
    pub fn equation_in(&self, r: impl Into<ShapeRef>) -> Option<(usize, usize, Equation)> {
        let r: ShapeRef = r.into();
        let paras = self.text_paras(r)?;
        for (pi, p) in paras.iter().enumerate() {
            for (mi, (_, xml)) in p.maths.iter().enumerate() {
                if let Ok(eq) = kantor_math::omml::parse(xml) {
                    return Some((pi, mi, eq));
                }
            }
        }
        None
    }

    /// Replaces the first equation in a shape's text (one undo step).
    pub fn set_equation(&mut self, r: impl Into<ShapeRef>, eq: &Equation) -> anyhow::Result<()> {
        let r: ShapeRef = r.into();
        let Some((pi, mi, _)) = self.equation_in(r) else {
            bail!("This shape has no equation");
        };
        self.record(Kind::Other, Some(r), Focus::shape(r.0, r.1), false, |p| {
            let s = p.slide_mut(r.0)?;
            let shape = s.shape_at_mut(r.1, r.2).context("no such shape")?;
            let body = shape.text.as_mut().context("This shape has no text")?;
            let para = body.paras.get_mut(pi).context("no such paragraph")?;
            let (range, _) = para.maths[mi].clone();
            let linear = kantor_math::linear::write(eq);
            let linear = if linear.is_empty() {
                " ".to_owned()
            } else {
                linear
            };
            let props = para.props_at(range.start + 1);
            let delta = linear.len() as isize - range.len() as isize;
            // The old linear text out, the new one in, later equations moved.
            let keep = std::mem::take(&mut para.maths);
            para.delete(range.clone());
            para.insert(range.start, &linear, props);
            para.maths = keep
                .into_iter()
                .enumerate()
                .map(|(k, (r, x))| {
                    if k == mi {
                        let xml = kantor_math::omml::write(eq, &WriteOptions::drawing());
                        (range.start..range.start + linear.len(), Arc::from(xml))
                    } else if r.start >= range.end {
                        let s = (r.start as isize + delta) as usize;
                        let e = (r.end as isize + delta) as usize;
                        (s..e, x)
                    } else {
                        (r, x)
                    }
                })
                .collect();
            Ok(((), Focus::shape(r.0, r.1)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn equation_paragraphs_write_a14_math() {
        let eq = kantor_math::latex::parse(r"\frac{a}{b}+x^2", true).unwrap();
        let p = equation_para(&eq);
        let mut out = String::new();
        crate::shape::write_para(&mut out, &p);
        assert!(out.contains("<a14:m><m:oMathPara xmlns:m="), "{out}");
        assert!(
            out.contains("<a:t>a/b+x^2</a:t></a:r></mc:Fallback>"),
            "{out}"
        );
        let m = out.find("<m:oMathPara").unwrap();
        let end = out.find("</a14:m>").unwrap();
        assert_eq!(kantor_math::omml::parse(&out[m..end]).unwrap(), eq);
    }
}
