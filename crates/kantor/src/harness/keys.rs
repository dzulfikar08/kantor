//! A compact summary of a JSON Schema's keys, for the system prompt: one
//! line per definition, so the model writes valid JSON without exploring
//! the schema one call at a time.

use serde_json::Value;

use super::App;

/// Enums with at most this many values are listed.
const MAX_ENUM: usize = 10;

/// `key`, `key*` (required), `key(a|b)` (enum), `key[]{a,b}` (list of
/// objects), `key{a,b}` (object).
fn key_list(def: &Value, depth: usize) -> String {
    let Some(props) = def.get("properties").and_then(Value::as_object) else {
        return String::new();
    };
    let required: Vec<&str> = def
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    props
        .iter()
        .map(|(k, v)| {
            let mut s = k.clone();
            if required.contains(&k.as_str()) {
                s.push('*');
            }
            if let Some(e) = v.get("enum").and_then(Value::as_array)
                && e.len() <= MAX_ENUM
            {
                let vals: Vec<String> = e
                    .iter()
                    .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                    .collect();
                s.push_str(&format!("({})", vals.join("|")));
            } else if depth < 2 {
                let item = v.get("items");
                if let Some(inner) = item.filter(|i| i.get("properties").is_some()) {
                    s.push_str(&format!("[]{{{}}}", key_list(inner, depth + 1)));
                } else if v.get("properties").is_some() {
                    s.push_str(&format!("{{{}}}", key_list(v, depth + 1)));
                }
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The keys of `schema` (Kantor's deck, document or workbook schema).
pub fn summary(app: App, schema: &Value) -> String {
    let what = match app {
        App::Slides => "slides: deck JSON (`build` → .pptx)",
        App::Docs => "docs: document JSON (`build` → .docx)",
        App::Sheets => "sheets: workbook JSON (`build` → .xlsx) and `edit` ops (tool: …_args)",
    };
    let mut out = format!("# Keys, {what}; * = required\ntop: {}", key_list(schema, 1));
    if let Some(defs) = schema.get("$defs").and_then(Value::as_object) {
        let mut plain = Vec::new();
        for (name, def) in defs {
            let keys = key_list(def, 1);
            if keys.is_empty() {
                plain.push(name.as_str());
            } else {
                out.push_str(&format!("\n{name}: {keys}"));
            }
        }
        if !plain.is_empty() {
            out.push_str(&format!(
                "\n(also defined: {}; schema with defs shows them)",
                plain.join(", ")
            ));
        }
    }
    out
}
