//! Equations: Office Math (`m:oMath` inline, `m:oMathPara` display)
//! kept as paragraph content ([`Level::Para`] objects). Unedited ones are
//! written back byte for byte; new and edited ones are written as Word's
//! equation editor writes them ([`kantor_math::omml`]), so Word opens them
//! as editable equations.

use std::sync::Arc;

use kantor_math::Equation;
use kantor_math::omml::{NS_M, WriteOptions};

use crate::document::{Document, Kind};
use crate::model::*;
use crate::object::{Level, OBJECT, Object, Opaque};

/// Whether an object is an equation.
pub fn is_equation(o: &Object) -> bool {
    matches!(o, Object::Opaque(op) if op.level == Level::Para
        && matches!(crate::model::element_name(&op.xml), "oMath" | "oMathPara"))
}

/// The object for an equation: its OMML, labelled with its LaTeX.
pub fn equation_object(eq: &Equation) -> Object {
    let xml = kantor_math::omml::write(eq, &WriteOptions::word());
    Object::Opaque(Arc::new(Opaque {
        level: Level::Para,
        xml,
        label: kantor_math::latex::write(eq),
    }))
}

/// `xmlns:m` added to a part's root start tag when its content has math
/// and the tag doesn't declare the prefix (parts Kantor made, or a
/// document that had no equations).
pub(crate) fn declare_math_ns(root: &mut String, content: &str) {
    if !content.contains("<m:") || root.contains("xmlns:m=") {
        return;
    }
    let end = if root.ends_with("/>") {
        root.len() - 2
    } else if root.ends_with('>') {
        root.len() - 1
    } else {
        return;
    };
    root.insert_str(end, &format!(" xmlns:m=\"{NS_M}\""));
}

impl Document {
    /// Replaces the selection with an equation (one undo step).
    pub fn insert_equation(&mut self, sel: Selection, eq: &Equation) -> Selection {
        self.insert_object(sel, equation_object(eq))
    }

    /// The equation whose object starts at `pos`: its OMML and model.
    pub fn equation_at(&self, pos: Position) -> Option<(String, Equation)> {
        let p = self.paragraphs.get(pos.para)?;
        let o = p.object_at(pos.offset)?;
        if !is_equation(o) {
            return None;
        }
        let Object::Opaque(op) = o else {
            return None;
        };
        let eq = kantor_math::omml::parse(&op.xml).ok()?;
        Some((op.xml.clone(), eq))
    }

    /// The equation the selection is on: the object selected, or the one
    /// just before or after the caret.
    pub fn equation_near(&self, sel: Selection) -> Option<Position> {
        let (a, b) = sel.ordered();
        let p = self.paragraphs.get(a.para)?;
        let is_eq = |off: usize| p.object_at(off).is_some_and(is_equation);
        if a.para == b.para && b.offset == a.offset + OBJECT.len_utf8() && is_eq(a.offset) {
            return Some(a);
        }
        if sel.is_empty() {
            if is_eq(a.offset) {
                return Some(a);
            }
            if a.offset >= OBJECT.len_utf8() && is_eq(a.offset - OBJECT.len_utf8()) {
                return Some(Position::new(a.para, a.offset - OBJECT.len_utf8()));
            }
        }
        None
    }

    /// Replaces the equation at `pos` (one undo step). `None` when there's
    /// no equation there.
    pub fn set_equation(&mut self, pos: Position, eq: &Equation) -> Option<Selection> {
        let p0 = self.paragraphs.get(pos.para)?;
        if !p0.object_at(pos.offset).is_some_and(is_equation) {
            return None;
        }
        let mut p = p0.clone();
        let mut start = 0;
        for run in &mut p.runs {
            if start == pos.offset && run.object.is_some() {
                run.object = Some(equation_object(eq));
                break;
            }
            start += run.len;
        }
        let sel = Selection {
            anchor: pos,
            head: Position::new(pos.para, pos.offset + OBJECT.len_utf8()),
        };
        self.replace(pos.para, 1, vec![p], (sel, sel), Kind::Other, true);
        Some(sel)
    }

    /// Every equation in the body: where it is and its LaTeX.
    pub fn equations(&self) -> Vec<(Position, bool, String)> {
        let mut out = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            for (off, o) in p.objects() {
                if let Object::Opaque(op) = o
                    && is_equation(o)
                    && let Ok(eq) = kantor_math::omml::parse(&op.xml)
                {
                    out.push((
                        Position::new(i, off),
                        eq.display,
                        kantor_math::latex::write(&eq),
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_the_prefix_once() {
        let mut r = String::from("<w:document xmlns:w=\"w\">");
        declare_math_ns(&mut r, "<m:oMath/>");
        assert_eq!(r, format!("<w:document xmlns:w=\"w\" xmlns:m=\"{NS_M}\">"));
        let before = r.clone();
        declare_math_ns(&mut r, "<m:oMath/>");
        assert_eq!(r, before);
        let mut plain = String::from("<w:hdr>");
        declare_math_ns(&mut plain, "<w:p/>");
        assert_eq!(plain, "<w:hdr>");
    }

    #[test]
    fn insert_edit_undo() {
        let mut doc = Document::new();
        let sel = Selection::caret(Position::new(0, 0));
        let eq = kantor_math::latex::parse(r"\frac{a}{b}", false).unwrap();
        let after = doc.insert_equation(sel, &eq);
        assert_eq!(after.head.offset, OBJECT.len_utf8());
        let at = doc
            .equation_near(after)
            .expect("the equation before the caret");
        let (xml, got) = doc.equation_at(at).unwrap();
        assert!(xml.starts_with("<m:oMath>"));
        assert_eq!(got, eq);
        let eq2 = kantor_math::latex::parse(r"\sqrt{x}", true).unwrap();
        doc.set_equation(at, &eq2).unwrap();
        assert_eq!(doc.equation_at(at).unwrap().1, eq2);
        assert_eq!(doc.equations()[0].2, r"\sqrt{x}");
        doc.undo();
        assert_eq!(doc.equation_at(at).unwrap().1, eq);
    }
}
