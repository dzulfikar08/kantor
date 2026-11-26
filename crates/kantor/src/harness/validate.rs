//! Problems in a build JSON found against the app's JSON Schema, all at
//! once. `build` stops at its first error, so a model fixing one error per
//! build paid a whole step (and a rebuild) for each; when a build fails,
//! these are listed with it.
//!
//! A small subset of JSON Schema: `$ref` (to `#/$defs/…`), `type`,
//! `properties`, `additionalProperties`, `required`, `items`, `enum`,
//! `const`, `anyOf`/`oneOf`/`allOf`. Only used to explain a failed build,
//! never to refuse one, so where it is looser or stricter than the
//! builder nothing breaks.

use serde_json::Value;

/// At most this many problems are listed.
pub const MAX_PROBLEMS: usize = 12;

/// Problems in `v` against `schema` (a whole schema with `$defs`), as
/// `path: what` lines.
pub fn problems(schema: &Value, v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    check(schema, schema, v, "", &mut out, 0);
    out.truncate(MAX_PROBLEMS);
    out
}

fn resolve<'a>(root: &'a Value, s: &'a Value) -> &'a Value {
    let mut s = s;
    for _ in 0..8 {
        let Some(r) = s.get("$ref").and_then(Value::as_str) else {
            break;
        };
        match r
            .strip_prefix("#/$defs/")
            .and_then(|n| root.get("$defs").and_then(|d| d.get(n)))
        {
            Some(next) => s = next,
            None => break,
        }
    }
    s
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Object(_) => "an object",
        Value::Array(_) => "a list",
        Value::String(_) => "text",
        Value::Number(_) => "a number",
        Value::Bool(_) => "true/false",
        Value::Null => "null",
    }
}

fn at(path: &str) -> &str {
    if path.is_empty() {
        "the top level"
    } else {
        path
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s
    }
}

fn check(root: &Value, s: &Value, v: &Value, path: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 40 || out.len() >= MAX_PROBLEMS {
        return;
    }
    let s = resolve(root, s);
    if let Some(all) = s.get("allOf").and_then(Value::as_array) {
        for sub in all {
            check(root, sub, v, path, out, depth + 1);
        }
    }
    for key in ["anyOf", "oneOf"] {
        let Some(any) = s.get(key).and_then(Value::as_array) else {
            continue;
        };
        // The branch of the value's own type, with the fewest problems,
        // explains it best.
        let mut best: Option<Vec<String>> = None;
        let mut types = Vec::new();
        for sub in any {
            let mut e = Vec::new();
            check(root, sub, v, path, &mut e, depth + 1);
            if e.is_empty() {
                best = None;
                types.clear();
                break;
            }
            let sub = resolve(root, sub);
            let declared: Vec<&str> = match sub.get("type") {
                Some(Value::String(t)) => vec![t.as_str()],
                Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            let same = if declared.is_empty() {
                sub.get("properties").is_some() == v.is_object()
            } else {
                declared.iter().any(|t| type_ok(t, v))
            };
            types.extend(declared);
            if same && best.as_ref().is_none_or(|b| e.len() < b.len()) {
                best = Some(e);
            }
        }
        if let Some(e) = best {
            out.extend(e);
            return;
        }
        if !types.is_empty() {
            types.dedup();
            out.push(format!(
                "{}: must be {}, not {}",
                at(path),
                types.join(" or "),
                kind(v)
            ));
            return;
        }
    }
    match s.get("type") {
        Some(Value::String(t)) if !type_ok(t, v) => {
            out.push(format!("{}: must be {t}, not {}", at(path), kind(v)));
            return;
        }
        Some(Value::Array(ts)) if !ts.iter().filter_map(Value::as_str).any(|t| type_ok(t, v)) => {
            let names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
            out.push(format!(
                "{}: must be {}, not {}",
                at(path),
                names.join(" or "),
                kind(v)
            ));
            return;
        }
        _ => {}
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        out.push(format!("{}: must be {c}", at(path)));
        return;
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        let list: Vec<String> = e.iter().take(12).map(Value::to_string).collect();
        out.push(format!(
            "{}: {} is not one of {}",
            at(path),
            short(v),
            list.join(", ")
        ));
        return;
    }
    match v {
        Value::Object(m) => {
            let props = s.get("properties").and_then(Value::as_object);
            if let Some(req) = s.get("required").and_then(Value::as_array) {
                for r in req.iter().filter_map(Value::as_str) {
                    if !m.contains_key(r) {
                        out.push(format!("{}: `{r}` is missing", at(path)));
                    }
                }
            }
            for (k, x) in m {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match (props.and_then(|p| p.get(k)), s.get("additionalProperties")) {
                    (Some(sub), _) => check(root, sub, x, &p, out, depth + 1),
                    (None, Some(Value::Bool(false))) => {
                        let allowed: Vec<&str> = props
                            .map(|p| p.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        out.push(format!(
                            "{}: unknown key \"{k}\" (allowed: {})",
                            at(path),
                            allowed.join(", ")
                        ));
                    }
                    (None, Some(sub @ Value::Object(_))) => check(root, sub, x, &p, out, depth + 1),
                    _ => {}
                }
                if out.len() >= MAX_PROBLEMS {
                    return;
                }
            }
        }
        Value::Array(items) => {
            if let Some(sub) = s.get("items").filter(|i| i.is_object()) {
                for (i, x) in items.iter().enumerate() {
                    check(root, sub, x, &format!("{path}[{i}]"), out, depth + 1);
                    if out.len() >= MAX_PROBLEMS {
                        return;
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[std::prelude::v1::test]
    fn every_problem_is_listed_with_its_path() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "size": {"anyOf": [{"type": "string", "enum": ["16:9", "4:3"]},
                                   {"type": "object", "properties": {"w": {"type": "number"}}}]},
                "slides": {"type": "array", "items": {"$ref": "#/$defs/slide"}}
            },
            "$defs": {"slide": {"type": "object", "additionalProperties": false,
                "required": ["kind"],
                "properties": {"kind": {"type": "string"}, "n": {"type": "integer"}}}}
        });
        let v = json!({"size": [13.3, 7.5], "slides": [{"kind": "cover"}, {"n": 2.5, "x": 1}], "extra": 1});
        let p = problems(&schema, &v);
        assert!(p.iter().any(|l| l.starts_with("size: must be")), "{p:?}");
        assert!(
            p.contains(&"slides[1]: `kind` is missing".to_owned()),
            "{p:?}"
        );
        assert!(
            p.contains(&"slides[1].n: must be integer, not a number".to_owned()),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|l| l.starts_with("slides[1]: unknown key \"x\"")),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|l| l.starts_with("the top level: unknown key \"extra\"")),
            "{p:?}"
        );
        assert!(problems(&schema, &json!({"size": "16:9", "slides": [{"kind": "a"}]})).is_empty());
    }
}
