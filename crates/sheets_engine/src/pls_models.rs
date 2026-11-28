//! PLS-SEM models (the path diagrams of Sheets' PLS-SEM editor) stored in
//! the workbook.
//!
//! Like the scripts ([`crate::scripts`]) they live in one custom XML part
//! (`customXml/itemN.xml`), which Excel keeps when it saves and otherwise
//! ignores. Each model is its pls-json text (docs/pls-json.md, with the
//! diagram's `layout`), so the part needs no schema of its own:
//!
//! ```xml
//! <plsModels xmlns="https://kantor.paparanai.id/2026/pls">
//!   <model name="Model 1">{"constructs": [...], "paths": [...]}</model>
//! </plsModels>
//! ```

use roxmltree::Document;

/// The namespace of the models part's root element.
pub const NAMESPACE: &str = "https://kantor.paparanai.id/2026/pls";

/// One model: its name (the tab's title) and its pls-json text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlsModel {
    pub name: String,
    pub json: String,
}

impl PlsModel {
    pub fn new(name: &str, json: &str) -> PlsModel {
        PlsModel {
            name: name.to_owned(),
            json: json.to_owned(),
        }
    }
}

/// The models part's XML.
pub fn to_xml(models: &[PlsModel]) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<plsModels xmlns=\"{NAMESPACE}\">"
    );
    for m in models {
        out.push_str(&format!(
            "\n<model name=\"{}\">{}</model>",
            crate::scripts::escape(&m.name, true),
            crate::scripts::escape(&m.json, false)
        ));
    }
    out.push_str("\n</plsModels>");
    out
}

/// Whether `xml` is a models part.
pub fn is_pls_part(xml: &str) -> bool {
    Document::parse(xml).is_ok_and(|d| {
        let root = d.root_element();
        root.tag_name().name() == "plsModels" && root.tag_name().namespace() == Some(NAMESPACE)
    })
}

/// The models in a models part; `None` when `xml` isn't one.
pub fn from_xml(xml: &str) -> Option<Result<Vec<PlsModel>, String>> {
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "plsModels" || root.tag_name().namespace() != Some(NAMESPACE) {
        return None;
    }
    let mut out = Vec::new();
    for n in root.children().filter(|n| n.is_element()) {
        if n.tag_name().name() != "model" {
            continue;
        }
        let Some(name) = n.attribute("name") else {
            return Some(Err("a PLS-SEM model without a name".into()));
        };
        let json: String = n
            .children()
            .filter(|c| c.is_text())
            .filter_map(|c| c.text())
            .collect();
        out.push(PlsModel::new(name, &json));
    }
    Some(Ok(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let models = vec![
            PlsModel::new(
                "Model <1> & \"2\"",
                r#"{"constructs":[{"name":"A&B","indicators":["x<1"]}],"paths":[]}"#,
            ),
            PlsModel::new("Kepuasan", "{\"constructs\":[],\n\"paths\":[]}"),
        ];
        let xml = to_xml(&models);
        assert!(is_pls_part(&xml));
        assert!(!crate::scripts::is_scripts_part(&xml));
        assert_eq!(from_xml(&xml).unwrap().unwrap(), models);
        assert!(from_xml("<plsModels/>").is_none());
        assert!(from_xml(&crate::scripts::to_xml(&[])).is_none());
    }
}
