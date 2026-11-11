//! KANTOR PATCH (functions): names of the functions Kantor adds.
//!
//! Upstream keeps every function name in `language/language.bin`, a bitcode
//! blob generated from per-language files outside the crate. Rather than
//! regenerate it, the functions Kantor adds are named here, in English, for
//! every language: `Functions::lookup` falls back to [`lookup`], and
//! `Function::to_localized_name` uses [`name`].

use crate::language::get_default_language;

use super::Function;

const NAMES: [(Function, &str); 8] = [
    (Function::Aggregate, "AGGREGATE"),
    (Function::Groupby, "GROUPBY"),
    (Function::Pivotby, "PIVOTBY"),
    (Function::Bahttext, "BAHTTEXT"),
    (Function::Usdollar, "USDOLLAR"),
    (Function::Dbcs, "DBCS"),
    (Function::Jis, "JIS"),
    (Function::Phonetic, "PHONETIC"),
];

/// The function called `key` (upper case), if it is one of Kantor's.
pub(crate) fn lookup(key: &str) -> Option<Function> {
    NAMES
        .iter()
        .find(|(_, name)| *name == key)
        .map(|(function, _)| function.clone())
}

/// The name of one of Kantor's functions ("" for upstream's).
pub(crate) fn name(function: &Function) -> &'static str {
    NAMES
        .iter()
        .find(|(f, _)| f == function)
        .map_or("", |(_, name)| name)
}

/// The function a bare name stands for when it is passed as a LAMBDA
/// ("eta reduced"), as `SUM` in `GROUPBY(A2:A9,B2:B9,SUM)`. LAMBDA and LET
/// themselves can't be passed that way.
pub(crate) fn eta_function(name: &str) -> Option<Function> {
    let function = get_default_language().functions.lookup(name)?;
    match function {
        Function::Lambda | Function::Let => None,
        f => Some(f),
    }
}

/// Every function name the engine knows (English), sorted: what formula
/// autocomplete offers.
pub fn all_function_names() -> Vec<String> {
    let language = get_default_language();
    let mut names: Vec<String> = Function::into_iter()
        .map(|f| f.to_localized_name(language))
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}
