//! Office Math (OMML, `m:oMath`) as linear text, the way Office's
//! equation editor writes an equation in its linear format: `a/b` for
//! fractions, `x^2` and `x_i` for scripts, `√(x)` for roots, `∑_(i=1)^n`
//! for big operators. Text shapes show equations this way.

use roxmltree::Node;

const NS_M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

fn kid<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| {
        c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == Some(NS_M)
    })
}

fn val(n: Option<Node>, name: &str) -> Option<String> {
    let c = kid(n?, name)?;
    c.attributes()
        .find(|a| a.name() == "val")
        .map(|a| a.value().to_owned())
}

/// An argument: grouped in parentheses when it's more than one item.
fn arg(n: Option<Node>) -> String {
    let s = n.map(linear).unwrap_or_default();
    if s.chars().count() <= 1 || (s.starts_with('(') && s.ends_with(')')) {
        s
    } else {
        format!("({s})")
    }
}

/// The linear text of an OMML element and what it holds.
pub fn linear(n: Node) -> String {
    if n.is_text() {
        return String::new();
    }
    let name = n.tag_name().name();
    if n.tag_name().namespace() != Some(NS_M) {
        // `a14:m` and other wrappers: their math.
        return n.children().map(linear).collect();
    }
    match name {
        "t" => n.text().unwrap_or_default().to_owned(),
        "f" => format!("{}/{}", arg(kid(n, "num")), arg(kid(n, "den"))),
        "sSup" => format!("{}^{}", arg(kid(n, "e")), arg(kid(n, "sup"))),
        "sSub" => format!("{}_{}", arg(kid(n, "e")), arg(kid(n, "sub"))),
        "sSubSup" => format!(
            "{}_{}^{}",
            arg(kid(n, "e")),
            arg(kid(n, "sub")),
            arg(kid(n, "sup"))
        ),
        "rad" => {
            let deg = kid(n, "deg").map(linear).unwrap_or_default();
            match deg.as_str() {
                "" => format!("√{}", arg(kid(n, "e"))),
                "3" => format!("∛{}", arg(kid(n, "e"))),
                "4" => format!("∜{}", arg(kid(n, "e"))),
                d => format!("√({d}&{})", kid(n, "e").map(linear).unwrap_or_default()),
            }
        }
        "d" => {
            let pr = kid(n, "dPr");
            let beg = val(pr, "begChr").unwrap_or_else(|| "(".into());
            let end = val(pr, "endChr").unwrap_or_else(|| ")".into());
            let sep = val(pr, "sepChr").unwrap_or_else(|| "|".into());
            let items: Vec<String> = n
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "e")
                .map(linear)
                .collect();
            format!("{beg}{}{end}", items.join(&sep))
        }
        "nary" => {
            let op = val(kid(n, "naryPr"), "chr").unwrap_or_else(|| "∫".into());
            let sub = kid(n, "sub").map(linear).unwrap_or_default();
            let sup = kid(n, "sup").map(linear).unwrap_or_default();
            let mut s = op;
            if !sub.is_empty() {
                s.push_str(&format!("_{}", arg(kid(n, "sub"))));
            }
            if !sup.is_empty() {
                s.push_str(&format!("^{}", arg(kid(n, "sup"))));
            }
            format!("{s} {}", kid(n, "e").map(linear).unwrap_or_default())
        }
        "func" => format!(
            "{} {}",
            kid(n, "fName").map(linear).unwrap_or_default(),
            kid(n, "e").map(linear).unwrap_or_default()
        ),
        "bar" | "acc" => {
            let chr = val(kid(n, if name == "acc" { "accPr" } else { "barPr" }), "chr")
                .unwrap_or_else(|| "¯".into());
            format!("{}{chr}", arg(kid(n, "e")))
        }
        "oMathPara" => n
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "oMath")
            .map(linear)
            .collect::<Vec<_>>()
            .join(" "),
        // Properties hold no text.
        n2 if n2.ends_with("Pr") => String::new(),
        _ => n.children().map(linear).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadratic_formula() {
        let xml = r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:r><m:t>x=</m:t></m:r><m:f><m:num><m:r><m:t>−b±</m:t></m:r><m:rad><m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/><m:e><m:sSup><m:e><m:r><m:t>b</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>−4ac</m:t></m:r></m:e></m:rad></m:num><m:den><m:r><m:t>2a</m:t></m:r></m:den></m:f></m:oMath>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        assert_eq!(linear(doc.root_element()), "x=(−b±√(b^2−4ac))/(2a)");
        let sum = r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e><m:r><m:t>i</m:t></m:r></m:e></m:nary></m:oMath>"#;
        let doc = roxmltree::Document::parse(sum).unwrap();
        assert_eq!(linear(doc.root_element()), "∑_(i=1)^n i");
    }
}
