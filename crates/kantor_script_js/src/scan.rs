//! The top-level functions of a module, found without running it.

use kantor_script::binding::{FunctionInfo, count_params, set_end_lines};

/// The functions a module declares at the start of a line (`function go() {`,
/// `async function load(url) {`), in order. A declaration indented or inside a block is not
/// listed: the Macros dialog lists what a module's top level defines.
pub fn functions(source: &str) -> Vec<FunctionInfo> {
    let mut found = Vec::new();
    let mut lines = 0;
    for (i, line) in source.lines().enumerate() {
        lines = i + 1;
        let rest = line
            .strip_prefix("async ")
            .map(str::trim_start)
            .unwrap_or(line);
        let Some(rest) = rest.strip_prefix("function") else {
            continue;
        };
        // `function*` generators and `functionName = …` aren't declarations of a macro.
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let after = rest[name.len()..].trim_start();
        let Some(list) = after.strip_prefix('(') else {
            continue;
        };
        let params = match list.find(')') {
            Some(end) => count_params(&list[..end]),
            // The list goes on over more lines: it has parameters.
            None => 1,
        };
        found.push(FunctionInfo {
            name,
            params,
            line: i,
            end_line: i,
        });
    }
    set_end_lines(&mut found, lines);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_top_level_functions() {
        let src = "// tools\nfunction go() {\n  function inner() {}\n}\n\nasync function load(url, n) {\n}\nfunction* gen() {}\nconst f = function () {};\nfunction _helper() {}\nfunction multi(\n  a\n) {}\n";
        let f = functions(src);
        let names: Vec<(&str, usize, bool)> = f
            .iter()
            .map(|x| (x.name.as_str(), x.params, x.is_macro()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("go", 0, true),
                ("load", 2, false),
                ("_helper", 0, false),
                ("multi", 1, false)
            ]
        );
        assert_eq!((f[0].line, f[0].end_line), (1, 4));
        assert_eq!(f[3].end_line, 12);
    }
}
